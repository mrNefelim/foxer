use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "forex-prepared-history-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(p.join("prepared")).unwrap();
    p
}
fn csv() -> String {
    "timestamp_utc,timestamp_ms,bar_end_utc,open,high,low,close,quote_count,gap_before_minutes\n2024-01-08T00:00:00Z,1704672000000,2024-01-08T00:01:00Z,1100000,1100100,1099900,1100000,1,0\n".into()
}
fn quality(
    root: &Path,
    source: &str,
    symbol: &str,
    timezone: &str,
    price_basis: &str,
    include_file: bool,
    contents: &str,
) {
    let digest = format!("{:x}", Sha256::digest(contents.as_bytes()));
    let outputs = if include_file {
        json!({format!("{symbol}-M1-2024-01.csv"): {"sha256":digest,"bytes":contents.len()}})
    } else {
        json!({})
    };
    fs::write(root.join("quality-report.json"), serde_json::to_vec(&json!({
        "source":source,"symbol":symbol,"timezone":timezone,"price_basis":price_basis,"outputs":outputs
    })).unwrap()).unwrap();
    if include_file {
        fs::write(
            root.join("prepared")
                .join(format!("{symbol}-M1-2024-01.csv")),
            contents,
        )
        .unwrap();
    }
}
fn write_config(root: &Path, reference_exit: bool) -> PathBuf {
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read("../tests/fixtures/swing-legacy-rules.json").unwrap())
            .unwrap();
    let mut cfg = document["instruments"]["EURUSD"].clone();
    if reference_exit {
        cfg["reference_exit"] = json!({"sign": 1, "recent_events": 4});
    }
    let path = root.join("swing.json");
    fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    path
}
fn run(root: &Path, symbol: &str, config: &Path, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_forex-backtest"))
        .args([
            "swing-history",
            root.to_str().unwrap(),
            symbol,
            config.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

#[test]
fn prepared_history_rejects_missing_files_and_no_quotes_but_accepts_no_signals() {
    let root = temp();
    let good_csv = csv();
    let config = write_config(&root, false);
    quality(
        &root,
        "test source",
        "EURUSD",
        "UTC",
        "index",
        false,
        &good_csv,
    );
    let missing_output = root.join("missing.csv");
    let missing = run(&root, "EURUSD", &config, &missing_output);
    assert!(
        !missing.status.success(),
        "empty prepared file set must fail"
    );
    assert!(!missing_output.exists());

    quality(&root, "test source", "EURUSD", "UTC", "index", true, "");
    let empty_output = root.join("empty.csv");
    let empty = run(&root, "EURUSD", &config, &empty_output);
    assert!(
        !empty.status.success(),
        "prepared files with no quotes must fail"
    );
    assert!(!empty_output.exists());

    quality(
        &root,
        "test source",
        "EURUSD",
        "UTC",
        "index",
        true,
        &good_csv,
    );
    let valid_output = root.join("valid.csv");
    let valid = run(&root, "EURUSD", &config, &valid_output);
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    let result = fs::read_to_string(valid_output).unwrap();
    assert_eq!(
        result.lines().count(),
        1,
        "valid history may have no trades"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_history_checks_symbol_timezone_basis_and_nonempty_source() {
    let root = temp();
    let good_csv = csv();
    let config = write_config(&root, false);
    for (source, symbol, timezone, basis) in [
        ("test source", "GBPUSD", "UTC", "index"),
        ("test source", "EURUSD", "America/New_York", "index"),
        ("test source", "EURUSD", "UTC", "bid"),
        ("", "EURUSD", "UTC", "index"),
    ] {
        quality(&root, source, symbol, timezone, basis, true, &good_csv);
        let output = root.join("bad.csv");
        let result = run(&root, "EURUSD", &config, &output);
        assert!(
            !result.status.success(),
            "accepted metadata {source}/{symbol}/{timezone}/{basis}"
        );
        assert!(!output.exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_history_rejects_reference_exit_without_index_inputs() {
    let root = temp();
    let good_csv = csv();
    quality(
        &root,
        "test source",
        "EURUSD",
        "UTC",
        "index",
        true,
        &good_csv,
    );
    let cfg_path = write_config(&root, true);
    let output = root.join("trades.csv");
    let result = Command::new(env!("CARGO_BIN_EXE_forex-backtest"))
        .args([
            "swing-history",
            root.to_str().unwrap(),
            "EURUSD",
            cfg_path.to_str().unwrap(),
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!output.exists());
    fs::remove_dir_all(root).unwrap();
}
