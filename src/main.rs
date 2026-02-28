use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::process::Command;

// Pane metadata returned by tmux.
#[derive(Debug, Clone)]
struct PaneInfo {
    target: String,
    window_name: String,
    pane_pid: i32,
    history_size: i64,
    history_limit: i64,
}

// One output row in the final report.
#[derive(Debug, Clone)]
struct ProcRecord {
    pid: i32,
    command: String,
    swap_bytes: u64,
    physical_bytes: u64,
    rss_bytes: u64,
    tmux_target: String,
    tmux_window_name: String,
    pane_history_size: i64,
    pane_history_limit: i64,
    pane_history_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Table,
    Json,
    Csv,
    Yaml,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchMode {
    Exact,
    Full,
}

#[derive(Debug, Clone)]
struct Cli {
    process_pattern: String,
    match_mode: MatchMode,
    view_mode: ViewMode,
    stdout_format: OutputFormat,
    export_path: Option<String>,
    export_format: Option<OutputFormat>,
    no_history_bytes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Process,
    Pane,
}

#[derive(Debug, Clone)]
struct PaneRecord {
    tmux_target: String,
    tmux_window_name: String,
    process_count: usize,
    pids: Vec<i32>,
    swap_bytes: u64,
    physical_bytes: u64,
    rss_bytes: u64,
    pane_history_size: i64,
    pane_history_limit: i64,
    pane_history_bytes: u64,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = parse_cli()?;

    // We keep this resilient: if tmux is not available, we still report process memory.
    let panes = match list_tmux_panes() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("warning: tmux panes unavailable: {e}");
            Vec::new()
        }
    };
    let pane_by_pid = panes
        .iter()
        .map(|p| (p.pane_pid, p.clone()))
        .collect::<HashMap<_, _>>();

    let pids = pgrep_pattern(&cli.process_pattern, cli.match_mode)
        .map_err(|e| format!("failed to discover processes: {e}"))?;

    let mut ppid_cache = HashMap::<i32, i32>::new();
    let mut history_cache = HashMap::<String, u64>::new();

    let mut rows = Vec::<ProcRecord>::new();
    for pid in pids {
        let command = ps_command(pid).unwrap_or_else(|_| "<unavailable>".to_string());
        let rss_bytes = ps_rss_bytes(pid).unwrap_or(0);
        let (swap_bytes, physical_bytes) = vmmap_memory(pid).unwrap_or((0, 0));

        let owner = find_owning_pane(pid, &pane_by_pid, &mut ppid_cache);
        let (
            tmux_target,
            tmux_window_name,
            pane_history_size,
            pane_history_limit,
            pane_history_bytes,
        ) = if let Some(pane) = owner {
            let history_bytes = if cli.no_history_bytes {
                0
            } else if let Some(v) = history_cache.get(&pane.target) {
                *v
            } else {
                let measured = capture_pane_bytes(&pane.target).unwrap_or(0);
                history_cache.insert(pane.target.clone(), measured);
                measured
            };

            (
                pane.target,
                pane.window_name,
                pane.history_size,
                pane.history_limit,
                history_bytes,
            )
        } else {
            ("?".to_string(), "?".to_string(), -1, -1, 0)
        };

        rows.push(ProcRecord {
            pid,
            command,
            swap_bytes,
            physical_bytes,
            rss_bytes,
            tmux_target,
            tmux_window_name,
            pane_history_size,
            pane_history_limit,
            pane_history_bytes,
        });
    }

    // Sort most memory-hungry first.
    rows.sort_by(|a, b| {
        b.swap_bytes
            .cmp(&a.swap_bytes)
            .then_with(|| b.physical_bytes.cmp(&a.physical_bytes))
            .then_with(|| a.pid.cmp(&b.pid))
    });

    let panes = aggregate_by_pane(&rows);

    let output = match cli.view_mode {
        ViewMode::Process => render_process(&rows, cli.stdout_format),
        ViewMode::Pane => render_pane(&panes, cli.stdout_format),
    };
    print!("{output}");

    if let Some(path) = cli.export_path {
        let fmt = cli
            .export_format
            .or_else(|| infer_format_from_path(&path))
            .unwrap_or(OutputFormat::Json);
        let body = match cli.view_mode {
            ViewMode::Process => render_process(&rows, fmt),
            ViewMode::Pane => render_pane(&panes, fmt),
        };
        fs::write(&path, body).map_err(|e| format!("failed writing export file '{path}': {e}"))?;
        let count = match cli.view_mode {
            ViewMode::Process => rows.len(),
            ViewMode::Pane => panes.len(),
        };
        eprintln!("exported {} records to {}", count, path);
    }

    Ok(())
}

