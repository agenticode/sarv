//! Data acquisition: locating sa daily files, invoking sadf, incremental
//! live polling, and optional self-collection through sadc at a custom
//! interval.

use crate::model::Store;
use crate::parse::{parse_sadf_json, Meta};
use anyhow::{bail, Context, Result};
use chrono::{Datelike, Duration as CDuration, Local, NaiveDate, TimeZone};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Candidate directories where distros keep sa daily files.
const SA_DIRS: &[&str] = &["/var/log/sa", "/var/log/sysstat"];

/// Candidate sadc locations (RHEL, Debian/Ubuntu, SUSE).
const SADC_PATHS: &[&str] = &[
    "/usr/lib64/sa/sadc",
    "/usr/lib/sa/sadc",
    "/usr/lib/sysstat/sadc",
    "/usr/local/lib64/sa/sadc",
    "/usr/local/lib/sa/sadc",
];

pub fn find_sadf(explicit: Option<&str>) -> Result<String> {
    if let Some(p) = explicit {
        return Ok(p.to_string());
    }
    if let Ok(out) = Command::new("sadf").arg("-V").output() {
        if out.status.success() || !out.stdout.is_empty() || !out.stderr.is_empty() {
            return Ok("sadf".to_string());
        }
    }
    for p in ["/usr/bin/sadf", "/usr/local/bin/sadf"] {
        if Path::new(p).exists() {
            return Ok(p.to_string());
        }
    }
    bail!("sadf not found; install the sysstat package or pass --sadf PATH")
}

pub fn find_sadc() -> Option<PathBuf> {
    SADC_PATHS.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Files ending in .json are treated as pre-exported `sadf -j` output.
pub fn is_json(p: &Path) -> bool {
    p.extension().map(|e| e == "json").unwrap_or(false)
}

pub fn default_sa_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(d) = explicit {
        if d.is_dir() {
            return Ok(d.to_path_buf());
        }
        bail!("sa directory {} does not exist", d.display());
    }
    for d in SA_DIRS {
        if Path::new(d).is_dir() {
            return Ok(PathBuf::from(d));
        }
    }
    bail!(
        "no sa data directory found (tried {}); is sysstat collecting?",
        SA_DIRS.join(", ")
    )
}

/// The sa daily file for a date, trying both naming schemes.
pub fn day_file(dir: &Path, date: NaiveDate) -> Option<PathBuf> {
    let long = dir.join(format!("sa{}", date.format("%Y%m%d")));
    if long.exists() {
        return Some(long);
    }
    let short = dir.join(format!("sa{:02}", date.day()));
    if short.exists() {
        // Guard against stale same-day-of-month files from previous months:
        // accept only if modified within ~2 days of the requested date.
        if let Ok(md) = short.metadata() {
            if let Ok(mtime) = md.modified() {
                let m: chrono::DateTime<Local> = mtime.into();
                let diff = (m.date_naive() - date).num_days().abs();
                if diff <= 2 {
                    return Some(short);
                }
                return None;
            }
        }
        return Some(short);
    }
    None
}

/// Existing daily files covering `days` days ending at `anchor` (inclusive),
/// oldest first.
pub fn day_files(dir: &Path, anchor: NaiveDate, days: u32) -> Vec<(NaiveDate, PathBuf)> {
    let mut out = Vec::new();
    for back in (0..days).rev() {
        let d = anchor - CDuration::days(back as i64);
        if let Some(p) = day_file(dir, d) {
            out.push((d, p));
        }
    }
    out
}

pub struct LoadStats {
    pub files: usize,
    pub samples: u64,
    pub hostname: String,
}

