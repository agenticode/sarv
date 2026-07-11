//! Application state and key handling.

use crate::model::{segments, tree_segments, Store, TreeRow};
use crate::source::Live;
use chrono::{Duration as CDuration, NaiveDate};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};

pub const MAX_SELECTED: usize = 8;
pub const RANGES: &[(u32, &str)] = &[(1, "1d"), (3, "3d"), (7, "7d"), (14, "2w"), (30, "1m")];

/// Display timezone presets for the picker: IT hubs the tool's users work
/// with, Seoul first. Any IANA zone also works through --tz.
pub const TZ_PRESETS: &[(&str, &str)] = &[
    ("Asia/Seoul", "Seoul, Korea (KST)"),
    ("UTC", "UTC"),
    ("Asia/Shanghai", "China (CST)"),
    ("Asia/Kolkata", "India (IST)"),
    ("Europe/London", "UK (GMT/BST)"),
    ("Europe/Berlin", "Central Europe (CET/CEST)"),
    ("America/New_York", "US Eastern"),
    ("America/Chicago", "US Central"),
    ("America/Los_Angeles", "US Pacific"),
    ("Australia/Sydney", "Australia (AEST/AEDT)"),
];

#[derive(PartialEq, Clone, Copy)]
pub enum Pane {
    Sidebar,
    Chart,
}

#[derive(PartialEq, Clone)]
pub enum Mode {
    Normal,
    Help,
    Glossary,
    TzPicker(usize),
    Input { purpose: InputPurpose, buf: String },
}

#[derive(PartialEq, Clone, Copy)]
pub enum InputPurpose {
    CompareDate,
    Filter,
}

/// Side effects the main loop must perform (they do I/O).
pub enum Effect {
    Quit,
    /// Reload the current range; clip = Some(view) for drill-down.
    Reload {
        clip: Option<(i64, i64)>,
    },
    LoadCompare(NaiveDate),
}

pub struct App {
    pub store: Store,
    pub compare: Option<(NaiveDate, Store)>,

    pub anchor: NaiveDate,
    pub range_idx: usize,
    pub custom_days: Option<u32>,
    known_series: usize,

    pub live: Option<Live>,
    pub live_on: bool,
    pub follow: bool,

    pub pane: Pane,
    pub mode: Mode,

    pub rows: Vec<TreeRow>,
    pub collapsed: HashSet<String>,
    pub sidebar_idx: usize,
    pub filter: String,

    pub selected: Vec<String>,

    pub view: (i64, i64),
    pub cursor: i64,
    pub drilled: bool,
    pub normalize: bool,

    pub chart_width: u16,
    pub status: String,
    pub load_info: String,

    /// Display timezone: None = system local, Some = a named IANA zone.
    pub tz: Option<chrono_tz::Tz>,
}

impl App {
    pub fn new(store: Store, anchor: NaiveDate, range_idx: usize) -> Self {
        let view = (store.t0, store.t1);
        let cursor = if store.last_sample_ts > 0 {
            store.last_sample_ts
        } else {
            store.t0
        };
        let mut app = Self {
            store,
            compare: None,
            anchor,
            range_idx,
            custom_days: None,
            known_series: 0,
            live: None,
            live_on: false,
            follow: true,
            pane: Pane::Sidebar,
            mode: Mode::Normal,
            rows: Vec::new(),
            collapsed: HashSet::new(),
            sidebar_idx: 0,
            filter: String::new(),
            selected: Vec::new(),
            view,
            cursor,
            drilled: false,
            normalize: true,
            chart_width: 80,
            status: String::new(),
            load_info: String::new(),
            tz: None,
        };
        app.default_collapse();
        app.rebuild_rows();
        app.default_selection();
        app.fit_view_to_data();
        app.known_series = app.store.order.len();
        app
    }

    /// Format a timestamp in the display timezone.
    pub fn fmt_ts(&self, ts: i64, fmt: &str) -> String {
        use chrono::{Local, TimeZone};
        match self.tz {
            Some(z) => z
                .timestamp_opt(ts, 0)
                .single()
                .map(|t| t.format(fmt).to_string())
                .unwrap_or_default(),
            None => Local
                .timestamp_opt(ts, 0)
                .single()
                .map(|t| t.format(fmt).to_string())
                .unwrap_or_default(),
        }
    }