fn parse_cli() -> Result<Cli, String> {
    // Intentionally no external CLI crate: tiny binary, tiny dependency surface.
    let mut process_pattern = "opencode".to_string();
    let mut match_mode = MatchMode::Exact;
    let mut view_mode = ViewMode::Process;
    let mut stdout_format = OutputFormat::Table;
    let mut export_path: Option<String> = None;
    let mut export_format: Option<OutputFormat> = None;
    let mut no_history_bytes = false;

    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut i = 0usize;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--process" => {
                i += 1;
                process_pattern = args.get(i).ok_or("--process requires a value")?.to_string();
            }
            "--match-mode" => {
                i += 1;
                let v = args.get(i).ok_or("--match-mode requires a value")?;
                match_mode = match v.to_ascii_lowercase().as_str() {
                    "exact" => MatchMode::Exact,
                    "full" => MatchMode::Full,
                    _ => return Err(format!("unsupported --match-mode value: {v}")),
                };
            }
            "--format" => {
                i += 1;
                let v = args.get(i).ok_or("--format requires a value")?;
                stdout_format = parse_format(v)?;
            }
            "--view" => {
                i += 1;
                let v = args.get(i).ok_or("--view requires a value")?;
                view_mode = parse_view_mode(v)?;
            }
            "--export" => {
                i += 1;
                export_path = Some(
                    args.get(i)
                        .ok_or("--export requires a file path")?
                        .to_string(),
                );
            }
            "--export-format" => {
                i += 1;
                let v = args.get(i).ok_or("--export-format requires a value")?;
                export_format = Some(parse_format(v)?);
            }
            "--no-history-bytes" => no_history_bytes = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}\n\nUse --help for usage.")),
        }
        i += 1;
    }

    Ok(Cli {
        process_pattern,
        match_mode,
        view_mode,
        stdout_format,
        export_path,
        export_format,
        no_history_bytes,
    })
}

fn print_help() {
    println!("opencode-tmux-mem 🔍");
    println!();
    println!("Usage:");
    println!("  opencode-tmux-mem [options]");
    println!();
    println!("Options:");
    println!("  --process <pattern>         Process pattern (default: opencode)");
    println!("  --match-mode <exact|full>   PID scan mode (default: exact)");
    println!("  --view <process|pane>       Output view mode (default: process)");
    println!("  --format <fmt>              table|json|csv|yaml|markdown (default: table)");
    println!("  --export <path>             Export to file");
    println!("  --export-format <fmt>       Export format override");
    println!("  --no-history-bytes          Skip tmux capture-pane byte estimation");
    println!("  -h, --help                  Show help");
}

fn parse_format(v: &str) -> Result<OutputFormat, String> {
    match v.to_ascii_lowercase().as_str() {
        "table" => Ok(OutputFormat::Table),
        "json" => Ok(OutputFormat::Json),
        "csv" => Ok(OutputFormat::Csv),
        "yaml" | "yml" => Ok(OutputFormat::Yaml),
        "markdown" | "md" => Ok(OutputFormat::Markdown),
        _ => Err(format!("unsupported format: {v}")),
    }
}

fn parse_view_mode(v: &str) -> Result<ViewMode, String> {
    match v.to_ascii_lowercase().as_str() {
        "process" => Ok(ViewMode::Process),
        "pane" => Ok(ViewMode::Pane),
        _ => Err(format!("unsupported view mode: {v}")),
    }
}

fn infer_format_from_path(path: &str) -> Option<OutputFormat> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".json") {
        Some(OutputFormat::Json)
    } else if lower.ends_with(".csv") {
        Some(OutputFormat::Csv)
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        Some(OutputFormat::Yaml)
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") {
        Some(OutputFormat::Markdown)
    } else {
        None
    }
}

