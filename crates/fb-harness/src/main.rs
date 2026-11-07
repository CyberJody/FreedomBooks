//! `fb-harness run <scenario.json> <out.json> [book dir]` applies a scenario to a fresh SQLCipher book.
//! `fb-harness compare <python.json> <rust.json> [scenario.json]` prints failures and warnings; exit 1 on any
//! failure. Pass the scenario to validate every intended outcome independently of the dump flags;
//! financial reports are checked against the scenario's request manifest, so they need the scenario.
use std::path::PathBuf;

/// A scenario or dump through the shared duplicate-key-rejecting parser; anything ambiguous is a failure (exit 1).
fn read_json(path: &str) -> serde_json::Value {
    let parsed = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|raw| fb_db::parse_unique_json(&raw).map_err(|e| e.to_string()));
    parsed.unwrap_or_else(|e| {
        println!("FAIL {path}: {e}");
        println!("MISMATCH — unreadable input; 1 failures, 0 warnings");
        std::process::exit(1);
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") if args.len() >= 4 => {
            let scenario = read_json(&args[2]);
            let dir = args.get(4).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
            std::fs::create_dir_all(&dir).unwrap();
            let book = dir.join(format!("harness-{}.fbbook", std::process::id()));
            let _ = std::fs::remove_file(&book);
            let mut runner = fb_harness::Runner::new(&scenario, &book);
            runner.run();
            let dump = runner.dump();
            std::fs::write(&args[3], serde_json::to_string_pretty(&dump).unwrap()).unwrap();
            drop(runner);
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", book.display()));
            }
        }
        Some("compare") if args.len() >= 4 => {
            let scenario = args.get(4).map(|p| read_json(p));
            let report = fb_harness::compare_scenario(scenario.as_ref(), &read_json(&args[2]), &read_json(&args[3]));
            for w in &report.warnings {
                println!("WARN {w}");
            }
            for f in &report.failures {
                println!("FAIL {f}");
            }
            let checked: Vec<String> = report.checked.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!(
                "{} — checked {}; {} failures, {} warnings, {} collation ties",
                if report.ok() { "MATCH" } else { "MISMATCH" },
                checked.join(" "),
                report.failures.len(),
                report.warnings.len(),
                report.collation_ties
            );
            std::process::exit(if report.ok() { 0 } else { 1 });
        }
        _ => {
            eprintln!("usage: fb-harness run <scenario.json> <out.json> [book-dir] | compare <python.json> <rust.json> [scenario.json]");
            std::process::exit(2);
        }
    }
}
