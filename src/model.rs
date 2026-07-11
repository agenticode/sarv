//! Data model: fixed-size bucketed series for bounded memory.
//!
//! Every loaded window is downsampled on ingest into a fixed number of
//! buckets per series (avg/min/max), so memory usage is O(series x buckets)
//! regardless of whether the window holds one hour of 1-second samples or a
//! month of 10-minute samples. Zooming below bucket resolution triggers a
//! reload of the narrower window instead of keeping raw samples around.

use std::collections::HashMap;

/// Number of buckets per series per loaded window.
pub const BUCKETS: usize = 2048;

/// Hard cap on distinct series kept per window (safety valve for hosts with
/// thousands of block devices / interfaces).
pub const MAX_SERIES: usize = 4096;

#[derive(Clone)]
pub struct Bucketed {
    pub t0: i64,
    pub t1: i64,
    n: usize,
    pub sum: Vec<f64>,
    pub min: Vec<f64>,
    pub max: Vec<f64>,
    pub cnt: Vec<u32>,
}

impl Bucketed {
    pub fn new(t0: i64, t1: i64, n: usize) -> Self {
        Self {
            t0,
            t1: t1.max(t0 + 1),
            n,
            sum: vec![0.0; n],
            min: vec![f64::INFINITY; n],
            max: vec![f64::NEG_INFINITY; n],
            cnt: vec![0; n],
        }
    }

    #[inline]
    pub fn span(&self) -> f64 {
        (self.t1 - self.t0) as f64 / self.n as f64
    }

    #[inline]
    pub fn idx(&self, ts: i64) -> Option<usize> {
        if ts < self.t0 || ts >= self.t1 {
            return None;
        }
        let i = ((ts - self.t0) as u128 * self.n as u128 / (self.t1 - self.t0) as u128) as usize;
        Some(i.min(self.n - 1))
    }

    pub fn ingest(&mut self, ts: i64, v: f64) {
        if !v.is_finite() {
            return;
        }
        if let Some(i) = self.idx(ts) {
            self.sum[i] += v;
            self.cnt[i] += 1;
            if v < self.min[i] {
                self.min[i] = v;
            }
            if v > self.max[i] {
                self.max[i] = v;
            }
        }
    }

    #[inline]
    pub fn bucket_center(&self, i: usize) -> f64 {
        self.t0 as f64 + (i as f64 + 0.5) * self.span()
    }

    /// Average value of the bucket containing ts, searching up to `reach`
    /// neighbouring buckets when that bucket is empty.
    pub fn value_near(&self, ts: i64, reach: usize) -> Option<(f64, f64, f64, usize)> {
        let c = self.idx(ts)?;
        for d in 0..=reach {
            for i in [c.saturating_sub(d), (c + d).min(self.n - 1)] {
                if self.cnt[i] > 0 {
                    return Some((
                        self.sum[i] / self.cnt[i] as f64,
                        self.min[i],
                        self.max[i],
                        i,
                    ));
                }
            }
        }
        None
    }

    /// (ts, avg) points for buckets overlapping [a, b), for charting.
    pub fn points(&self, a: i64, b: i64) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        for i in 0..self.n {
            if self.cnt[i] == 0 {
                continue;
            }
            let x = self.bucket_center(i);
            if x < a as f64 || x > b as f64 {
                continue;
            }
            out.push((x, self.sum[i] / self.cnt[i] as f64));
        }
        out
    }
}

/// One loaded time window holding every discovered series, bucketed.
pub struct Store {
    pub t0: i64,
    pub t1: i64,
    pub series: HashMap<String, Bucketed>,
    /// Discovery order: keeps sidebar and colors stable across reloads.
    pub order: Vec<String>,
    pub hostname: String,
    pub first_sample_ts: i64,
    pub last_sample_ts: i64,
    pub samples: u64,
    pub truncated: bool,
}