fn run_cmd(program: &str, args: &[&str]) -> io::Result<String> {
    let out = Command::new(program).args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "command failed: {} {} => {}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn list_tmux_panes() -> io::Result<Vec<PaneInfo>> {
    let raw = run_cmd(
        "tmux",
        &[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}:#{window_index}.#{pane_index}\t#{window_name}\t#{pane_pid}\t#{history_size}\t#{history_limit}",
        ],
    )?;

    let mut panes = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split('\t');
        let target = parts.next().unwrap_or("").to_string();
        let window_name = parts.next().unwrap_or("").to_string();
        let pane_pid = parts
            .next()
            .unwrap_or("0")
            .trim()
            .parse::<i32>()
            .unwrap_or(0);
        let history_size = parts
            .next()
            .unwrap_or("0")
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        let history_limit = parts
            .next()
            .unwrap_or("0")
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        if !target.is_empty() && pane_pid > 0 {
            panes.push(PaneInfo {
                target,
                window_name,
                pane_pid,
                history_size,
                history_limit,
            });
        }
    }
    Ok(panes)
}

fn pgrep_pattern(pattern: &str, match_mode: MatchMode) -> io::Result<Vec<i32>> {
    let args = match match_mode {
        MatchMode::Exact => vec!["-x", pattern],
        MatchMode::Full => vec!["-f", pattern],
    };
    let out = Command::new("pgrep").args(args).output()?;
    if !out.status.success() && out.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    let mut pids = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Ok(pid) = line.trim().parse::<i32>() {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

fn ps_command(pid: i32) -> io::Result<String> {
    Ok(run_cmd("ps", &["-p", &pid.to_string(), "-o", "command="])?
        .trim()
        .to_string())
}

fn ps_rss_bytes(pid: i32) -> io::Result<u64> {
    let raw = run_cmd("ps", &["-p", &pid.to_string(), "-o", "rss="])?;
    let kb = raw.trim().parse::<u64>().unwrap_or(0);
    Ok(kb.saturating_mul(1024))
}

fn ps_ppid(pid: i32, cache: &mut HashMap<i32, i32>) -> i32 {
    if let Some(v) = cache.get(&pid) {
        return *v;
    }
    let ppid = run_cmd("ps", &["-p", &pid.to_string(), "-o", "ppid="])
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    cache.insert(pid, ppid);
    ppid
}

fn find_owning_pane(
    pid: i32,
    pane_by_pid: &HashMap<i32, PaneInfo>,
    ppid_cache: &mut HashMap<i32, i32>,
) -> Option<PaneInfo> {
    // Follow parent PID chain until we hit a pane root PID.
    let mut cur = pid;
    let mut guard = 0usize;
    while cur > 0 && guard < 512 {
        if let Some(pane) = pane_by_pid.get(&cur) {
            return Some(pane.clone());
        }
        cur = ps_ppid(cur, ppid_cache);
        guard += 1;
    }
    None
}

fn vmmap_memory(pid: i32) -> io::Result<(u64, u64)> {
    let raw = run_cmd("vmmap", &["-summary", &pid.to_string()])?;
    let mut swap_bytes = 0u64;
    let mut physical_bytes = 0u64;

    for line in raw.lines() {
        let t = line.trim_start();
        if t.starts_with("Physical footprint:") {
            if let Some(v) = t.split(':').nth(1) {
                let tok = v.split_whitespace().next().unwrap_or("0B");
                physical_bytes = parse_compact_bytes(tok);
            }
        }
        if t.starts_with("TOTAL") && !t.contains("minus reserved") {
            let cols = t.split_whitespace().collect::<Vec<_>>();
            if cols.len() >= 5 {
                swap_bytes = parse_compact_bytes(cols[4]);
            }
            break;
        }
    }

    Ok((swap_bytes, physical_bytes))
}

fn capture_pane_bytes(target: &str) -> io::Result<u64> {
    let out = run_cmd(
        "tmux",
        &["capture-pane", "-p", "-S", "-", "-E", "-", "-t", target],
    )?;
    Ok(out.len() as u64)
}

fn parse_compact_bytes(token: &str) -> u64 {
    let t = token.trim();
    if t.is_empty() {
        return 0;
    }
    if let Ok(v) = t.parse::<u64>() {
        return v;
    }

    let (num, unit) = t.split_at(t.len().saturating_sub(1));
    let n = num.parse::<f64>().unwrap_or(0.0);
    let m = match unit.to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (n * m) as u64
}

fn human_bytes(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = bytes as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, units[i])
    } else {
        format!("{v:.2} {}", units[i])
    }
}

