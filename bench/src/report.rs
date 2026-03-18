//! Pretty-print benchmark results to stdout in the canonical Blazil format.
//!
//! ANSI colors are applied only when stdout is a real TTY; plain text is
//! emitted for CI pipes so logs stay readable.

use std::io::IsTerminal as _;

use crate::metrics::BenchmarkResult;

// ── color palette ─────────────────────────────────────────────────────────────

struct Colors {
    header: &'static str,  // bold blue   — section headers / dividers
    tps: &'static str,     // bold green  — TPS numbers
    latency: &'static str, // bold cyan   — latency numbers
    label: &'static str,   // white       — scenario labels
    context: &'static str, // yellow      — context comparisons
    reset: &'static str,
}

impl Colors {
    fn new() -> Self {
        if std::io::stdout().is_terminal() {
            Colors {
                header: "\x1b[1;34m",
                tps: "\x1b[1;32m",
                latency: "\x1b[1;36m",
                label: "\x1b[0;37m",
                context: "\x1b[0;33m",
                reset: "\x1b[0m",
            }
        } else {
            Colors {
                header: "",
                tps: "",
                latency: "",
                label: "",
                context: "",
                reset: "",
            }
        }
    }
}

// ── public API ────────────────────────────────────────────────────────────────

/// Print the full benchmark report to stdout.
///
/// `tb_result` is `None` when the TigerBeetle scenario was skipped.
pub fn print_report(
    ring: &BenchmarkResult,
    pipeline: &BenchmarkResult,
    sharded_1: &BenchmarkResult,
    sharded_4: &BenchmarkResult,
    tcp: &BenchmarkResult,
    tb: Option<&BenchmarkResult>,
) {
    let c = Colors::new();
    let sep = "━".repeat(54);
    let rule = "─".repeat(53);

    let cpu = get_cpu_info();
    let os = std::env::consts::OS.to_owned();
    let rust = get_rust_version();
    let date = get_date();

    println!("\n{h}{sep}{r}", h = c.header, r = c.reset);
    println!("{h} BLAZIL BENCHMARK RESULTS{r}", h = c.header, r = c.reset);
    println!("{h} Hardware: {cpu}{r}", h = c.header, r = c.reset);
    println!("{h} OS: {os}{r}", h = c.header, r = c.reset);
    println!("{h} Rust: {rust}{r}", h = c.header, r = c.reset);
    println!("{h} Date: {date}{r}", h = c.header, r = c.reset);
    println!("{h}{sep}{r}\n", h = c.header, r = c.reset);

    // ── summary table ────────────────────────────────────────────────────────
    println!(
        " {h}{:<28}{r} {h}{:>12}{r}  {h}{:>15}{r}",
        "Scenario",
        "TPS",
        "P99 Latency",
        h = c.header,
        r = c.reset,
    );
    println!("{h} {rule}{r}", h = c.header, r = c.reset);

    print_row(&c, "Ring Buffer (raw)", ring, "ns");
    print_row(&c, "Pipeline (in-memory)", pipeline, "ns");
    print_row(&c, "Sharded (1 shard)", sharded_1, "ns");
    print_row(&c, "Sharded (4 shards)", sharded_4, "ns");
    print_tcp_row(&c, "End-to-End TCP", tcp);

    if let Some(tb) = tb {
        println!(
            " {lc}{:<28}{r} {tc}{:>12}{r}  {lc}{:>12} µs{r}",
            "TigerBeetle (real)*",
            fmt_commas(tb.tps),
            fmt_commas(tb.p99_ns / 1_000),
            lc = c.label,
            tc = c.tps,
            r = c.reset,
        );
    } else {
        println!(
            " {lc}{:<28}{r} {tc}{:>12}{r}  {lc}{:>15}{r}",
            "TigerBeetle (real)*",
            "SKIPPED",
            "—",
            lc = c.label,
            tc = c.tps,
            r = c.reset,
        );
    }

    println!();
    println!(" * Requires BLAZIL_TB_ADDRESS — skipped if not set");

    // ── detailed latency for pipeline ────────────────────────────────────────
    println!();
    println!(
        "{h} Detailed latency (Pipeline in-memory):{r}",
        h = c.header,
        r = c.reset
    );
    println!(
        "   P50:   {lc}{} ns{r}",
        fmt_commas(pipeline.p50_ns),
        lc = c.latency,
        r = c.reset
    );
    println!(
        "   P95:   {lc}{} ns{r}",
        fmt_commas(pipeline.p95_ns),
        lc = c.latency,
        r = c.reset
    );
    println!(
        "   P99:   {lc}{} ns{r}",
        fmt_commas(pipeline.p99_ns),
        lc = c.latency,
        r = c.reset
    );
    println!(
        "   P99.9: {lc}{} ns{r}",
        fmt_commas(pipeline.p99_9_ns),
        lc = c.latency,
        r = c.reset
    );

    // ── sharded scaling analysis ─────────────────────────────────────────────
    let scaling_ratio = sharded_4.tps as f64 / sharded_1.tps as f64;
    let scaling_efficiency = (scaling_ratio / 4.0) * 100.0;
    
    println!();
    println!(
        "{h} Sharded Pipeline Scaling:{r}",
        h = c.header,
        r = c.reset
    );
    println!(
        "   1 shard:  {tc}{} TPS{r}",
        fmt_commas(sharded_1.tps),
        tc = c.tps,
        r = c.reset
    );
    println!(
        "   4 shards: {tc}{} TPS{r}",
        fmt_commas(sharded_4.tps),
        tc = c.tps,
        r = c.reset
    );
    println!(
        "   Scaling:  {lc}{:.2}x (efficiency: {:.1}%){r}",
        scaling_ratio,
        scaling_efficiency,
        lc = c.latency,
        r = c.reset
    );

    // ── context ──────────────────────────────────────────────────────────────
    println!("\n{h}{sep}{r}", h = c.header, r = c.reset);
    println!(" Context:");
    println!(
        "   {cx}Visa peak:        ~24,000 TPS{r}",
        cx = c.context,
        r = c.reset
    );
    println!(
        "   {cx}NASDAQ:       ~2,000,000 TPS{r}",
        cx = c.context,
        r = c.reset
    );
    println!(
        "   {cx}Blazil target: 10,000,000 TPS (multi-node){r}",
        cx = c.context,
        r = c.reset
    );
    println!("{h}{sep}{r}\n", h = c.header, r = c.reset);
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn print_row(c: &Colors, label: &str, r: &BenchmarkResult, unit: &str) {
    println!(
        " {lc}{:<28}{r} {tc}{:>12}{r}  {lc}{:>12} {unit}{r}",
        label,
        fmt_commas(r.tps),
        fmt_commas(r.p99_ns),
        lc = c.label,
        tc = c.tps,
        r = c.reset,
    );
}

fn print_tcp_row(c: &Colors, label: &str, r: &BenchmarkResult) {
    println!(
        " {lc}{:<28}{r} {tc}{:>12}{r}  {lc}{:>12} µs{r}",
        label,
        fmt_commas(r.tps),
        fmt_commas(r.p99_ns / 1_000),
        lc = c.label,
        tc = c.tps,
        r = c.reset,
    );
}

/// Format a `u64` with comma separators: 1234567 → "1,234,567".
pub fn fmt_commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn get_cpu_info() -> String {
    #[cfg(target_arch = "aarch64")]
    {
        "Apple Silicon (ARM64)".to_owned()
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        if let Ok(id) = std::env::var("PROCESSOR_IDENTIFIER") {
            return id;
        }
        // Linux /proc/cpuinfo
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("model name") {
                    if let Some(val) = line.split_once(':').map(|x| x.1) {
                        return val.trim().to_owned();
                    }
                }
            }
        }
        "x86_64 (unknown)".to_owned()
    }
}

fn get_rust_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned())
}

fn get_date() -> String {
    std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned())
}