    /// Short badge for the header, e.g. "KST +09:00" or "Local".
    pub fn tz_badge(&self) -> String {
        use chrono::{TimeZone, Utc};
        match self.tz {
            Some(z) => {
                let now = Utc::now().timestamp();
                z.timestamp_opt(now, 0)
                    .single()
                    .map(|t| t.format("%Z %:z").to_string())
                    .unwrap_or_else(|| z.name().to_string())
            }
            None => "Local".to_string(),
        }
    }

    /// Apply a picker selection: 0 = local, 1.. = TZ_PRESETS.
    pub fn apply_tz_pick(&mut self, idx: usize) {
        if idx == 0 {
            self.tz = None;
            self.status = "timezone: system local".into();
            return;
        }
        if let Some((name, label)) = TZ_PRESETS.get(idx - 1) {
            match name.parse::<chrono_tz::Tz>() {
                Ok(z) => {
                    self.tz = Some(z);
                    self.status = format!("timezone: {label} ({})", self.tz_badge());
                }
                Err(_) => self.status = format!("unknown timezone {name}"),
            }
        }
    }

    /// Fit the view to the span that actually has samples (with padding),
    /// so partially collected days do not render as a mostly empty chart.
    pub fn fit_view_to_data(&mut self) {
        let first = if self.store.first_sample_ts == i64::MAX {
            self.store.t0
        } else {
            self.store.first_sample_ts
        };
        let last = if self.store.last_sample_ts == 0 {
            self.store.t1
        } else {
            self.store.last_sample_ts
        };
        let span = (last - first).max(300);
        let pad = (span / 40).max(30);
        self.view = (
            (first - pad).max(self.store.t0),
            (last + pad).min(self.store.t1),
        );
        if self.view.1 - self.view.0 < 300 {
            self.view.1 = (self.view.0 + 300).min(self.store.t1);
        }
        self.clamp_view();
    }

    pub fn range_days(&self) -> u32 {
        self.custom_days.unwrap_or(RANGES[self.range_idx].0)
    }

    pub fn range_label(&self) -> String {
        match self.custom_days {
            Some(d) => format!("{d}d"),
            None => RANGES[self.range_idx].1.to_string(),
        }
    }

    /// Rebuild the sidebar when live polling discovered new series (a device
    /// or interface appeared, or the very first samples arrived when running
    /// with --interval on a host without any history).
    pub fn rebuild_rows_if_new(&mut self) {
        if self.store.order.len() != self.known_series {
            let first_data = self.known_series == 0;
            self.known_series = self.store.order.len();
            if first_data {
                self.default_collapse();
            }
            self.rebuild_rows();
            if self.selected.is_empty() {
                self.default_selection();
            }
            if first_data {
                self.fit_view_to_data();
            }
        }
    }

    /// Collapse every internal node below the top level by default: top
    /// groups stay open, instances and sub-groups start folded.
    pub fn default_collapse(&mut self) {
        self.collapsed.clear();
        for id in &self.store.order {
            let segs = tree_segments(id);
            let mut path = String::new();
            for (i, s) in segs.iter().enumerate() {
                path = if i == 0 {
                    s.clone()
                } else {
                    format!("{path}.{s}")
                };
                let is_leaf = i + 1 == segs.len();
                if !is_leaf && i >= 1 {
                    self.collapsed.insert(path.clone());
                }
            }
        }
    }

