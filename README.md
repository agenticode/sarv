# sarv

**The sar cockpit for your terminal: live view and weeks of history, in one static binary.**

[![CI](https://github.com/agenticode/sarv/actions/workflows/ci.yml/badge.svg)](https://github.com/agenticode/sarv/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/sarv.svg)](https://crates.io/crates/sarv)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.74%2B-orange.svg)](https://www.rust-lang.org)

Every Linux server running the sysstat collector is already sitting on a
goldmine: days or weeks of CPU, memory, disk, network and pressure history in
`/var/log/sa`. Actually reading it usually means `sar -f` text tables scrolled
past a terminal, or exporting graphs somewhere else. sarv opens that same data
as an interactive terminal UI, on the box where the data lives, over ssh, with
nothing but one binary.

Unlike viewers that only browse the past, sarv treats *now* as part of the
timeline: by default it tails today's data file and keeps drawing as new
samples arrive, so one tool answers both "what is happening" and "what
happened last Tuesday at 03:12".

![sarv demo](https://raw.githubusercontent.com/agenticode/sarv/main/docs/demo.gif)

## Features

- **One static binary.** No Java, no Node, no Python, no web server. Build
  with cargo or drop the musl binary onto any x86_64/aarch64 Linux host.
- **Live by default.** Opens today's collection and follows it. Need finer
  granularity than the system collector? `sarv -i 1` runs a private sadc at a
  1-second interval, even on hosts that have never collected history before.
- **Multi-metric overlay.** Select any combination of metrics with checkboxes
  and read them in a single chart: memory paging next to page cache next to
  disk await. Per-series normalization makes different units comparable, and
  the readout below always shows real values.
- **A time cursor you drive from the keyboard.** Scrub the crosshair along the
  timeline and read exact per-series values at every step; zoom into an
  incident window, pan, jump to the latest sample.
- **Ranges up to a month, one continuous timeline.** 1d / 3d / 7d / 2w / 1m
  presets or `--days N`, drawn across daily files. Zooming deep into a long
  range transparently reloads just that slice at full resolution.
- **Day-to-day compare.** Overlay any reference date on the current view,
  aligned by time of day, with signed deltas at the cursor. Works even for
  dates inside an already loaded range or JSON exports.
- **Built-in metric glossary.** Press `d` on any metric: what it measures, its
  unit, and how to read it, without leaving the terminal. No more guessing
  what `%vmeff` or `kbcommit` mean.
- **Timezones for global teams.** Press `t` to re-render every timestamp in
  another zone: Seoul (KST) first, then UTC, China, India, UK, Central
  Europe, US Eastern/Central/Pacific and Australia are presets, and any IANA
  zone works via `--tz`. DST is handled correctly, and the active zone badge
  is always visible in the header.
- **Bounded memory by design.** Fixed-size bucket downsampling at ingest: a
  month of history costs the same RAM as a day. Measured, not promised - see
  below.
- **Version-tolerant.** Metrics are discovered dynamically from `sadf -j`
  output rather than hardcoded, so new sysstat activities appear
  automatically. Verified against sysstat 12.5, 12.6 and 12.7.

## Screenshots

Three days of history with the crosshair parked on a spike, live updates
running:

![overview](https://raw.githubusercontent.com/agenticode/sarv/main/docs/overview.png)

Comparing today against yesterday, aligned by time of day (magenta badge),
with a memory series overlaid on CPU metrics:

![compare](https://raw.githubusercontent.com/agenticode/sarv/main/docs/compare.png)

The glossary explains every selected metric in place:

![glossary](https://raw.githubusercontent.com/agenticode/sarv/main/docs/glossary.png)

## Install

```sh
# from crates.io (Rust 1.74+)
cargo install sarv

# or grab a static binary from the releases page
# https://github.com/agenticode/sarv/releases
```

Runtime requirements: the `sysstat` package (`sadf` decodes the data files).
For history, enable the collector: `systemctl enable --now sysstat`. For
ad-hoc live use without any history, `sarv -i 2` is enough.

## Usage

```sh
sarv                        # today, live-following
sarv -r 7d                  # last 7 days as one timeline
sarv --days 30              # a month
sarv --compare 2026-07-05   # overlay July 5 on today
sarv -i 2                   # 2-second live resolution via a private sadc
sarv --tz Asia/Seoul        # view a UTC server's history in KST
sarv /var/log/sa/sa05       # a specific daily file
sarv export.json            # a saved "sadf -j -- -A" export (works anywhere)
ssh web01 'sadf -j -- -A' > web01.json && sarv web01.json   # remote workflow
```

## Keys

| Key | Action |
|---|---|
| `Tab` | switch between sidebar and chart |
| `Up`/`Down` or `j`/`k` | move in the sidebar |
| `Space` / `Enter` | select metric (multi-select) / fold group |
| `/` | filter metrics by substring, `Esc` clears |
| `x` | clear selection |
| `Left`/`Right` or `h`/`l` | move the time cursor (Shift = larger steps) |
| `g` / `G` | cursor to window start / latest sample |
| `+` / `-` | zoom in / out around the cursor |
| `0` | reset zoom |
| `[` / `]` | pan left / right |
| `r` / `R` | cycle range 1d, 3d, 7d, 2w, 1m |
| `c` / `C` | compare with another day / clear compare |
| `L` | pause or resume live updates |
| `n` | toggle per-series normalization |
| `t` | display timezone picker (Seoul, UTC, US, EU, ...) |
| `d` | metric glossary |
| `?` | help |
| `q` | quit |

## How it works

sarv shells out to `sadf -j <file> -- -A` and flattens every numeric leaf of
the JSON into a series identified by its path: `cpu-load[all].iowait`,
`disk[sda].await`, `network.net-dev[eth0].rxkB`. Nothing is hardcoded per
activity - whatever your sysstat version reports is what shows up in the
sidebar, grouped and foldable per instance.

Live mode polls the current daily file and asks sadf only for samples newer
than the last one seen (sadf's `-s`/`-e` bounds are matched against the file's
raw UTC record times), so the poll cost stays constant as the day grows. If a
sysstat version quirk defeats the filter, sarv detects the heavy responses and
backs off the poll rate automatically. With `--interval N`, sarv spawns its
own `sadc N` writer into a private temp file and follows that instead,
cleaning both up on exit.

## Memory discipline

A monitoring viewer must not become the memory problem it is investigating.
sarv's storage is O(series x 2048 buckets) regardless of range length or
collection interval:

- Every series is downsampled at ingest into a fixed 2048-bucket window
  (average, minimum, maximum per bucket). Raw samples are never retained.
- Zooming below bucket resolution reloads only the zoomed slice at full
  resolution instead of accumulating more data.
- Series count is capped (4096) as a safety valve for pathological hosts.

Measured on a 12-core Rocky Linux 9.6 host (364 series, live polling active):
RSS flat at ~50 MB over a sustained soak with zero growth; a 30-day range
costs within 1% of a 1-day range; valgrind reports no leaks of any kind.

## Verified environments

| Environment | sysstat | Notes |
|---|---|---|
| Rocky Linux 9.6 (x86_64) | 12.5.4 | live, ranges, compare, drill-down |
| Ubuntu 24.04 on WSL2 (x86_64) | 12.6.1 | file mode and `-i` live collection |
| Debian testing container (aarch64) | 12.7.5 | new /usr/libexec sadc layout |

## Related tools

- [ksar](https://github.com/vlsi/ksar): the classic Java desktop grapher for
  sar output.
- [sargraph / SARchart](https://github.com/sargraph/sargraph.github.io):
  web-based charts from uploaded sar data.
- [sarviewer](https://github.com/juliojsb/sarviewer): gnuplot/matplotlib PNG
  generation scripts.
- [svy](https://github.com/svy-tui/svy): a Node.js TUI focused on browsing
  historical sar data day by day.

sarv aims at the gap between these: a dependency-free binary you can run on
the server itself, that handles live data, arbitrary metric combinations,
month-long ranges and day-to-day comparison in one place.

## Roadmap

- Bundled demo dataset (`sarv --demo`)
- Remote mode (`sarv --host web01`) running sadf over ssh
- Saved metric-set presets
- Delta-emphasized compare readout and reference-day styling options

## License

MIT