/// Run sadf for one file, optionally restricted to [since_ts, until_ts).
pub fn run_sadf(
    sadf: &str,
    file: &Path,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
) -> Result<String> {
    let mut cmd = Command::new(sadf);
    cmd.arg("-j");
    if let Some(s) = since_ts {
        let t = Local.timestamp_opt(s, 0).single().context("bad since ts")?;
        cmd.arg("-s").arg(t.format("%H:%M:%S").to_string());
    }
    if let Some(e) = until_ts {
        let t = Local.timestamp_opt(e, 0).single().context("bad until ts")?;
        cmd.arg("-e").arg(t.format("%H:%M:%S").to_string());
    }
    cmd.arg(file);
    cmd.args(["--", "-A"]);
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run {sadf}"))?;
    if !out.status.success() && out.stdout.is_empty() {
        bail!(
            "sadf failed on {}: {}",
            file.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Load a set of daily files into a fresh store covering [t0, t1).
pub fn load_window(
    store: &mut Store,
    sadf: &str,
    files: &[(NaiveDate, PathBuf)],
    clip: Option<(i64, i64)>,
) -> Result<LoadStats> {
    let mut stats = LoadStats {
        files: 0,
        samples: 0,
        hostname: String::new(),
    };
    for (date, path) in files {
        // When clipping (drill-down), restrict sadf to the overlap for speed.
        let (mut s, mut e) = (None, None);
        if let Some((a, b)) = clip {
            let day_start = day_start_ts(*date);
            let day_end = day_start + 86_400;
            if b <= day_start || a >= day_end {
                continue;
            }
            if a > day_start {
                s = Some(a);
            }
            if b < day_end {
                e = Some(b);
            }
        }
        let text = if is_json(path) {
            match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(err) => {
                    eprintln!("sarv: skipping {}: {err}", path.display());
                    continue;
                }
            }
        } else {
            match run_sadf(sadf, path, s, e) {
                Ok(t) => t,
                Err(err) => {
                    // A single unreadable/corrupt day should not sink the window.
                    eprintln!("sarv: skipping {}: {err:#}", path.display());
                    continue;
                }
            }
        };
        let meta: Meta = parse_sadf_json(&text, |ts, id, v| store.ingest(id, ts, v))?;
        stats.files += 1;
        stats.samples += meta.samples;
        if !meta.hostname.is_empty() {
            stats.hostname = meta.hostname;
        }
    }
    store.hostname = stats.hostname.clone();
    Ok(stats)
}

pub fn day_start_ts(d: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

/// Live polling state. Polls either the system daily file or a private file
/// written by our own sadc child at a custom interval.
pub struct Live {
    pub dir: PathBuf,
    pub poll_every: Duration,
    pub last_poll: Instant,
    pub own_child: Option<Child>,
    pub own_file: Option<PathBuf>,
    pub last_seen_ts: i64,
}

impl Live {
    pub fn new(dir: PathBuf, poll_secs: u64) -> Self {
        Self {
            dir,
            poll_every: Duration::from_secs(poll_secs.max(1)),
            last_poll: Instant::now() - Duration::from_secs(3600),
            own_child: None,
            own_file: None,
            last_seen_ts: 0,
        }
    }

    /// Start a private sadc collector at `interval` seconds.
    pub fn start_own_collector(&mut self, interval: u32) -> Result<()> {
        let sadc = find_sadc()
            .context("sadc not found (looked in /usr/lib64/sa, /usr/lib/sa, /usr/lib/sysstat)")?;
        let file = std::env::temp_dir().join(format!("sarv-live-{}.sa", std::process::id()));
        let child = Command::new(&sadc)
            .arg("-S")
            .arg("ALL")
            .arg(interval.to_string())
            .arg("1000000")
            .arg(&file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {}", sadc.display()))?;
        self.own_child = Some(child);
        self.own_file = Some(file);
        Ok(())
    }

    pub fn current_file(&self, today: NaiveDate) -> Option<PathBuf> {
        if let Some(f) = &self.own_file {
            if f.exists() {
                return Some(f.clone());
            }
            return None;
        }
        day_file(&self.dir, today)
    }

    /// Fetch samples newer than last_seen_ts from the live file.
    pub fn poll(&mut self, store: &mut Store, sadf: &str) -> Result<bool> {
        self.last_poll = Instant::now();
        let today = Local::now().date_naive();
        let Some(file) = self.current_file(today) else {
            return Ok(false);
        };
        let since = if self.last_seen_ts > 0 {
            Some(self.last_seen_ts + 1)
        } else {
            None
        };
        let text = match run_sadf(sadf, &file, since, None) {
            Ok(t) => t,
            Err(_) => return Ok(false), // transient (file rotating, empty since-range)
        };
        let before = store.samples;
        let meta = parse_sadf_json(&text, |ts, id, v| {
            if ts > self.last_seen_ts {
                store.ingest(id, ts, v);
            }
        })?;
        store.samples = before + meta.samples;
        if meta.max_ts > self.last_seen_ts {
            self.last_seen_ts = meta.max_ts;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn due(&self) -> bool {
        self.last_poll.elapsed() >= self.poll_every
    }

    pub fn shutdown(&mut self) {
        if let Some(mut c) = self.own_child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(f) = self.own_file.take() {
            let _ = std::fs::remove_file(f);
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.shutdown();
    }
}
