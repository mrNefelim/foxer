use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path, process::Command, time::SystemTime};

fn temp() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "forex-mixed-execution-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn zip_file(path: &Path, inner_name: &str, contents: &[u8]) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(inner_name, zip::write::FileOptions::default())
        .unwrap();
    zip.write_all(contents).unwrap();
    zip.finish().unwrap();
}

#[test]
fn mixed_tick_and_prepared_run_records_each_symbol_execution_mode() {
    let root = temp();
    let prepared = root.join("prepared-root");
    fs::create_dir_all(prepared.join("prepared")).unwrap();
    let mut prepared_csv = String::from(
        "timestamp_utc,timestamp_ms,bar_end_utc,open,high,low,close,quote_count,gap_before_minutes\n",
    );
    let start = forex_core::prices::utc("2025-12-01T00:00:00Z").unwrap();
    let end = forex_core::prices::utc("2026-01-03T00:00:00Z").unwrap();
    for timestamp in (start..end).step_by(60) {
        let iso = forex_core::prices::iso(timestamp).unwrap();
        prepared_csv.push_str(&format!(
            "{iso},{},{},1.100000,1.100200,1.099800,1.100100,3,0\n",
            timestamp * 1000,
            forex_core::prices::iso(timestamp + 60).unwrap()
        ));
    }
    let prepared_name = "EURUSD-M1-index-2025-12.csv";
    fs::write(prepared.join("prepared").join(prepared_name), &prepared_csv).unwrap();
    let prepared_hash = format!("{:x}", Sha256::digest(prepared_csv.as_bytes()));
    fs::write(
        prepared.join("quality-report.json"),
        json!({
            "symbol":"EURUSD", "source":"synthetic prepared fixture", "timezone":"UTC",
            "price_basis":"index", "origin":"mt5_m1_bars", "gaps":[], "missing_months":[],
            "outputs":{(prepared_name):{"sha256":prepared_hash,"bytes":prepared_csv.len()}}
        })
        .to_string(),
    )
    .unwrap();

    let ticks_root = root.join("tick-root");
    fs::create_dir_all(ticks_root.join("raw")).unwrap();
    let tick_ms = forex_core::prices::utc("2026-01-02T00:00:00Z").unwrap() * 1000;
    let raw_csv = format!("timestamp_ms,price\n{},1.10000\n", tick_ms + 1);
    let archive_name = "GBPUSD-T-2026-01.zip";
    zip_file(
        &ticks_root.join("raw").join(archive_name),
        "GBPUSD-index-202601.csv",
        raw_csv.as_bytes(),
    );
    let archive_path = ticks_root.join("raw").join(archive_name);
    let archive_hash = format!("{:x}", Sha256::digest(fs::read(&archive_path).unwrap()));
    // Tick-mode datasets still need prepared context rows to seed W/D/H4 bars.
    // These rows are context only; the selected replay stream remains the raw archive.
    let tick_context = ticks_root.join("prepared");
    fs::create_dir_all(&tick_context).unwrap();
    let tick_context_name = "GBPUSD.csv";
    let tick_context_csv = prepared_csv.replace("EURUSD", "GBPUSD");
    fs::write(tick_context.join(tick_context_name), &tick_context_csv).unwrap();
    let tick_context_hash = format!("{:x}", Sha256::digest(tick_context_csv.as_bytes()));
    fs::write(
        ticks_root.join("manifest.json"),
        json!({"files":[{"month":"2026-01","file":format!("raw/{archive_name}"),"sha256":archive_hash}]}).to_string(),
    ).unwrap();
    fs::write(
        ticks_root.join("quality-report.json"),
        json!({
            "source":"synthetic observed-tick fixture", "timezone":"UTC", "price_basis":"index", "gaps":[],
            "outputs":{(tick_context_name):{"sha256":tick_context_hash,"bytes":tick_context_csv.len()}}
        }).to_string(),
    ).unwrap();

    let mut strategy: Value =
        serde_json::from_slice(&fs::read("../strategy/fvg-bot-rules.json").unwrap()).unwrap();
    strategy["instruments"].as_array_mut().unwrap().truncate(2);
    for timeframe in ["W", "D", "H4"] {
        strategy["context"]["minimum_bars"][timeframe] = json!(1);
    }
    fs::write(root.join("rules.json"), strategy.to_string()).unwrap();
    fs::write(
        root.join("sources.json"),
        json!({
            "EURUSD":[prepared.to_str().unwrap()],
            "GBPUSD":[ticks_root.to_str().unwrap()]
        })
        .to_string(),
    )
    .unwrap();
    let output = root.join("result");
    let run = Command::new(env!("CARGO_BIN_EXE_forex-backtest"))
        .args([
            "--strategy",
            root.join("rules.json").to_str().unwrap(),
            "--data-config",
            root.join("sources.json").to_str().unwrap(),
            "--from",
            "2026-01-02",
            "--to",
            "2026-01-03",
            "--output",
            output.to_str().unwrap(),
            "--no-pdf",
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let result_path = output.join("results.json");
    if let Some(artifact_dir) = std::env::var_os("FOREX_TEST_ARTIFACT_DIR") {
        let artifact_dir = std::path::PathBuf::from(artifact_dir);
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::copy(&result_path, artifact_dir.join("results.json")).unwrap();
    }
    let result: Value = serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
    assert_eq!(result["execution"]["intrabar"], "mixed");
    assert_eq!(
        result["execution"]["intrabar_by_symbol"]["EURUSD"],
        "synthetic"
    );
    assert_eq!(
        result["execution"]["intrabar_by_symbol"]["GBPUSD"],
        "observed ticks"
    );
    assert_eq!(result["coverage"]["EURUSD"]["ticks"], 4 * 1440);
    assert_eq!(result["coverage"]["GBPUSD"]["ticks"], 1);
    fs::remove_dir_all(root).unwrap();
}
