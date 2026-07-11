//! Metric glossary: human explanations for sar metrics, keyed by the JSON
//! leaf names produced by `sadf -j`. Shown in the in-app glossary pane so
//! users do not need to memorize the sar(1) man page.

use crate::model::segments;

/// (group, leaf, unit, description). Group "*" matches any group.
const ENTRIES: &[(&str, &str, &str, &str)] = &[
    // ---- CPU ----
    ("cpu-load", "user", "%", "Time spent running unprivileged user code. Excludes nice-adjusted tasks. Sustained high user time means the CPUs are busy with application work."),
    ("cpu-load", "usr", "%", "User time excluding time spent running virtual CPUs for guests. High values mean application-level CPU pressure."),
    ("cpu-load", "nice", "%", "User-level time for processes with a positive nice value (lowered priority). Batch or background jobs usually show up here."),
    ("cpu-load", "system", "%", "Time spent in the kernel on behalf of processes: syscalls, memory management, network and block I/O processing. High system time with low user time often points at I/O, networking or syscall-heavy workloads."),
    ("cpu-load", "sys", "%", "Kernel (system) time, excluding IRQ/softirq time. See system."),
    ("cpu-load", "iowait", "%", "Time the CPU was idle while at least one task was blocked on disk I/O. Persistent iowait means storage cannot keep up; near-zero iowait does not prove storage is fine on multi-core systems."),
    ("cpu-load", "steal", "%", "Time a virtual CPU wanted to run but the hypervisor gave the physical CPU to someone else. Nonzero steal on a VM means noisy neighbours or an oversubscribed host."),
    ("cpu-load", "irq", "%", "Time servicing hardware interrupts."),
    ("cpu-load", "soft", "%", "Time servicing softirqs (network RX/TX processing, timers, tasklets). High soft with heavy traffic is normal; high soft while idle deserves a look at network settings."),
    ("cpu-load", "guest", "%", "Time spent running virtual CPUs of KVM guests (already included in user on some kernels; sar subtracts it)."),
    ("cpu-load", "gnice", "%", "Guest time for nice-adjusted guests."),
    ("cpu-load", "idle", "%", "Time with nothing to run and no outstanding disk I/O. The classic utilization headline: 100 - idle."),
    ("cpu-frequency", "frequency", "MHz", "Average CPU clock during the interval. Compare against nominal frequency to spot power-save throttling or turbo behaviour."),
    // ---- Memory ----
    ("memory", "memfree", "kB", "Completely unused RAM. Low memfree alone is NOT a problem on Linux: the kernel deliberately keeps memory filled with cache. Look at avail instead."),
    ("memory", "avail", "kB", "Kernel estimate of memory available for new workloads without swapping (includes reclaimable cache). This is the number to alert on, not memfree."),
    ("memory", "memused", "kB", "RAM in use as computed by sar (total - free - buffers - cache - slab). Rough working-set indicator."),
    ("memory", "memused-percent", "%", "memused as a percentage of total RAM."),
    ("memory", "buffers", "kB", "Block-device metadata cache (filesystem metadata, raw block reads). Reclaimable."),
    ("memory", "cached", "kB", "Page cache: file contents cached in RAM. Grows to fill free memory by design and is reclaimed under pressure; a large value is healthy, not a leak."),
    ("memory", "commit", "kB", "Total virtual memory the kernel has promised to all processes (committed address space). Can exceed RAM+swap with overcommit."),
    ("memory", "commit-percent", "%", "Committed memory relative to RAM+swap. Values well above 100 mean the system relies on overcommit; an OOM kill becomes possible if promises are called in."),
    ("memory", "active", "kB", "Recently used pages the kernel prefers to keep. With MGLRU kernels the active/inactive split is generational rather than two lists."),
    ("memory", "inactive", "kB", "Pages not referenced recently; first candidates for reclaim. A large inactive pool is normal on file-heavy servers."),
    ("memory", "dirty", "kB", "Page cache waiting to be written back to disk. Sustained growth means writeback cannot keep up with the write rate."),
    ("memory", "anonpg", "kB", "Anonymous pages (process heaps/stacks, not file-backed). Steady unexplained growth here is what an actual memory leak looks like."),
    ("memory", "slab", "kB", "Kernel object caches (dentries, inodes, network buffers). Much of it is reclaimable; check /proc/meminfo SReclaimable vs SUnreclaim before calling it a leak."),
    ("memory", "kstack", "kB", "Kernel stacks of all threads. Scales with thread count."),
    ("memory", "pgtbl", "kB", "Page table memory. Grows with many processes mapping large address spaces; huge pages reduce it."),
    ("memory", "vmused", "kB", "Used virtual address space (vmalloc)."),
    ("memory", "swpfree", "kB", "Free swap space."),
    ("memory", "swpused", "kB", "Swap space in use. Static moderate usage of cold pages is fine; the harmful signal is continuous swap in/out traffic (see pswpin/pswpout)."),
    ("memory", "swpused-percent", "%", "Swap used as a percentage of swap size."),
    ("memory", "swpcad", "kB", "Swap cache: pages that exist both in RAM and swap, cheap to evict again."),
    ("memory", "swpcad-percent", "%", "Swap cache relative to used swap."),
    // ---- Paging ----
    ("paging", "pgpgin", "kB/s", "Data paged in from block devices (all file/disk reads through the page cache, not just swap). Baseline load indicator for reads."),
    ("paging", "pgpgout", "kB/s", "Data paged out to block devices (writeback of dirty pages)."),
    ("paging", "fault", "/s", "Page faults, minor + major. Minor faults are cheap and ubiquitous; watch majflt for the expensive kind."),
    ("paging", "majflt", "/s", "Major faults: the page had to be read from disk (file mmap or swap-in). Sustained majflt means the working set no longer fits in RAM or cold code paths are being loaded."),
    ("paging", "pgfree", "/s", "Pages placed on the free list per second."),
    ("paging", "pgscank", "/s", "Pages scanned by kswapd (background reclaim). Nonzero is normal under load; explosive growth means memory pressure."),
    ("paging", "pgscand", "/s", "Pages scanned directly by allocating processes (direct reclaim). Any sustained value means allocations are stalling to find memory - a stronger pressure signal than pgscank."),
    ("paging", "pgsteal", "/s", "Pages actually reclaimed per second."),
    ("paging", "vmeff-percent", "%", "Reclaim efficiency: pgsteal/pgscan. Near 100 means scans find reclaimable pages easily; low values mean the kernel is scanning hard for little gain (thrashing risk)."),
    // ---- Swapping ----
    ("swap-pages", "pswpin", "/s", "Pages swapped in per second. Nonzero means previously swapped memory is being touched again."),
    ("swap-pages", "pswpout", "/s", "Pages swapped out per second. Sustained pswpout while pswpin is also active is real memory thrashing."),
    // ---- I/O totals ----
    ("io", "tps", "/s", "Transfers (I/O requests) per second issued to all physical devices."),
    ("io", "rtps", "/s", "Read requests per second."),
    ("io", "wtps", "/s", "Write requests per second."),
    ("io", "dtps", "/s", "Discard (TRIM) requests per second."),
    ("io", "bread", "blk/s", "Blocks read per second (512-byte blocks)."),
    ("io", "bwrtn", "blk/s", "Blocks written per second (512-byte blocks)."),
    ("io", "bdscd", "blk/s", "Blocks discarded per second."),
    // ---- Per-disk ----
    ("disk", "tps", "/s", "I/O requests per second for this device (after merging)."),
    ("disk", "rkB", "kB/s", "Data read from this device."),
    ("disk", "wkB", "kB/s", "Data written to this device."),
    ("disk", "dkB", "kB/s", "Data discarded on this device."),
    ("disk", "rd_sec", "sect/s", "Sectors read per second (older sysstat naming)."),
    ("disk", "wr_sec", "sect/s", "Sectors written per second (older sysstat naming)."),
    ("disk", "areq-sz", "kB", "Average request size. Small requests with high await point at random I/O; large requests at streaming."),
    ("disk", "aqu-sz", "", "Average queue length (in-flight + queued). Rule of thumb: consistently above device parallelism means saturation."),
    ("disk", "await", "ms", "Average time a request spends from submission to completion, queueing included. The primary latency signal; compare reads vs writes with iostat -x when it spikes."),
    ("disk", "util-percent", "%", "Fraction of time the device had at least one request in flight. 100 means always busy - which for NVMe/SSD arrays with internal parallelism is NOT automatically saturation; combine with await and aqu-sz."),
    // ---- Network ----
    ("net-dev", "rxpck", "/s", "Packets received per second."),
    ("net-dev", "txpck", "/s", "Packets transmitted per second."),
    ("net-dev", "rxkB", "kB/s", "Bytes received per second (in kB)."),
    ("net-dev", "txkB", "kB/s", "Bytes transmitted per second (in kB)."),
    ("net-dev", "rxcmp", "/s", "Compressed packets received (PPP/SLIP era; usually 0)."),
    ("net-dev", "txcmp", "/s", "Compressed packets transmitted."),
    ("net-dev", "rxmcst", "/s", "Multicast packets received."),
    ("net-dev", "ifutil-percent", "%", "Interface utilization vs its negotiated speed. Meaningless (0) on virtual interfaces without a speed."),
    ("net-edev", "rxerr", "/s", "Bad packets received. Any sustained nonzero value deserves attention (cabling, NIC, driver)."),
    ("net-edev", "txerr", "/s", "Transmit errors per second."),
    ("net-edev", "coll", "/s", "Collisions per second (half-duplex legacy; should be 0)."),
    ("net-edev", "rxdrop", "/s", "Received packets dropped for lack of kernel buffer space. Points at RX ring or softirq backlog pressure."),
    ("net-edev", "txdrop", "/s", "Transmit packets dropped."),
    ("net-edev", "txcarr", "/s", "Carrier errors on transmit (link flaps)."),
    ("net-edev", "rxfram", "/s", "Frame alignment errors on receive."),
    ("net-edev", "rxfifo", "/s", "RX FIFO overruns: the NIC had data the host did not drain in time."),
    ("net-edev", "txfifo", "/s", "TX FIFO errors."),
    ("softnet", "total", "/s", "Packets processed by the softnet (NAPI) layer per second, per CPU."),
    ("softnet", "dropd", "/s", "Packets dropped because the per-CPU backlog queue was full. Increase net.core.netdev_max_backlog or spread IRQs if sustained."),
    ("softnet", "squeezd", "/s", "Times NAPI stopped early because its budget ran out while work remained. Frequent squeezing means CPUs cannot keep up with packet rate."),
    ("softnet", "rx_rps", "/s", "Inter-CPU wakeups for Receive Packet Steering."),
    ("softnet", "flw_lim", "/s", "Times the RPS flow limit was hit."),
    // ---- Load / tasks ----
    ("queue", "runq-sz", "", "Runnable tasks waiting for a CPU (plus running). Consistently above CPU count means CPU contention."),
    ("queue", "plist-sz", "", "Total number of tasks (processes + threads) in the process list."),
    ("queue", "ldavg-1", "", "1-minute load average: runnable + uninterruptible-sleep tasks, exponentially averaged. Compare against CPU count; disk-blocked tasks inflate it too."),
    ("queue", "ldavg-5", "", "5-minute load average."),
    ("queue", "ldavg-15", "", "15-minute load average. Slow-moving trend; useful for capacity plots."),
    ("queue", "blocked", "", "Tasks currently blocked in uninterruptible (D-state) sleep, usually waiting on I/O. Spikes here mean storage stalls."),
    ("process-and-context-switch", "proc", "/s", "New processes (fork/clone of thread group leaders) created per second."),
    ("process-and-context-switch", "cswch", "/s", "Context switches per second. High absolute numbers are normal on busy boxes; a step change without a traffic change hints at lock contention or interrupt storms."),
    // ---- Kernel tables / inodes ----
    ("kernel", "dentunusd", "", "Unused-but-cached directory entries in the dentry cache (reclaimable)."),
    ("kernel", "file-nr", "", "Open file handles system-wide."),
    ("kernel", "inode-nr", "", "In-memory inodes."),
    ("kernel", "pty-nr", "", "Pseudo-terminals in use (interactive/ssh sessions)."),
    // ---- Hugepages ----
    ("hugepages", "hugfree", "kB", "Free static hugepage memory."),
    ("hugepages", "hugused", "kB", "Used static hugepage memory."),
    ("hugepages", "hugused-percent", "%", "Used share of the static hugepage pool."),
    ("hugepages", "hugrsvd", "kB", "Reserved (promised but not yet faulted) hugepage memory."),
    ("hugepages", "hugsurp", "kB", "Surplus hugepages allocated beyond the static pool."),
    // ---- Filesystems ----
    ("filesystems", "MBfsfree", "MB", "Free space on this filesystem."),
    ("filesystems", "MBfsused", "MB", "Used space on this filesystem."),
    ("filesystems", "fsused-percent", "%", "Used space as seen by root (includes the reserved blocks)."),
    ("filesystems", "ufsused-percent", "%", "Used space as seen by unprivileged users."),
    ("filesystems", "Ifree", "", "Free inodes."),
    ("filesystems", "Iused", "", "Used inodes."),
    ("filesystems", "iused-percent", "%", "Used inode percentage. Filesystems can be 'full' on inodes with plenty of bytes free (many small files)."),
    // ---- PSI (pressure stall information) ----
    ("psi-cpu", "some-avg10", "%", "Share of the last 10s in which at least one task stalled waiting for CPU. The modern, direct 'is anything waiting' signal."),
    ("psi-cpu", "some-avg60", "%", "CPU pressure over the last 60s."),
    ("psi-cpu", "some-avg300", "%", "CPU pressure over the last 5 minutes."),
    ("psi-io", "some-avg10", "%", "Share of the last 10s where tasks stalled on I/O. Rises before iowait becomes obvious on multi-core systems."),
    ("psi-io", "full-avg10", "%", "Share of the last 10s where ALL non-idle tasks stalled on I/O simultaneously (whole-system stall)."),
    ("psi-mem", "some-avg10", "%", "Share of the last 10s where tasks stalled on memory (reclaim, swap-in, thrashing)."),
    ("psi-mem", "full-avg10", "%", "Whole-system memory stalls in the last 10s. Any sustained value is serious."),
    // ---- Interrupts ----
    ("interrupts", "value", "/s", "Interrupts per second for this source (or the sum across sources for the 'sum' row)."),
    // ---- Serial ----
    ("serial", "rcvin", "/s", "Serial line receive interrupts per second."),
    ("serial", "xmtin", "/s", "Serial line transmit interrupts per second."),
];