fn render_process(rows: &[ProcRecord], fmt: OutputFormat) -> String {
    match fmt {
        OutputFormat::Table => render_table(rows),
        OutputFormat::Json => render_json(rows),
        OutputFormat::Csv => render_csv(rows),
        OutputFormat::Yaml => render_yaml(rows),
        OutputFormat::Markdown => render_markdown(rows),
    }
}

fn render_pane(rows: &[PaneRecord], fmt: OutputFormat) -> String {
    match fmt {
        OutputFormat::Table => render_pane_table(rows),
        OutputFormat::Json => render_pane_json(rows),
        OutputFormat::Csv => render_pane_csv(rows),
        OutputFormat::Yaml => render_pane_yaml(rows),
        OutputFormat::Markdown => render_pane_markdown(rows),
    }
}

fn aggregate_by_pane(rows: &[ProcRecord]) -> Vec<PaneRecord> {
    let mut by_pane = HashMap::<String, PaneRecord>::new();
    for row in rows {
        let entry = by_pane
            .entry(row.tmux_target.clone())
            .or_insert_with(|| PaneRecord {
                tmux_target: row.tmux_target.clone(),
                tmux_window_name: row.tmux_window_name.clone(),
                process_count: 0,
                pids: Vec::new(),
                swap_bytes: 0,
                physical_bytes: 0,
                rss_bytes: 0,
                pane_history_size: row.pane_history_size,
                pane_history_limit: row.pane_history_limit,
                pane_history_bytes: row.pane_history_bytes,
            });

        entry.process_count += 1;
        entry.pids.push(row.pid);
        entry.swap_bytes = entry.swap_bytes.saturating_add(row.swap_bytes);
        entry.physical_bytes = entry.physical_bytes.saturating_add(row.physical_bytes);
        entry.rss_bytes = entry.rss_bytes.saturating_add(row.rss_bytes);

        // Keep the richest pane history metadata and avoid double-counting bytes.
        if row.pane_history_size > entry.pane_history_size {
            entry.pane_history_size = row.pane_history_size;
        }
        if row.pane_history_limit > entry.pane_history_limit {
            entry.pane_history_limit = row.pane_history_limit;
        }
        if row.pane_history_bytes > entry.pane_history_bytes {
            entry.pane_history_bytes = row.pane_history_bytes;
        }
    }

    let mut pane_rows = by_pane.into_values().collect::<Vec<_>>();
    pane_rows.sort_by(|a, b| {
        b.swap_bytes
            .cmp(&a.swap_bytes)
            .then_with(|| b.physical_bytes.cmp(&a.physical_bytes))
            .then_with(|| a.tmux_target.cmp(&b.tmux_target))
    });
    pane_rows
}

fn render_table(rows: &[ProcRecord]) -> String {
    let mut out = String::new();
    out.push_str(
        "PID\tTmux window.pane\tWindow\tSwap\tPhysical\tRSS\tPaneHistory\tHistory lines\tCommand\n",
    );
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("{}/{}", row.pane_history_size, row.pane_history_limit)
        } else {
            "-".to_string()
        };
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.pid,
            row.tmux_target,
            row.tmux_window_name,
            human_bytes(row.swap_bytes),
            human_bytes(row.physical_bytes),
            human_bytes(row.rss_bytes),
            human_bytes(row.pane_history_bytes),
            history_lines,
            row.command,
        );
    }

    let total_swap = rows.iter().map(|r| r.swap_bytes).sum::<u64>();
    let total_phys = rows.iter().map(|r| r.physical_bytes).sum::<u64>();
    let total_rss = rows.iter().map(|r| r.rss_bytes).sum::<u64>();

    // Count pane history bytes once per pane to avoid duplicate totals.
    let mut unique_hist = HashMap::<String, u64>::new();
    for row in rows {
        unique_hist
            .entry(row.tmux_target.clone())
            .and_modify(|v| *v = (*v).max(row.pane_history_bytes))
            .or_insert(row.pane_history_bytes);
    }
    let total_hist = unique_hist.values().sum::<u64>();

    out.push('\n');
    let _ = writeln!(out, "Total swap:\t{}", human_bytes(total_swap));
    let _ = writeln!(out, "Total physical:\t{}", human_bytes(total_phys));
    let _ = writeln!(out, "Total RSS:\t{}", human_bytes(total_rss));
    let _ = writeln!(
        out,
        "Total pane history bytes:\t{}",
        human_bytes(total_hist)
    );
    out
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn render_json(rows: &[ProcRecord]) -> String {
    let mut out = String::new();
    out.push_str("[\n");
    for (idx, row) in rows.iter().enumerate() {
        let comma = if idx + 1 == rows.len() { "" } else { "," };
        let history_lines = if row.pane_history_size >= 0 {
            format!("\"{}/{}\"", row.pane_history_size, row.pane_history_limit)
        } else {
            "null".to_string()
        };
        let _ = writeln!(
            out,
            "  {{\"pid\":{},\"tmux_target\":\"{}\",\"tmux_window\":\"{}\",\"swap_bytes\":{},\"swap_human\":\"{}\",\"physical_bytes\":{},\"physical_human\":\"{}\",\"rss_bytes\":{},\"rss_human\":\"{}\",\"pane_history_bytes\":{},\"pane_history_human\":\"{}\",\"pane_history_lines\":{},\"command\":\"{}\"}}{}",
            row.pid,
            escape_json(&row.tmux_target),
            escape_json(&row.tmux_window_name),
            row.swap_bytes,
            escape_json(&human_bytes(row.swap_bytes)),
            row.physical_bytes,
            escape_json(&human_bytes(row.physical_bytes)),
            row.rss_bytes,
            escape_json(&human_bytes(row.rss_bytes)),
            row.pane_history_bytes,
            escape_json(&human_bytes(row.pane_history_bytes)),
            history_lines,
            escape_json(&row.command),
            comma,
        );
    }
    out.push_str("]\n");
    out
}

fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn render_csv(rows: &[ProcRecord]) -> String {
    let mut out = String::new();
    out.push_str("pid,tmux_target,tmux_window,swap_bytes,swap_human,physical_bytes,physical_human,rss_bytes,rss_human,pane_history_bytes,pane_history_human,pane_history_lines,command\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("{}/{}", row.pane_history_size, row.pane_history_limit)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{}",
            row.pid,
            escape_csv(&row.tmux_target),
            escape_csv(&row.tmux_window_name),
            row.swap_bytes,
            escape_csv(&human_bytes(row.swap_bytes)),
            row.physical_bytes,
            escape_csv(&human_bytes(row.physical_bytes)),
            row.rss_bytes,
            escape_csv(&human_bytes(row.rss_bytes)),
            row.pane_history_bytes,
            escape_csv(&human_bytes(row.pane_history_bytes)),
            escape_csv(&history_lines),
            escape_csv(&row.command),
        );
    }
    out
}

fn render_yaml(rows: &[ProcRecord]) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("\"{}/{}\"", row.pane_history_size, row.pane_history_limit)
        } else {
            "null".to_string()
        };
        let _ = writeln!(
            out,
            "- pid: {}\n  tmux_target: \"{}\"\n  tmux_window: \"{}\"\n  swap_bytes: {}\n  swap_human: \"{}\"\n  physical_bytes: {}\n  physical_human: \"{}\"\n  rss_bytes: {}\n  rss_human: \"{}\"\n  pane_history_bytes: {}\n  pane_history_human: \"{}\"\n  pane_history_lines: {}\n  command: \"{}\"",
            row.pid,
            row.tmux_target.replace('"', "\\\""),
            row.tmux_window_name.replace('"', "\\\""),
            row.swap_bytes,
            human_bytes(row.swap_bytes).replace('"', "\\\""),
            row.physical_bytes,
            human_bytes(row.physical_bytes).replace('"', "\\\""),
            row.rss_bytes,
            human_bytes(row.rss_bytes).replace('"', "\\\""),
            row.pane_history_bytes,
            human_bytes(row.pane_history_bytes).replace('"', "\\\""),
            history_lines,
            row.command.replace('"', "\\\""),
        );
    }
    out
}

fn render_markdown(rows: &[ProcRecord]) -> String {
    let mut out = String::new();
    out.push_str("| PID | Tmux window.pane | Window | Swap | Physical | RSS | PaneHistory | History lines | Command |\n");
    out.push_str("|---:|---|---|---:|---:|---:|---:|---:|---|\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("{}/{}", row.pane_history_size, row.pane_history_limit)
        } else {
            "-".to_string()
        };
        let cmd = row.command.replace('|', "\\|").replace('\n', " ");
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row.pid,
            row.tmux_target,
            row.tmux_window_name,
            human_bytes(row.swap_bytes),
            human_bytes(row.physical_bytes),
            human_bytes(row.rss_bytes),
            human_bytes(row.pane_history_bytes),
            history_lines,
            cmd,
        );
    }
    out
}