impl Store {
    pub fn new(t0: i64, t1: i64) -> Self {
        Self {
            t0,
            t1,
            series: HashMap::new(),
            order: Vec::new(),
            hostname: String::new(),
            first_sample_ts: i64::MAX,
            last_sample_ts: 0,
            samples: 0,
            truncated: false,
        }
    }

    pub fn ingest(&mut self, id: &str, ts: i64, v: f64) {
        if ts > self.last_sample_ts {
            self.last_sample_ts = ts;
        }
        if ts < self.first_sample_ts {
            self.first_sample_ts = ts;
        }
        if let Some(b) = self.series.get_mut(id) {
            b.ingest(ts, v);
            return;
        }
        if self.series.len() >= MAX_SERIES {
            self.truncated = true;
            return;
        }
        let mut b = Bucketed::new(self.t0, self.t1, BUCKETS);
        b.ingest(ts, v);
        self.series.insert(id.to_string(), b);
        self.order.push(id.to_string());
    }

    /// Approximate resident bytes used by series storage.
    pub fn approx_bytes(&self) -> usize {
        // 3 x f64 + u32 per bucket, plus map/key overhead approximation.
        self.series.len() * (BUCKETS * 28 + 128)
    }
}

/// A row of the sidebar tree, flattened for rendering.
#[derive(Clone)]
pub struct TreeRow {
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub series: Option<String>,
    pub has_children: bool,
    pub expanded: bool,
    /// Number of selected series at or under this row.
    pub sel_under: usize,
}

/// Segments used for the sidebar tree: instance segments are split into a
/// grouping level plus the instance itself, so all instances of an activity
/// fold under one parent.
/// "cpu-load[3].usr" -> ["cpu-load", "cpu-load[3]", "usr"]
pub fn tree_segments(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in segments(id) {
        if let Some(i) = seg.find('[') {
            if seg.ends_with(']') && i > 0 {
                out.push(seg[..i].to_string());
                out.push(seg.clone());
                continue;
            }
        }
        out.push(seg);
    }
    out
}

/// Split a series id into hierarchical segments.
/// "network.net-dev[eth0].rxkB/s" -> ["network", "net-dev[eth0]", "rxkB/s"]
pub fn segments(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for ch in id.chars() {
        match ch {
            '[' => {
                depth += 1;
                cur.push(ch);
            }
            ']' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            '.' if depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_ingest_and_query() {
        let mut b = Bucketed::new(0, 2048, BUCKETS);
        for t in 0..2048 {
            b.ingest(t, t as f64);
        }
        let (avg, min, max, _) = b.value_near(100, 0).unwrap();
        assert_eq!(avg, 100.0);
        assert_eq!(min, 100.0);
        assert_eq!(max, 100.0);
        assert_eq!(b.points(0, 2048).len(), 2048);
    }

    #[test]
    fn bucket_downsamples() {
        let mut b = Bucketed::new(0, 20480, BUCKETS);
        for t in 0..20480 {
            b.ingest(t, 1.0);
        }
        // 10 samples per bucket, all value 1.0
        let (avg, _, _, _) = b.value_near(10000, 0).unwrap();
        assert!((avg - 1.0).abs() < 1e-9);
    }

    #[test]
    fn segment_split() {
        assert_eq!(
            segments("network.net-dev[eth0].rxkB/s"),
            vec!["network", "net-dev[eth0]", "rxkB/s"]
        );
        assert_eq!(
            segments("cpu-load[all].idle"),
            vec!["cpu-load[all]", "idle"]
        );
        assert_eq!(segments("memory.memused"), vec!["memory", "memused"]);
    }

    #[test]
    fn store_caps_series() {
        let mut s = Store::new(0, 100);
        for i in 0..(MAX_SERIES + 10) {
            s.ingest(&format!("m{i}"), 1, 1.0);
        }
        assert_eq!(s.series.len(), MAX_SERIES);
        assert!(s.truncated);
    }
}
