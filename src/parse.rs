//! sadf JSON parsing: dynamically flattens every numeric leaf of
//! `sadf -j ... -- -A` output into (timestamp, series-id, value) triples.
//!
//! No activity is hardcoded: objects recurse with dotted prefixes and arrays
//! of per-instance objects (cpu, iface, disk-device, ...) become
//! `prefix[instance]` segments. This keeps sarv forward-compatible with new
//! sysstat activities and versions.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde_json::Value;

pub struct Meta {
    pub hostname: String,
    pub min_ts: i64,
    pub max_ts: i64,
    pub samples: u64,
}

/// Keys that identify an instance inside per-instance arrays, in priority
/// order. Fallback: first string-valued field of the element.
const IDENT_KEYS: &[&str] = &[
    "cpu",
    "iface",
    "disk-device",
    "mountpoint",
    "filesystem",
    "fchost",
    "tty",
    "temp-device",
    "in-device",
    "fan-device",
    "usb-device",
    "battery",
    "number",
    "intr",
];

/// Fields that are metadata rather than metrics and must never become series.
const SKIP_KEYS: &[&str] = &[
    "timestamp",
    "restarts",
    "comments",
    "file-date",
    "file-utc-time",
];

pub fn parse_sadf_json(text: &str, mut sink: impl FnMut(i64, &str, f64)) -> Result<Meta> {
    let root: Value = serde_json::from_str(text).context("sadf produced invalid JSON")?;
    let mut meta = Meta {
        hostname: String::new(),
        min_ts: i64::MAX,
        max_ts: 0,
        samples: 0,
    };

    let hosts = root
        .pointer("/sysstat/hosts")
        .and_then(Value::as_array)
        .context("unexpected sadf JSON: missing sysstat.hosts")?;

    for host in hosts {
        if let Some(n) = host.get("nodename").and_then(Value::as_str) {
            meta.hostname = n.to_string();
        }
        let Some(stats) = host.get("statistics").and_then(Value::as_array) else {
            continue;
        };
        let mut idbuf = String::with_capacity(64);
        for entry in stats {
            let Some(obj) = entry.as_object() else {
                continue;
            };
            let Some(ts) = parse_timestamp(obj.get("timestamp")) else {
                continue;
            };
            meta.samples += 1;
            meta.min_ts = meta.min_ts.min(ts);
            meta.max_ts = meta.max_ts.max(ts);
            for (k, v) in obj {
                if SKIP_KEYS.contains(&k.as_str()) {
                    continue;
                }
                idbuf.clear();
                idbuf.push_str(k);
                flatten(&mut idbuf, v, ts, &mut sink);
            }
        }
    }
    if meta.min_ts == i64::MAX {
        meta.min_ts = 0;
    }
    Ok(meta)
}

fn parse_timestamp(v: Option<&Value>) -> Option<i64> {
    let o = v?.as_object()?;
    let date = o.get("date")?.as_str()?;
    let time = o.get("time")?.as_str()?;
    let utc = o.get("utc").and_then(Value::as_i64).unwrap_or(0) == 1;
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let t = NaiveTime::parse_from_str(time, "%H:%M:%S").ok()?;
    let ndt = NaiveDateTime::new(d, t);
    let ts = if utc {
        Utc.from_utc_datetime(&ndt).timestamp()
    } else {
        Local
            .from_local_datetime(&ndt)
            .single()
            .map(|dt: DateTime<Local>| dt.timestamp())
            .unwrap_or_else(|| Utc.from_utc_datetime(&ndt).timestamp())
    };
    Some(ts)
}

