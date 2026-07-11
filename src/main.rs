mod app;
mod glossary;
mod model;
mod parse;
mod source;
mod ui;

use anyhow::{anyhow, bail, Context, Result};
use app::{App, Effect, RANGES};
use chrono::{Duration as CDuration, Local, NaiveDate};
use clap::Parser;
use model::Store;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use source::{day_file, day_files, day_start_ts, default_sa_dir, find_sadf, load_window, Live};
use std::path::PathBuf;
use std::time::Duration;

/// Interactive terminal cockpit for sar/sysstat data: live view, weeks of
/// history, multi-metric overlay and day-to-day compare.
#[derive(Parser)]
#[command(name = "sarv", version, about, long_about = None)]
struct Cli {
    /// sa data files or sadf -j JSON exports to open (implies --no-live)
    paths: Vec<PathBuf>,

    /// Directory with sa daily files (default: /var/log/sa or /var/log/sysstat)
    #[arg(long)]
    dir: Option<PathBuf>,

    /// Time range preset: 1d, 3d, 7d, 2w, 1m
    #[arg(short = 'r', long, default_value = "1d")]
    range: String,

    /// Custom range in days (overrides --range)
    #[arg(long)]
    days: Option<u32>,

    /// Anchor date (default: today)
    #[arg(long)]
    date: Option<NaiveDate>,

    /// Overlay this date as a comparison reference
    #[arg(long)]
    compare: Option<NaiveDate>,

    /// Force live updates on
    #[arg(long)]
    live: bool,

    /// Disable live updates
    #[arg(long)]
    no_live: bool,

    /// Collect at this interval (seconds) with a private sadc, for
    /// finer-grained live data than the system collector
    #[arg(short = 'i', long)]
    interval: Option<u32>,

    /// Live poll period in seconds
    #[arg(long, default_value_t = 5)]
    poll: u64,

    /// Path to the sadf binary
    #[arg(long)]
    sadf: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let sadf = match find_sadf(cli.sadf.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            // JSON-only invocations work without sadf.
            if !cli.paths.is_empty() && cli.paths.iter().all(|p| source::is_json(p)) {
                String::new()
            } else {
                return Err(e);
            }
        }
    };

    let anchor = cli.date.unwrap_or_else(|| Local::now().date_naive());
    let range_idx = RANGES
        .iter()
        .position(|(_, l)| *l == cli.range)
        .ok_or_else(|| anyhow!("--range must be one of 1d, 3d, 7d, 2w, 1m"))?;

    // ---- initial load ----
    let mut store;
    let explicit = !cli.paths.is_empty();
    if explicit {
        store = load_explicit(&sadf, &cli.paths)?;
    } else {
        let days = cli.days.unwrap_or(RANGES[range_idx].0);
        let dir = default_sa_dir(cli.dir.as_deref()).unwrap_or_default();
        let files = if dir.as_os_str().is_empty() {
            Vec::new()
        } else {
            day_files(&dir, anchor, days)
        };
        if files.is_empty() && cli.interval.is_none() {
            bail!(
                "no sa daily files found for the last {} day(s); enable the \
                 collector (systemctl enable --now sysstat) for history, or \
                 run with --interval N to collect live data directly",
                days
            );
        }
        let t0 = day_start_ts(anchor - CDuration::days(days as i64 - 1));
        let t1 = day_start_ts(anchor) + 86_400;
        store = Store::new(t0, t1);
        load_window(&mut store, &sadf, &files, None)?;
    }
    if store.order.is_empty() && cli.interval.is_none() {
        bail!("no metrics found in the input data");
    }

    let mut app = App::new(store, anchor, range_idx);
    app.custom_days = cli.days;

    // ---- live setup ----
    let live_default = !explicit && anchor == Local::now().date_naive();
    let live_on = (live_default || cli.live) && !cli.no_live;
    if live_on {
        let dir = default_sa_dir(cli.dir.as_deref()).unwrap_or_else(|_| std::env::temp_dir());
        let mut live = Live::new(
            dir,
            cli.poll
                .min(cli.interval.map(u64::from).unwrap_or(u64::MAX)),
        );
        live.last_seen_ts = app.store.last_sample_ts;
        if let Some(iv) = cli.interval {
            live.start_own_collector(iv.max(1))
                .context("failed to start private sadc collector for --interval")?;
            app.status = format!("private sadc collector started at {iv}s interval");
        }
        app.live = Some(live);
        app.live_on = true;
    }

    // ---- compare preload ----
    let mut pending: Option<Effect> = cli.compare.map(Effect::LoadCompare);

    // ---- TUI loop ----
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &cli, &sadf, &mut pending);
    ratatui::restore();
    if let Some(live) = &mut app.live {
        live.shutdown();
    }
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cli: &Cli,
    sadf: &str,
    pending: &mut Option<Effect>,
) -> Result<()> {
    loop {
        if let Some(effect) = pending.take() {
            match effect {
                Effect::Quit => return Ok(()),
                Effect::Reload { clip } => {
                    app.status = "loading...".into();
                    terminal.draw(|f| ui::draw(f, app))?;
                    match reload(app, cli, sadf, clip) {
                        Ok(()) => app.status.clear(),
                        Err(e) => app.status = format!("reload failed: {e:#}"),
                    }
                }
                Effect::LoadCompare(date) => {
                    app.status = format!("loading {date} for compare...");
                    terminal.draw(|f| ui::draw(f, app))?;
                    match load_compare(app, cli, sadf, date) {
                        Ok(()) => app.status = format!("comparing against {date}"),
                        Err(e) => app.status = format!("compare failed: {e:#}"),
                    }
                }
            }
        }

        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    *pending = app.on_key(k);
                }
                _ => {}
            }
            continue;
        }

        // Idle tick: live polling and midnight rollover.
        if app.live_on && app.live.is_some() {
            let today = Local::now().date_naive();
            if today != app.anchor {
                app.anchor = today;
                *pending = Some(Effect::Reload { clip: None });
                continue;
            }
            let due = app.live.as_ref().map(|l| l.due()).unwrap_or(false);
            if due {
                if let Some(mut live) = app.live.take() {
                    let polled = live.poll(&mut app.store, sadf);
                    app.live = Some(live);
                    if let Ok(true) = polled {
                        app.on_live_data();
                        app.rebuild_rows_if_new();
                    }
                }
            }
        }
    }
}