    /// Rebuild flattened sidebar rows from the store's discovery order.
    pub fn rebuild_rows(&mut self) {
        #[derive(Default)]
        struct Node {
            children: Vec<String>, // child path keys, first-seen order
            series: Option<String>,
        }
        let mut nodes: HashMap<String, Node> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        let filter = self.filter.to_lowercase();

        for id in &self.store.order {
            if !filter.is_empty() && !id.to_lowercase().contains(&filter) {
                continue;
            }
            let segs = tree_segments(id);
            let mut path = String::new();
            for (i, s) in segs.iter().enumerate() {
                let parent = path.clone();
                path = if i == 0 {
                    s.clone()
                } else {
                    format!("{path}.{s}")
                };
                if !nodes.contains_key(&path) {
                    nodes.insert(path.clone(), Node::default());
                    if i == 0 {
                        if !roots.contains(&path) {
                            roots.push(path.clone());
                        }
                    } else {
                        let p = nodes.get_mut(&parent).unwrap();
                        p.children.push(path.clone());
                    }
                }
            }
            nodes.get_mut(&path).unwrap().series = Some(id.clone());
        }

        let selected: HashSet<&String> = self.selected.iter().collect();
        let mut rows = Vec::new();
        // DFS
        fn visit(
            path: &str,
            depth: usize,
            nodes: &HashMap<String, Node>,
            collapsed: &HashSet<String>,
            selected: &HashSet<&String>,
            force_expand: bool,
            rows: &mut Vec<TreeRow>,
        ) -> usize {
            let node = &nodes[path];
            let label = segments(path).pop().unwrap_or_default();
            let has_children = !node.children.is_empty();
            let expanded = force_expand || !collapsed.contains(path);
            let mut sel_under = 0;
            if let Some(id) = &node.series {
                if selected.contains(id) {
                    sel_under += 1;
                }
            }
            let idx = rows.len();
            rows.push(TreeRow {
                path: path.to_string(),
                label,
                depth,
                series: node.series.clone(),
                has_children,
                expanded,
                sel_under: 0,
            });
            if has_children && expanded {
                for c in &node.children {
                    sel_under +=
                        visit(c, depth + 1, nodes, collapsed, selected, force_expand, rows);
                }
            } else if has_children {
                // still count selections beneath collapsed nodes
                fn count(
                    path: &str,
                    nodes: &HashMap<String, Node>,
                    selected: &HashSet<&String>,
                ) -> usize {
                    let n = &nodes[path];
                    let mut c = 0;
                    if let Some(id) = &n.series {
                        if selected.contains(id) {
                            c += 1;
                        }
                    }
                    for k in &n.children {
                        c += count(k, nodes, selected);
                    }
                    c
                }
                for c in &node.children {
                    sel_under += count(c, nodes, selected);
                }
            }
            rows[idx].sel_under = sel_under;
            sel_under
        }
        let force = !filter.is_empty();
        for r in &roots {
            visit(r, 0, &nodes, &self.collapsed, &selected, force, &mut rows);
        }
        self.rows = rows;
        if self.sidebar_idx >= self.rows.len() {
            self.sidebar_idx = self.rows.len().saturating_sub(1);
        }
    }

    /// Sensible startup selection: overall CPU utilization pieces.
    pub fn default_selection(&mut self) {
        let candidates = [
            "cpu-load[all].user",
            "cpu-load[all].usr",
            "cpu-load[all].system",
            "cpu-load[all].sys",
            "cpu-load[all].iowait",
        ];
        for c in candidates {
            if self.store.series.contains_key(c) && self.selected.len() < 3 {
                self.selected.push(c.to_string());
            }
        }
        if self.selected.is_empty() {
            if let Some(first) = self.store.order.first() {
                self.selected.push(first.clone());
            }
        }
        self.rebuild_rows();
    }

    pub fn toggle_series(&mut self, id: &str) {
        if let Some(pos) = self.selected.iter().position(|s| s == id) {
            self.selected.remove(pos);
        } else if self.selected.len() >= MAX_SELECTED {
            self.status = format!("selection limit {MAX_SELECTED}; deselect something first");
            return;
        } else {
            self.selected.push(id.to_string());
        }
        self.rebuild_rows();
    }

    pub fn view_span(&self) -> i64 {
        (self.view.1 - self.view.0).max(1)
    }

    pub fn clamp_view(&mut self) {
        let span = self.view_span().min(self.store.t1 - self.store.t0);
        if self.view.0 < self.store.t0 {
            self.view = (self.store.t0, self.store.t0 + span);
        }
        if self.view.1 > self.store.t1 {
            self.view = (self.store.t1 - span, self.store.t1);
        }
        self.cursor = self.cursor.clamp(self.view.0, self.view.1 - 1);
    }

    fn cursor_step(&self) -> i64 {
        (self.view_span() / self.chart_width.max(20) as i64).max(1)
    }