/// Look up the description for a series id.
pub fn describe(id: &str) -> Option<(&'static str, &'static str)> {
    let segs = segments(id);
    if segs.is_empty() {
        return None;
    }
    let group = strip_instance(&segs[0]);
    let leaf = strip_instance(segs.last().unwrap());
    // Exact group match first, then wildcard by leaf.
    ENTRIES
        .iter()
        .find(|(g, l, _, _)| *g == group && *l == leaf)
        .or_else(|| ENTRIES.iter().find(|(_, l, _, _)| *l == leaf))
        .map(|(_, _, u, d)| (*u, *d))
}

/// Unit hint for a series id (used as a readout suffix).
pub fn unit(id: &str) -> &'static str {
    if let Some((u, _)) = describe(id) {
        return u;
    }
    let leaf = segments(id).pop().unwrap_or_default();
    if leaf.contains("percent") {
        "%"
    } else if leaf.ends_with("kB") || leaf.starts_with("kb") {
        "kB/s"
    } else if leaf.ends_with("pck") || leaf.ends_with("/s") {
        "/s"
    } else {
        ""
    }
}

fn strip_instance(seg: &str) -> String {
    match seg.find('[') {
        Some(i) => seg[..i].to_string(),
        None => seg.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        let (u, d) = describe("cpu-load[all].iowait").unwrap();
        assert_eq!(u, "%");
        assert!(d.contains("idle"));
        let (u2, _) = describe("disk[sda].await").unwrap();
        assert_eq!(u2, "ms");
        assert!(describe("memory.cached").is_some());
        assert!(describe("network.net-dev[eth0].rxkB").is_some());
        assert_eq!(unit("memory.memused-percent"), "%");
    }
}
