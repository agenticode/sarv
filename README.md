# sarv

**The sar cockpit for your terminal: live view and weeks of history, in one static binary.**

Every Linux server that runs the sysstat collector is already sitting on a
goldmine: days or weeks of CPU, memory, disk, network and pressure history in
`/var/log/sa`. Actually reading it usually means `sar -f` text tables scrolled
past a terminal, or exporting SVG files and copying them around. sarv opens
that same data as an interactive terminal UI, on the box where the data lives,
over ssh, with nothing but one binary.

Unlike graphical or web-based sar viewers, sarv also treats *now* as part of
the timeline: by default it tails today's data file and keeps drawing as new
samples arrive, so the same tool answers both "what is happening" and "what
happened last Tuesday at 03:12".

## Features

- **One static binary.** No Java, no Node, no Python, no web server. Build it
  with cargo or drop the musl binary on any x86_64/aarch64 Linux host.
- **Live by default.** Opens today's collection and keeps following it. Need
  finer granularity than your system collector? `sarv -i 1` runs a private
  sadc at a 1-second interval alongside the recorded history.
- **Multi-metric overlay.** Select any combination of metrics with checkboxes
  (memory paging next to cache next to disk await) and read them in a single
  chart. Per-series normalization makes different units comparable; the
  readout below always shows real values.
- **Time cursor you can drive from the keyboard.** Scrub the crosshair across
  the timeline and read exact per-series values at every step; zoom into an
  incident window and pan around it.
- **Ranges up to a month.** 1d, 3d, 7d, 2w, 1m presets (or `--days N`) render
  as one continuous timeline across daily files. Zooming deep into a long
  range reloads just that slice at full resolution.
- **Day-to-day compare.** Overlay any reference date on top of today (or any
  anchor date), aligned by time of day, with signed deltas at the cursor.
- **Built-in metric glossary.** Press `d` on any metric: what it measures, its
  unit, and how to interpret it, without leaving the terminal or opening
  man pages.
- **Bounded memory by design.** Data is downsampled into fixed-size buckets at
  ingest; a month of history costs the same RAM as a day. See
  [Memory discipline](#memory-discipline).
- **Version-tolerant.** Metrics are discovered dynamically from `sadf -j`
  output rather than hardcoded, so new sysstat activities appear
  automatically.

## Quick start

```sh
# from source (Rust 1.74+)
cargo install --git https://github.com/agenticode/sarv

# on the server
sarv                    # today, live-following (needs the sysstat collector)
sarv -r 7d              # last 7 days as one timeline
sarv --days 30          # a month
sarv --compare 2026-07-05   # overlay July 5 on top of today
sarv -i 2               # follow at 2-second resolution via a private sadc
sarv /var/log/sa/sa05   # a specific daily file
sarv export.json        # a saved "sadf -j -- -A" export (works anywhere)
```

Requirements: the `sysstat` package (`sadf` is used to decode data files), and
its collector enabled for history: `systemctl enable --now sysstat`.

## Keys

| Key | Action |
|---|---|
| `Tab` | switch between sidebar and chart |
| `Up`/`Down` or `j`/`k` | move in the sidebar |
| `Space` / `Enter` | select metric (multi-select) / fold group |
| `/` | filter metrics by substring |
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
| `d` | metric glossary |
| `?` | help |
| `q` | quit |

## How it works

sarv shells out to `sadf -j <file> -- -A` and flattens every numeric leaf of
the JSON into a series identified by its path, for example
`cpu-load[all].iowait`, `disk[sda].await` or `network.net-dev[eth0].rxkB`.
Nothing is hardcoded per activity: whatever your sysstat version reports is
what shows up in the sidebar, grouped and foldable.

Live mode polls the current daily file and asks sadf only for samples newer
than the last one seen, so the poll cost stays constant over time. With
`--interval N`, sarv additionally spawns its own `sadc N` writer into a
private temp file and follows that instead, giving second-level live
resolution while the 10-minute system history remains loaded behind it.

## Memory discipline

A monitoring viewer must not become the memory problem it is investigating.

- Every series is downsampled at ingest into a fixed 2048-bucket window
  (average, minimum and maximum per bucket). RAM is O(series x 2048)
  regardless of range length or collection interval.
- Raw samples are never retained; a month of 1-minute data and a day of
  1-second data cost the same.
- Zooming below bucket resolution does not accumulate more data in memory:
  sarv reloads only the zoomed slice (using sadf time filters) at full
  resolution, replacing the previous window.
- Series count is capped (4096 by default) as a safety valve for hosts with
  extreme device counts; the status line reports if truncation happens.

The status bar shows the approximate series-storage footprint of the current
window at all times.

## Related tools

- [ksar](https://github.com/vlsi/ksar): the classic Java desktop grapher for
  sar text output.
- [sargraph / SARchart](https://github.com/sargraph/sargraph.github.io):
  web-based charts from uploaded sar data.
- [sarviewer](https://github.com/juliojsb/sarviewer): gnuplot/matplotlib PNG
  generation scripts.
- [svy](https://github.com/svy-tui/svy): a Node.js TUI focused on browsing
  historical sar data day by day.

sarv aims at the gap between these: a dependency-free binary you can run on
the server itself, that handles live data, arbitrary metric combinations,
long ranges and day-to-day comparison in one place.

## Roadmap

- Prebuilt static release binaries (x86_64, aarch64)
- Bundled demo dataset (`sarv --demo`) to try the UI without sysstat
- Remote mode (`sarv --host web01`) running sadf over ssh
- Saved metric-set presets
- crates.io release

## License

MIT