fn render_pane_table(rows: &[PaneRecord]) -> String {
    let mut out = String::new();
    out.push_str("Tmux window.pane\tWindow\tProcesses\tPIDs\tSwap\tPhysical\tRSS\tPaneHistory\tHistory lines\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("{}/{}", row.pane_history_size, row.pane_history_limit)
        } else {
            "-".to_string()
        };
        let pids = row
            .pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.tmux_target,
            row.tmux_window_name,
            row.process_count,
            pids,
            human_bytes(row.swap_bytes),
            human_bytes(row.physical_bytes),
            human_bytes(row.rss_bytes),
            human_bytes(row.pane_history_bytes),
            history_lines,
        );
    }

    let total_swap = rows.iter().map(|r| r.swap_bytes).sum::<u64>();
    let total_phys = rows.iter().map(|r| r.physical_bytes).sum::<u64>();
    let total_rss = rows.iter().map(|r| r.rss_bytes).sum::<u64>();
    let total_hist = rows.iter().map(|r| r.pane_history_bytes).sum::<u64>();

    out.push('\n');
    let _ = writeln!(out, "Total swap:\t{}", human_bytes(total_swap));
    let _ = writeln!(out, "Total physical:\t{}", human_bytes(total_phys));
    let _ = writeln!(out, "Total RSS:\t{}", human_bytes(total_rss));
    let _ = writeln!(
        out,
        "Total pane history bytes:\t{}",
        human_bytes(total_hist)
    );
    out
}

fn render_pane_json(rows: &[PaneRecord]) -> String {
    let mut out = String::new();
    out.push_str("[\n");
    for (idx, row) in rows.iter().enumerate() {
        let comma = if idx + 1 == rows.len() { "" } else { "," };
        let history_lines = if row.pane_history_size >= 0 {
            format!("\"{}/{}\"", row.pane_history_size, row.pane_history_limit)
        } else {
            "null".to_string()
        };
        let pids = row
            .pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            out,
            "  {{\"tmux_target\":\"{}\",\"tmux_window\":\"{}\",\"process_count\":{},\"pids\":[{}],\"swap_bytes\":{},\"swap_human\":\"{}\",\"physical_bytes\":{},\"physical_human\":\"{}\",\"rss_bytes\":{},\"rss_human\":\"{}\",\"pane_history_bytes\":{},\"pane_history_human\":\"{}\",\"pane_history_lines\":{}}}{}",
            escape_json(&row.tmux_target),
            escape_json(&row.tmux_window_name),
            row.process_count,
            pids,
            row.swap_bytes,
            escape_json(&human_bytes(row.swap_bytes)),
            row.physical_bytes,
            escape_json(&human_bytes(row.physical_bytes)),
            row.rss_bytes,
            escape_json(&human_bytes(row.rss_bytes)),
            row.pane_history_bytes,
            escape_json(&human_bytes(row.pane_history_bytes)),
            history_lines,
            comma,
        );
    }
    out.push_str("]\n");
    out
}

fn render_pane_csv(rows: &[PaneRecord]) -> String {
    let mut out = String::new();
    out.push_str("tmux_target,tmux_window,process_count,pids,swap_bytes,swap_human,physical_bytes,physical_human,rss_bytes,rss_human,pane_history_bytes,pane_history_human,pane_history_lines\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("{}/{}", row.pane_history_size, row.pane_history_limit)
        } else {
            String::new()
        };
        let pids = row
            .pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{}",
            escape_csv(&row.tmux_target),
            escape_csv(&row.tmux_window_name),
            row.process_count,
            escape_csv(&pids),
            row.swap_bytes,
            escape_csv(&human_bytes(row.swap_bytes)),
            row.physical_bytes,
            escape_csv(&human_bytes(row.physical_bytes)),
            row.rss_bytes,
            escape_csv(&human_bytes(row.rss_bytes)),
            row.pane_history_bytes,
            escape_csv(&human_bytes(row.pane_history_bytes)),
            escape_csv(&history_lines),
        );
    }
    out
}