    /// Handle a key event; may return an Effect for the main loop.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Effect> {
        // Modal states first.
        match &mut self.mode {
            Mode::Help | Mode::Glossary => {
                self.mode = Mode::Normal;
                return None;
            }
            Mode::TzPicker(idx) => {
                let n = TZ_PRESETS.len() + 1;
                let i = *idx;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('t') | KeyCode::Char('q') => {
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.mode = Mode::TzPicker((i + n - 1) % n);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.mode = Mode::TzPicker((i + 1) % n);
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        self.mode = Mode::Normal;
                        self.apply_tz_pick(i);
                    }
                    _ => {}
                }
                return None;
            }
            Mode::Input { purpose, buf } => {
                match key.code {
                    KeyCode::Esc => {
                        if *purpose == InputPurpose::Filter {
                            self.filter.clear();
                            self.rebuild_rows();
                        }
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Enter => {
                        let text = buf.clone();
                        let purpose = *purpose;
                        self.mode = Mode::Normal;
                        match purpose {
                            InputPurpose::Filter => {} // already applied live
                            InputPurpose::CompareDate => {
                                match parse_date_expr(&text, self.anchor) {
                                    Some(d) => return Some(Effect::LoadCompare(d)),
                                    None => {
                                        self.status =
                                            "compare: use YYYY-MM-DD or -N (days back)".into()
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                        if *purpose == InputPurpose::Filter {
                            self.filter = buf.clone();
                            self.rebuild_rows();
                        }
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        if *purpose == InputPurpose::Filter {
                            self.filter = buf.clone();
                            self.rebuild_rows();
                        }
                    }
                    _ => {}
                }
                return None;
            }
            Mode::Normal => {}
        }

        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') => return Some(Effect::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(Effect::Quit)
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('d') => self.mode = Mode::Glossary,
            KeyCode::Char('t') => {
                let cur = match self.tz {
                    None => 0,
                    Some(z) => TZ_PRESETS
                        .iter()
                        .position(|(name, _)| *name == z.name())
                        .map(|p| p + 1)
                        .unwrap_or(1),
                };
                self.mode = Mode::TzPicker(cur);
            }
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild_rows();
                    self.status = "filter cleared".into();
                } else {
                    self.status.clear();
                }
            }
            KeyCode::Tab => {
                self.pane = match self.pane {
                    Pane::Sidebar => Pane::Chart,
                    Pane::Chart => Pane::Sidebar,
                }
            }
            KeyCode::Char('/') => {
                self.pane = Pane::Sidebar;
                self.mode = Mode::Input {
                    purpose: InputPurpose::Filter,
                    buf: self.filter.clone(),
                };
            }
            KeyCode::Char('n') => {
                self.normalize = !self.normalize;
                self.status = if self.normalize {
                    "normalize: each series scaled to its own max".into()
                } else {
                    "normalize off: shared y-axis with real values".into()
                };
            }
            KeyCode::Char('r') => {
                self.custom_days = None;
                self.range_idx = (self.range_idx + 1) % RANGES.len();
                return Some(Effect::Reload { clip: None });
            }
            KeyCode::Char('R') => {
                self.custom_days = None;
                self.range_idx = (self.range_idx + RANGES.len() - 1) % RANGES.len();
                return Some(Effect::Reload { clip: None });
            }
            KeyCode::Char('c') => {
                self.mode = Mode::Input {
                    purpose: InputPurpose::CompareDate,
                    buf: String::new(),
                };
            }
            KeyCode::Char('C') => {
                self.compare = None;
                self.status = "compare cleared".into();
            }
            KeyCode::Char('L') => {
                self.live_on = !self.live_on;
                self.status = if self.live_on {
                    "live updates on".into()
                } else {
                    "live updates paused".into()
                };
            }
            _ => match self.pane {
                Pane::Sidebar => return self.on_key_sidebar(key),
                Pane::Chart => return self.on_key_chart(key, shift),
            },
        }
        None
    }