fn flatten(prefix: &mut String, v: &Value, ts: i64, sink: &mut impl FnMut(i64, &str, f64)) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                sink(ts, prefix, f);
            }
        }
        Value::String(s) => {
            // A few sysstat versions emit numbers as strings.
            if let Ok(f) = s.trim().parse::<f64>() {
                sink(ts, prefix, f);
            }
        }
        Value::Object(map) => {
            let base = prefix.len();
            for (k, v2) in map {
                prefix.push('.');
                prefix.push_str(k);
                flatten(prefix, v2, ts, sink);
                prefix.truncate(base);
            }
        }
        Value::Array(arr) => {
            let base = prefix.len();
            for (i, el) in arr.iter().enumerate() {
                match el {
                    Value::Object(map) => {
                        let ident_key = IDENT_KEYS
                            .iter()
                            .find(|k| map.get(**k).map(is_stringish).unwrap_or(false))
                            .copied()
                            .or_else(|| {
                                map.iter()
                                    .find(|(_, vv)| vv.is_string())
                                    .map(|(k, _)| k.as_str())
                            });
                        let ident = ident_key
                            .and_then(|k| map.get(k))
                            .map(value_to_ident)
                            .unwrap_or_else(|| i.to_string());
                        prefix.push('[');
                        prefix.push_str(&ident);
                        prefix.push(']');
                        for (k, v2) in map {
                            if Some(k.as_str()) == ident_key {
                                continue;
                            }
                            let inner = prefix.len();
                            prefix.push('.');
                            prefix.push_str(k);
                            flatten(prefix, v2, ts, sink);
                            prefix.truncate(inner);
                        }
                        prefix.truncate(base);
                    }
                    other => {
                        prefix.push('[');
                        prefix.push_str(&i.to_string());
                        prefix.push(']');
                        flatten(prefix, other, ts, sink);
                        prefix.truncate(base);
                    }
                }
            }
        }
        _ => {}
    }
}

fn is_stringish(v: &Value) -> bool {
    v.is_string() || v.is_number()
}

fn value_to_ident(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const FIXTURE: &str = r#"{"sysstat": {"hosts": [{
        "nodename": "demo-host",
        "sysname": "Linux",
        "release": "5.14.0",
        "machine": "x86_64",
        "number-of-cpus": 2,
        "file-date": "2026-07-11",
        "file-utc-time": "00:00:01",
        "statistics": [
            {"timestamp": {"date": "2026-07-11", "time": "10:00:01", "utc": 1, "interval": 60},
             "cpu-load": [
                {"cpu": "all", "user": 1.5, "system": 0.5, "idle": 98.0},
                {"cpu": "0", "user": 2.0, "system": 1.0, "idle": 97.0}
             ],
             "memory": {"memfree": 1000, "memused": 2000, "memused-percent": 66.6,
                        "swpcad": "12"},
             "network": {"net-dev": [
                {"iface": "eth0", "rxpck": 10.5, "txpck": 5.25}
             ]},
             "queue": {"runq-sz": 1, "plist-sz": 200, "ldavg-1": 0.5},
             "restarts": []
            },
            {"timestamp": {"date": "2026-07-11", "time": "10:01:01", "utc": 1, "interval": 60},
             "cpu-load": [{"cpu": "all", "user": 2.5, "system": 0.7, "idle": 96.8}],
             "memory": {"memfree": 900, "memused": 2100, "memused-percent": 70.0}
            }
        ]}]}}"#;

    #[test]
    fn parses_fixture() {
        let mut got: HashMap<String, Vec<(i64, f64)>> = HashMap::new();
        let meta = parse_sadf_json(FIXTURE, |ts, id, v| {
            got.entry(id.to_string()).or_default().push((ts, v));
        })
        .unwrap();
        assert_eq!(meta.hostname, "demo-host");
        assert_eq!(meta.samples, 2);
        assert_eq!(got["cpu-load[all].user"].len(), 2);
        assert_eq!(
            got["cpu-load[0].idle"],
            vec![(got["cpu-load[0].idle"][0].0, 97.0)]
        );
        assert_eq!(got["memory.memused-percent"][1].1, 70.0);
        assert_eq!(got["network.net-dev[eth0].rxpck"][0].1, 10.5);
        // numeric string accepted
        assert_eq!(got["memory.swpcad"][0].1, 12.0);
        // identifier string must not become a series
        assert!(!got.contains_key("cpu-load[all].cpu"));
        // timestamps are 60s apart
        let ts: Vec<i64> = got["cpu-load[all].user"].iter().map(|p| p.0).collect();
        assert_eq!(ts[1] - ts[0], 60);
    }
}