fn render_pane_yaml(rows: &[PaneRecord]) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("\"{}/{}\"", row.pane_history_size, row.pane_history_limit)
        } else {
            "null".to_string()
        };
        let pids = row
            .pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "- tmux_target: \"{}\"\n  tmux_window: \"{}\"\n  process_count: {}\n  pids: [{}]\n  swap_bytes: {}\n  swap_human: \"{}\"\n  physical_bytes: {}\n  physical_human: \"{}\"\n  rss_bytes: {}\n  rss_human: \"{}\"\n  pane_history_bytes: {}\n  pane_history_human: \"{}\"\n  pane_history_lines: {}",
            row.tmux_target.replace('"', "\\\""),
            row.tmux_window_name.replace('"', "\\\""),
            row.process_count,
            pids,
            row.swap_bytes,
            human_bytes(row.swap_bytes).replace('"', "\\\""),
            row.physical_bytes,
            human_bytes(row.physical_bytes).replace('"', "\\\""),
            row.rss_bytes,
            human_bytes(row.rss_bytes).replace('"', "\\\""),
            row.pane_history_bytes,
            human_bytes(row.pane_history_bytes).replace('"', "\\\""),
            history_lines,
        );
    }
    out
}

fn render_pane_markdown(rows: &[PaneRecord]) -> String {
    let mut out = String::new();
    out.push_str("| Tmux window.pane | Window | Processes | PIDs | Swap | Physical | RSS | PaneHistory | History lines |\n");
    out.push_str("|---|---|---:|---|---:|---:|---:|---:|---:|\n");
    for row in rows {
        let history_lines = if row.pane_history_size >= 0 {
            format!("{}/{}", row.pane_history_size, row.pane_history_limit)
        } else {
            "-".to_string()
        };
        let pids = row
            .pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row.tmux_target,
            row.tmux_window_name,
            row.process_count,
            pids,
            human_bytes(row.swap_bytes),
            human_bytes(row.physical_bytes),
            human_bytes(row.rss_bytes),
            human_bytes(row.pane_history_bytes),
            history_lines,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compact_bytes_supports_units() {
        assert_eq!(parse_compact_bytes("1024"), 1024);
        assert_eq!(parse_compact_bytes("1K"), 1024);
        assert_eq!(parse_compact_bytes("1.5M"), 1_572_864);
        assert_eq!(parse_compact_bytes("2G"), 2_147_483_648);
    }

    #[test]
    fn infer_export_format_from_extension() {
        assert_eq!(
            infer_format_from_path("report.json"),
            Some(OutputFormat::Json)
        );
        assert_eq!(
            infer_format_from_path("report.csv"),
            Some(OutputFormat::Csv)
        );
        assert_eq!(
            infer_format_from_path("report.yaml"),
            Some(OutputFormat::Yaml)
        );
        assert_eq!(
            infer_format_from_path("report.md"),
            Some(OutputFormat::Markdown)
        );
        assert_eq!(infer_format_from_path("report.txt"), None);
    }

    #[test]
    fn csv_escape_quotes_and_commas() {
        let got = escape_csv("a,\"b\"");
        assert_eq!(got, "\"a,\"\"b\"\"\"");
    }

    #[test]
    fn parse_view_mode_supports_process_and_pane() {
        assert_eq!(parse_view_mode("process"), Ok(ViewMode::Process));
        assert_eq!(parse_view_mode("pane"), Ok(ViewMode::Pane));
    }

    #[test]
    fn aggregate_by_pane_sums_process_memory() {
        let rows = vec![
            ProcRecord {
                pid: 1,
                command: "a".to_string(),
                swap_bytes: 100,
                physical_bytes: 200,
                rss_bytes: 300,
                tmux_target: "s:1.0".to_string(),
                tmux_window_name: "w".to_string(),
                pane_history_size: 10,
                pane_history_limit: 100,
                pane_history_bytes: 1000,
            },
            ProcRecord {
                pid: 2,
                command: "b".to_string(),
                swap_bytes: 50,
                physical_bytes: 70,
                rss_bytes: 90,
                tmux_target: "s:1.0".to_string(),
                tmux_window_name: "w".to_string(),
                pane_history_size: 11,
                pane_history_limit: 100,
                pane_history_bytes: 900,
            },
        ];

        let panes = aggregate_by_pane(&rows);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].process_count, 2);
        assert_eq!(panes[0].swap_bytes, 150);
        assert_eq!(panes[0].physical_bytes, 270);
        assert_eq!(panes[0].rss_bytes, 390);
        assert_eq!(panes[0].pane_history_bytes, 1000);
    }
}