    fn on_key_sidebar(&mut self, key: KeyEvent) -> Option<Effect> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sidebar_idx = self.sidebar_idx.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.sidebar_idx + 1 < self.rows.len() {
                    self.sidebar_idx += 1;
                }
            }
            KeyCode::PageUp => self.sidebar_idx = self.sidebar_idx.saturating_sub(15),
            KeyCode::PageDown => {
                self.sidebar_idx = (self.sidebar_idx + 15).min(self.rows.len().saturating_sub(1))
            }
            KeyCode::Home => self.sidebar_idx = 0,
            KeyCode::End => self.sidebar_idx = self.rows.len().saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(row) = self.rows.get(self.sidebar_idx).cloned() {
                    if let Some(id) = &row.series {
                        self.toggle_series(id);
                    } else if row.has_children {
                        if self.collapsed.contains(&row.path) {
                            self.collapsed.remove(&row.path);
                        } else {
                            self.collapsed.insert(row.path.clone());
                        }
                        self.rebuild_rows();
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(row) = self.rows.get(self.sidebar_idx).cloned() {
                    if row.has_children && row.expanded {
                        self.collapsed.insert(row.path.clone());
                        self.rebuild_rows();
                    }
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(row) = self.rows.get(self.sidebar_idx).cloned() {
                    if row.has_children && !row.expanded {
                        self.collapsed.remove(&row.path);
                        self.rebuild_rows();
                    }
                }
            }
            KeyCode::Char('x') => {
                self.selected.clear();
                self.rebuild_rows();
                self.status = "selection cleared".into();
            }
            _ => {}
        }
        None
    }

    fn on_key_chart(&mut self, key: KeyEvent, shift: bool) -> Option<Effect> {
        let step = self.cursor_step() * if shift { 12 } else { 1 };
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.cursor = (self.cursor - step).max(self.view.0);
                self.follow = false;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.cursor = (self.cursor + step).min(self.view.1 - 1);
                self.follow = self.cursor >= self.store.last_sample_ts;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.cursor = self.view.0;
                self.follow = false;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.cursor = self
                    .store
                    .last_sample_ts
                    .clamp(self.view.0, self.view.1 - 1);
                self.follow = true;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let span = (self.view_span() / 2).max(60);
                let c = self.cursor;
                let left = (c - span / 2).max(self.store.t0);
                self.view = (left, (left + span).min(self.store.t1));
                self.clamp_view();
                if self.should_drill() {
                    return Some(Effect::Reload {
                        clip: Some(self.view),
                    });
                }
            }
            KeyCode::Char('-') => {
                let span = (self.view_span() * 2).min(self.store.t1 - self.store.t0);
                let c = self.cursor;
                let left = (c - span / 2).max(self.store.t0);
                self.view = (left, (left + span).min(self.store.t1));
                self.clamp_view();
                if self.drilled && self.view_span() >= (self.store.t1 - self.store.t0) {
                    return Some(Effect::Reload { clip: None });
                }
            }
            KeyCode::Char('0') => {
                if self.drilled {
                    return Some(Effect::Reload { clip: None });
                }
                self.view = (self.store.t0, self.store.t1);
                self.clamp_view();
            }
            KeyCode::Char('[') => {
                let d = self.view_span() / 4;
                self.view = (self.view.0 - d, self.view.1 - d);
                self.clamp_view();
                self.follow = false;
            }
            KeyCode::Char(']') => {
                let d = self.view_span() / 4;
                self.view = (self.view.0 + d, self.view.1 + d);
                self.clamp_view();
            }
            _ => {}
        }
        None
    }

    /// Drill-down: reload a narrow window at full bucket resolution when the
    /// view is much smaller than the loaded window.
    fn should_drill(&self) -> bool {
        let loaded = self.store.t1 - self.store.t0;
        !self.drilled && self.view_span() < loaded / 8 && loaded > 3600
    }

    /// Advance cursor when new live data arrived and we are following;
    /// extend the view to the right so the newest samples stay visible.
    pub fn on_live_data(&mut self) {
        if self.follow {
            let last = self.store.last_sample_ts;
            if last + 30 > self.view.1 {
                let ext = (last + (self.view_span() / 20).max(30)).min(self.store.t1);
                self.view.1 = ext.max(self.view.1);
            }
            self.cursor = last.clamp(self.view.0, self.view.1 - 1);
        }
    }
}

/// "YYYY-MM-DD", "-N" (N days before anchor) or "N" day-of-month shortcuts.
pub fn parse_date_expr(s: &str, anchor: NaiveDate) -> Option<NaiveDate> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    if let Some(stripped) = s.strip_prefix('-') {
        if let Ok(n) = stripped.parse::<i64>() {
            return Some(anchor - CDuration::days(n));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tz_presets_parse_and_format() {
        // Every preset must be a valid IANA zone.
        for (name, _) in TZ_PRESETS {
            assert!(name.parse::<chrono_tz::Tz>().is_ok(), "bad zone {name}");
        }
        // Seoul is first and formats at +09:00 (no DST).
        assert_eq!(TZ_PRESETS[0].0, "Asia/Seoul");
        let mut app = App::new(
            Store::new(0, 86_400),
            NaiveDate::from_ymd_opt(2026, 7, 12).unwrap(),
            0,
        );
        app.apply_tz_pick(1);
        assert_eq!(app.fmt_ts(0, "%Y-%m-%d %H:%M:%S"), "1970-01-01 09:00:00");
        assert!(app.tz_badge().contains("+09:00"));
        app.apply_tz_pick(0);
        assert!(app.tz.is_none());
    }

    #[test]
    fn date_expr() {
        let a = NaiveDate::from_ymd_opt(2026, 7, 12).unwrap();
        assert_eq!(
            parse_date_expr("2026-07-05", a),
            NaiveDate::from_ymd_opt(2026, 7, 5)
        );
        assert_eq!(
            parse_date_expr("-7", a),
            NaiveDate::from_ymd_opt(2026, 7, 5)
        );
        assert_eq!(parse_date_expr("zzz", a), None);
    }
}
