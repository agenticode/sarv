//! Rendering: header, sidebar tree, multi-series chart with crosshair,
//! per-series readout, status bar, help and glossary overlays.

use crate::app::{App, InputPurpose, Mode, Pane};
use crate::glossary;
use crate::model::segments;
use chrono::{Local, TimeZone};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, Borders, Chart, Clear, Dataset, GraphType, List, ListItem, ListState, Paragraph,
    Wrap,
};
use ratatui::Frame;

pub const PALETTE: [Color; 8] = [
    Color::Cyan,
    Color::Yellow,
    Color::Green,
    Color::Magenta,
    Color::LightRed,
    Color::LightBlue,
    Color::LightGreen,
    Color::LightMagenta,
];

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, app, header);

    let sidebar_w = 34.min(f.area().width / 3).max(20);
    let [side, right] =
        Layout::horizontal([Constraint::Length(sidebar_w), Constraint::Min(20)]).areas(body);

    draw_sidebar(f, app, side);

    let readout_h = (app.selected.len() as u16 + 2).clamp(3, 11);
    let [chart_area, readout] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(readout_h)]).areas(right);
    draw_chart(f, app, chart_area);
    draw_readout(f, app, readout);

    draw_status(f, app, status);

    match app.mode {
        Mode::Help => draw_help(f),
        Mode::Glossary => draw_glossary(f, app),
        _ => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let live = if app.live.is_some() && app.live_on {
        Span::styled(
            " LIVE ",
            Style::new().fg(Color::Black).bg(Color::Green).bold(),
        )
    } else if app.live.is_some() {
        Span::styled(" PAUSED ", Style::new().fg(Color::Black).bg(Color::Yellow))
    } else {
        Span::styled(
            " HISTORY ",
            Style::new().fg(Color::Black).bg(Color::DarkGray),
        )
    };
    let range = format!(
        " {} | {} .. {} ",
        app.range_label(),
        fmt_ts_full(app.store.t0),
        fmt_ts_full(app.store.t1 - 1)
    );
    let mut spans = vec![
        Span::styled(
            " sarv ",
            Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
        ),
        Span::raw(" "),
        live,
        Span::raw(" "),
        Span::styled(
            format!(" {} ", app.store.hostname),
            Style::new().fg(Color::Cyan).bold(),
        ),
        Span::raw(range),
    ];
    if let Some((d, _)) = &app.compare {
        spans.push(Span::styled(
            format!(" compare:{d} "),
            Style::new().fg(Color::Black).bg(Color::Magenta),
        ));
    }
    if app.normalize {
        spans.push(Span::styled(" norm ", Style::new().fg(Color::DarkGray)));
    }
    if app.drilled {
        spans.push(Span::styled(" drilled ", Style::new().fg(Color::DarkGray)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.pane == Pane::Sidebar;
    let title = if app.filter.is_empty() {
        " metrics ".to_string()
    } else {
        format!(" metrics /{} ", app.filter)
    };
    let border_style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            let mut spans = vec![Span::raw(indent)];
            if row.has_children {
                spans.push(Span::styled(
                    if row.expanded { "- " } else { "+ " },
                    Style::new().fg(Color::DarkGray),
                ));
                spans.push(Span::styled(row.label.clone(), Style::new().bold()));
                if row.sel_under > 0 {
                    spans.push(Span::styled(
                        format!(" ({})", row.sel_under),
                        Style::new().fg(Color::Cyan),
                    ));
                }
            } else if let Some(id) = &row.series {
                let sel_pos = app.selected.iter().position(|s| s == id);
                let (mark, style) = match sel_pos {
                    Some(i) => ("[x] ", Style::new().fg(PALETTE[i % PALETTE.len()]).bold()),
                    None => ("[ ] ", Style::new()),
                };
                spans.push(Span::styled(mark, style));
                spans.push(Span::styled(row.label.clone(), style));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .highlight_style(
            Style::new()
                .bg(Color::Rgb(40, 44, 52))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("");
    let mut state = ListState::default().with_selected(Some(app.sidebar_idx));
    f.render_stateful_widget(list, area, &mut state);
}

struct SeriesDraw {
    label: String,
    color: Color,
    pts: Vec<(f64, f64)>,
    ref_pts: Option<Vec<(f64, f64)>>,
}

fn draw_chart(f: &mut Frame, app: &mut App, area: Rect) {
    app.chart_width = area.width.saturating_sub(10).max(20);
    let focused = app.pane == Pane::Chart;
    let border_style = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!(" cursor {} ", fmt_ts_full(app.cursor)));

    if app.selected.is_empty() {
        let p = Paragraph::new(
            "no metrics selected\n\nswitch to the sidebar (Tab), pick metrics with Space",
        )
        .alignment(Alignment::Center)
        .block(block);
        f.render_widget(p, area);
        return;
    }

    let (a, b) = app.view;
    let shift = app
        .compare
        .as_ref()
        .map(|(_, ref_store)| app.store.t0 - ref_store.t0)
        .unwrap_or(0);

    // Assemble drawable series.
    let mut drawn: Vec<SeriesDraw> = Vec::new();
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut all_pct = true;
    for (i, id) in app.selected.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        let Some(bk) = app.store.series.get(id) else {
            continue;
        };
        let mut pts = bk.points(a, b);
        let mut ref_pts = app.compare.as_ref().and_then(|(_, rs)| {
            rs.series.get(id).map(|rbk| {
                let mut p = rbk.points(a - shift, b - shift);
                for q in &mut p {
                    q.0 += shift as f64;
                }
                p
            })
        });
        if glossary::unit(id) != "%" {
            all_pct = false;
        }
        // Normalization: scale each series (and its reference twin) by the
        // series max over the visible window so different units share one
        // chart meaningfully.
        if app.normalize {
            let mut m = pts.iter().map(|p| p.1.abs()).fold(0.0_f64, f64::max);
            if let Some(rp) = &ref_pts {
                m = rp.iter().map(|p| p.1.abs()).fold(m, f64::max);
            }
            let scale = if m > 0.0 { 100.0 / m } else { 1.0 };
            for p in &mut pts {
                p.1 *= scale;
            }
            if let Some(rp) = &mut ref_pts {
                for p in rp.iter_mut() {
                    p.1 *= scale;
                }
            }
        }
        for p in &pts {
            lo = lo.min(p.1);
            hi = hi.max(p.1);
        }
        if let Some(rp) = &ref_pts {
            for p in rp {
                lo = lo.min(p.1);
                hi = hi.max(p.1);
            }
        }
        drawn.push(SeriesDraw {
            label: short_label(id),
            color,
            pts,
            ref_pts,
        });
    }

    if !lo.is_finite() || !hi.is_finite() {
        lo = 0.0;
        hi = 1.0;
    }
    if app.normalize {
        lo = 0.0;
        hi = 105.0;
    } else if all_pct && lo >= 0.0 && hi <= 100.0 {
        lo = 0.0;
        hi = 100.0;
    } else {
        if lo > 0.0 && lo < hi * 0.5 {
            lo = 0.0;
        }
        let pad = (hi - lo).abs().max(1e-9) * 0.05;
        hi += pad;
        if lo < 0.0 {
            lo -= pad;
        }
    }

    let cross: Vec<(f64, f64)> = vec![(app.cursor as f64, lo), (app.cursor as f64, hi)];

    let mut datasets: Vec<Dataset> = Vec::new();
    for s in &drawn {
        if let Some(rp) = &s.ref_pts {
            datasets.push(
                Dataset::default()
                    .name(format!("{} (ref)", s.label))
                    .marker(symbols::Marker::Braille)
                    .graph_type(GraphType::Line)
                    .style(Style::new().fg(s.color).add_modifier(Modifier::DIM))
                    .data(rp),
            );
        }
        datasets.push(
            Dataset::default()
                .name(s.label.clone())
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(s.color))
                .data(&s.pts),
        );
    }
    datasets.push(
        Dataset::default()
            .marker(symbols::Marker::Bar)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(Color::White).add_modifier(Modifier::DIM))
            .data(&cross),
    );

    let span = b - a;
    let x_labels: Vec<String> = (0..4)
        .map(|i| fmt_ts_axis(a + span * i / 3, span))
        .collect();
    let y_labels: Vec<String> = (0..4)
        .map(|i| {
            let v = lo + (hi - lo) * i as f64 / 3.0;
            if app.normalize {
                format!("{v:.0}%")
            } else {
                fmt_val(v)
            }
        })
        .collect();

    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(
            Axis::default()
                .bounds([a as f64, b as f64])
                .labels(x_labels)
                .style(Style::new().fg(Color::DarkGray)),
        )
        .y_axis(
            Axis::default()
                .bounds([lo, hi])
                .labels(y_labels)
                .style(Style::new().fg(Color::DarkGray)),
        );
    f.render_widget(chart, area);
}

fn draw_readout(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let shift = app
        .compare
        .as_ref()
        .map(|(_, rs)| app.store.t0 - rs.t0)
        .unwrap_or(0);
    for (i, id) in app.selected.iter().enumerate() {
        let color = PALETTE[i % PALETTE.len()];
        let unit = glossary::unit(id);
        let mut spans = vec![
            Span::styled("  * ", Style::new().fg(color).bold()),
            Span::styled(format!("{:<32}", short_label(id)), Style::new().fg(color)),
        ];
        match app
            .store
            .series
            .get(id)
            .and_then(|b| b.value_near(app.cursor, 3))
        {
            Some((avg, mn, mx, _)) => {
                spans.push(Span::styled(
                    format!("{:>12} {:<5}", fmt_val(avg), unit),
                    Style::new().bold(),
                ));
                spans.push(Span::styled(
                    format!(" bucket[{} .. {}]", fmt_val(mn), fmt_val(mx)),
                    Style::new().fg(Color::DarkGray),
                ));
                if let Some((d, rs)) = &app.compare {
                    if let Some(rb) = rs.series.get(id) {
                        if let Some((ravg, _, _, _)) = rb.value_near(app.cursor - shift, 3) {
                            let delta = avg - ravg;
                            let sign = if delta >= 0.0 { "+" } else { "" };
                            let dc = if delta >= 0.0 {
                                Color::Red
                            } else {
                                Color::Green
                            };
                            spans.push(Span::styled(
                                format!("  {sign}{} vs {d}", fmt_val(delta)),
                                Style::new().fg(dc),
                            ));
                        }
                    }
                }
            }
            None => spans.push(Span::styled(
                "           -",
                Style::new().fg(Color::DarkGray),
            )),
        }
        lines.push(Line::from(spans));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(format!(" values @ {} ", fmt_ts_full(app.cursor)));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.mode {
        Mode::Input { purpose, buf } => {
            let prompt = match purpose {
                InputPurpose::CompareDate => "compare with date (YYYY-MM-DD or -N days): ",
                InputPurpose::Filter => "filter: ",
            };
            Line::from(vec![
                Span::styled(prompt, Style::new().fg(Color::Yellow).bold()),
                Span::raw(buf.clone()),
                Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            ])
        }
        _ => {
            let hint = "Tab panes | Space select | arrows move | +/- zoom | r range | c compare | L live | n norm | d glossary | ? help | q quit";
            if app.status.is_empty() {
                Line::from(vec![
                    Span::styled(hint, Style::new().fg(Color::DarkGray)),
                    Span::raw("  "),
                    Span::styled(app.load_info.clone(), Style::new().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(vec![
                    Span::styled(app.status.clone(), Style::new().fg(Color::Yellow)),
                    Span::raw("  |  "),
                    Span::styled(hint, Style::new().fg(Color::DarkGray)),
                ])
            }
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

fn overlay(f: &mut Frame, w: u16, h: u16) -> Rect {
    let area = f.area();
    let w = w.min(area.width.saturating_sub(4));
    let h = h.min(area.height.saturating_sub(2));
    let x = (area.width - w) / 2;
    let y = (area.height - h) / 2;
    let r = Rect::new(x, y, w, h);
    f.render_widget(Clear, r);
    r
}

fn draw_help(f: &mut Frame) {
    let r = overlay(f, 74, 24);
    let rows: &[(&str, &str)] = &[
        ("Tab", "switch between sidebar and chart"),
        ("Up/Down or j/k", "move in the sidebar"),
        ("Space / Enter", "select metric (multi) / fold group"),
        ("/", "filter metrics (Esc clears)"),
        ("x", "clear selection"),
        (
            "Left/Right or h/l",
            "move time cursor (Shift = larger steps)",
        ),
        ("g / G", "cursor to window start / latest sample"),
        ("+ / -", "zoom in / out around the cursor"),
        ("0", "reset zoom (reload full range if drilled)"),
        ("[ / ]", "pan the view left / right"),
        ("r / R", "cycle time range 1d 3d 7d 2w 1m"),
        ("c", "compare with another day (prompt)"),
        ("C", "clear compare"),
        ("L", "pause / resume live updates"),
        ("n", "toggle per-series normalization"),
        ("d", "metric glossary for highlighted and selected metrics"),
        ("?", "this help"),
        ("q", "quit"),
    ];
    let mut lines = vec![Line::from(Span::styled(
        "keys",
        Style::new().bold().fg(Color::Cyan),
    ))];
    for (k, v) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<20}"), Style::new().fg(Color::Yellow)),
            Span::raw(*v),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "press any key to close",
        Style::new().fg(Color::DarkGray),
    )));
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" help "))
        .wrap(Wrap { trim: false });
    f.render_widget(p, r);
}

fn draw_glossary(f: &mut Frame, app: &App) {
    let r = overlay(f, 78, 22);
    let mut lines: Vec<Line> = Vec::new();

    // Describe the highlighted sidebar metric first, then the selection.
    let mut ids: Vec<String> = Vec::new();
    if let Some(row) = app.rows.get(app.sidebar_idx) {
        if let Some(id) = &row.series {
            ids.push(id.clone());
        }
    }
    for id in &app.selected {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    if ids.is_empty() {
        lines.push(Line::raw(
            "select or highlight a metric to see its description",
        ));
    }
    for id in ids.iter().take(6) {
        match glossary::describe(id) {
            Some((unit, desc)) => {
                let u = if unit.is_empty() {
                    String::new()
                } else {
                    format!("  [{unit}]")
                };
                lines.push(Line::from(vec![
                    Span::styled(id.clone(), Style::new().bold().fg(Color::Cyan)),
                    Span::styled(u, Style::new().fg(Color::DarkGray)),
                ]));
                lines.push(Line::raw(format!("  {desc}")));
            }
            None => {
                lines.push(Line::from(Span::styled(
                    id.clone(),
                    Style::new().bold().fg(Color::Cyan),
                )));
                lines.push(Line::raw("  no description yet for this metric"));
            }
        }
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(Span::styled(
        "press any key to close",
        Style::new().fg(Color::DarkGray),
    )));
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" metric glossary "),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, r);
}

/// "network.net-dev[eth0].rxkB" -> "rxkB [eth0]"
pub fn short_label(id: &str) -> String {
    let segs = segments(id);
    let leaf = segs.last().cloned().unwrap_or_default();
    let inst = segs
        .iter()
        .rev()
        .find_map(|s| s.find('[').map(|i| s[i..].to_string()));
    match inst {
        Some(i) => format!("{leaf} {i}"),
        None => {
            if segs.len() >= 2 {
                format!("{leaf} ({})", segs[0])
            } else {
                leaf
            }
        }
    }
}

pub fn fmt_val(v: f64) -> String {
    let a = v.abs();
    if a >= 1e12 {
        format!("{:.2}T", v / 1e12)
    } else if a >= 1e9 {
        format!("{:.2}G", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if a >= 10_000.0 {
        format!("{:.1}k", v / 1e3)
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else if a == 0.0 {
        "0".to_string()
    } else {
        format!("{v:.3}")
    }
}

fn fmt_ts_axis(ts: i64, span: i64) -> String {
    match Local.timestamp_opt(ts, 0).single() {
        Some(t) => {
            if span <= 36 * 3600 {
                t.format("%H:%M").to_string()
            } else {
                t.format("%m-%d %H:%M").to_string()
            }
        }
        None => String::new(),
    }
}

fn fmt_ts_full(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}