fn reload(app: &mut App, cli: &Cli, sadf: &str, clip: Option<(i64, i64)>) -> Result<()> {
    let days = app.range_days();
    let dir = default_sa_dir(cli.dir.as_deref())?;
    let files = day_files(&dir, app.anchor, days);
    if files.is_empty() {
        bail!(
            "no sa files for the last {days} day(s) in {}",
            dir.display()
        );
    }
    let (t0, t1) = match clip {
        Some(c) => c,
        None => (
            day_start_ts(app.anchor - CDuration::days(days as i64 - 1)),
            day_start_ts(app.anchor) + 86_400,
        ),
    };
    let mut store = Store::new(t0, t1);
    let stats = load_window(&mut store, sadf, &files, clip)?;
    app.store = store;
    app.drilled = clip.is_some();
    if let Some(live) = &mut app.live {
        live.last_seen_ts = app.store.last_sample_ts;
    }
    app.cursor = if app.store.last_sample_ts > 0 {
        app.store.last_sample_ts.clamp(t0, t1 - 1)
    } else {
        t0
    };
    if clip.is_some() {
        app.view = (t0, t1);
        app.clamp_view();
    } else {
        app.fit_view_to_data();
    }
    app.default_collapse();
    app.rebuild_rows();
    app.load_info = format!(
        "{} file(s) | {} samples | {} series | ~{:.1} MiB",
        stats.files,
        stats.samples,
        app.store.order.len(),
        app.store.approx_bytes() as f64 / 1_048_576.0
    );
    Ok(())
}

fn load_compare(app: &mut App, cli: &Cli, sadf: &str, date: NaiveDate) -> Result<()> {
    // Prefer the real sa daily file when one exists.
    if !sadf.is_empty() {
        if let Ok(dir) = default_sa_dir(cli.dir.as_deref()) {
            if let Some(file) = day_file(&dir, date) {
                let t0 = day_start_ts(date);
                let mut store = Store::new(t0, t0 + 86_400);
                load_window(&mut store, sadf, &[(date, file)], None)?;
                if !store.order.is_empty() {
                    app.compare = Some((date, store));
                    return Ok(());
                }
            }
        }
    }
    // Fall back to resampling the already loaded window (works for JSON
    // inputs and for dates inside a multi-day range).
    if let Some(store) = synth_compare_from_window(app, date) {
        app.compare = Some((date, store));
        return Ok(());
    }
    bail!("no sa file for {date} and the date is not inside the loaded window")
}

/// Build a one-day reference store by resampling the loaded window's buckets.
fn synth_compare_from_window(app: &App, date: NaiveDate) -> Option<Store> {
    let t0 = day_start_ts(date);
    let t1 = t0 + 86_400;
    if t1 <= app.store.t0 || t0 >= app.store.t1 {
        return None;
    }
    let mut store = Store::new(t0, t1);
    for id in &app.store.order {
        if let Some(b) = app.store.series.get(id) {
            for i in 0..b.cnt.len() {
                if b.cnt[i] == 0 {
                    continue;
                }
                let ts = b.bucket_center(i) as i64;
                if ts >= t0 && ts < t1 {
                    store.ingest(id, ts, b.sum[i] / b.cnt[i] as f64);
                }
            }
        }
    }
    if store.last_sample_ts == 0 {
        return None;
    }
    store.hostname = app.store.hostname.clone();
    Some(store)
}

/// Load explicitly passed sa/JSON files in two passes so memory stays
/// bounded: pass 1 scans only for the covered time window, pass 2 ingests
/// straight into the bucketed store. Raw samples are never accumulated.
fn load_explicit(sadf: &str, paths: &[PathBuf]) -> Result<Store> {
    let texts: Vec<String> = paths
        .iter()
        .map(|p| {
            if source::is_json(p) {
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))
            } else {
                if sadf.is_empty() {
                    bail!("sadf is required to read {}", p.display());
                }
                source::run_sadf(sadf, p, None, None)
            }
        })
        .collect::<Result<_>>()?;

    // Pass 1: find the overall window.
    let (mut t0, mut t1) = (i64::MAX, 0i64);
    let mut samples = 0u64;
    for text in &texts {
        let meta = parse::parse_sadf_json(text, |_, _, _| {})?;
        if meta.samples > 0 {
            t0 = t0.min(meta.min_ts);
            t1 = t1.max(meta.max_ts);
            samples += meta.samples;
        }
    }
    if samples == 0 {
        bail!("no samples found in the given files");
    }

    // Pass 2: ingest directly into fixed buckets.
    let mut store = Store::new(t0, t1 + 1);
    let mut hostname = String::new();
    for text in &texts {
        let meta = parse::parse_sadf_json(text, |ts, id, v| store.ingest(id, ts, v))?;
        if !meta.hostname.is_empty() {
            hostname = meta.hostname;
        }
    }
    store.hostname = hostname;
    store.samples = samples;
    Ok(store)
}
