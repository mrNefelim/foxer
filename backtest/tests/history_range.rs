use forex_backtest::history::read_minutes;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

#[test]
fn strict_minute_reader_keeps_the_half_open_range_and_minute_values() {
    let root = std::env::temp_dir().join(format!("forex-history-range-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("EURUSD.csv");
    let mut csv = String::from(
        "timestamp_utc,timestamp_ms,bar_end_utc,open,high,low,close,quote_count,gap_before_minutes\n",
    );
    let first = 1_704_067_200i64;
    for timestamp in (first..=first + 180).step_by(60) {
        csv.push_str(&format!(
            "{},{},{},1.10000,1.10100,1.09900,1.10050,1,0\n",
            forex_core::prices::iso(timestamp).unwrap(),
            timestamp * 1000,
            forex_core::prices::iso(timestamp + 60).unwrap(),
        ));
    }
    fs::write(&path, csv).unwrap();
    let files = vec![(
        PathBuf::from(&path),
        json!({"sha256": forex_backtest::history::hash(&path).unwrap()}),
    )];
    let mut observed = Vec::new();
    read_minutes(
        &files,
        (first + 60) * 1000,
        (first + 180) * 1000,
        |minute| {
            observed.push(minute);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        observed
            .iter()
            .map(|minute| minute.timestamp)
            .collect::<Vec<_>>(),
        [first + 60, first + 120]
    );
    assert!(observed
        .iter()
        .all(|minute| minute.price == [1_100_000, 1_101_000, 1_099_000, 1_100_500]));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manifest_replacement_after_context_cannot_change_the_replay_prefix() {
    use forex_backtest::history::Dataset;
    use forex_core::model::Strategy;

    let root = std::env::temp_dir().join(format!("forex-history-manifest-{}", std::process::id()));
    fs::create_dir_all(root.join("prepared")).unwrap();
    let from = forex_core::prices::utc("2026-01-02T00:00:00Z").unwrap();
    let to = forex_core::prices::utc("2026-01-03T00:00:00Z").unwrap();
    let csv_for = |in_range_price: &str, malformed_prefix: bool| {
        let mut csv = String::from(
            "timestamp_utc,timestamp_ms,bar_end_utc,open,high,low,close,quote_count,gap_before_minutes\n",
        );
        let start = forex_core::prices::utc("2025-12-01T00:00:00Z").unwrap();
        for timestamp in (start..=to).step_by(60) {
            let (open, high, low, close) = if timestamp < from && malformed_prefix {
                (in_range_price, "1.00000", "1.00000", in_range_price)
            } else if timestamp >= from {
                if in_range_price == "1.10000" {
                    (in_range_price, "1.10100", "1.09900", in_range_price)
                } else {
                    (in_range_price, "1.20100", "1.19900", in_range_price)
                }
            } else {
                ("1.10000", "1.10100", "1.09900", "1.10050")
            };
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},1,0\n",
                forex_core::prices::iso(timestamp).unwrap(),
                timestamp * 1000,
                forex_core::prices::iso(timestamp + 60).unwrap(),
                open,
                high,
                low,
                close,
            ));
        }
        csv
    };
    let original_path = root.join("prepared/EURUSD.csv");
    let original_csv = csv_for("1.10000", false);
    fs::write(&original_path, &original_csv).unwrap();
    let original_sha = forex_backtest::history::hash(&original_path).unwrap();
    let quality_path = root.join("quality-report.json");
    let quality = json!({
        "source": "Synthetic prepared feed",
        "timezone": "UTC",
        "price_basis": "index",
        "symbol": "EURUSD",
        "origin": "mt5_m1_bars",
        "gaps": [],
        "missing_months": [],
        "outputs": {"EURUSD.csv": {"sha256": original_sha, "bytes": original_csv.len()}}
    });
    fs::write(&quality_path, serde_json::to_vec(&quality).unwrap()).unwrap();

    let mut config: Value =
        serde_json::from_slice(&fs::read("../strategy/fvg-bot-rules.json").unwrap()).unwrap();
    for timeframe in ["W", "D", "H4"] {
        config["context"]["minimum_bars"][timeframe] = json!(1);
    }
    let config: Strategy = serde_json::from_value(config).unwrap();
    let mut dataset = Dataset::open(vec![root.clone()], "EURUSD", from * 1000, to * 1000).unwrap();
    let (_engine, _aggregator, _counts, prepared_replay) =
        dataset.prepare("EURUSD", from * 1000, &config).unwrap();

    let replacement_path = root.join("prepared/EURUSD-alt-M1.csv");
    let replacement_csv = csv_for("1.20000", true);
    fs::write(&replacement_path, &replacement_csv).unwrap();
    let replacement_sha = forex_backtest::history::hash(&replacement_path).unwrap();
    let mut replacement_quality = quality;
    replacement_quality["outputs"] = json!({
        "EURUSD-alt-M1.csv": {"sha256": replacement_sha, "bytes": replacement_csv.len()}
    });
    fs::write(
        &quality_path,
        serde_json::to_vec(&replacement_quality).unwrap(),
    )
    .unwrap();

    let mut replay_prices = Vec::new();
    forex_backtest::history::read_minutes_after_context(
        &prepared_replay,
        from * 1000,
        to * 1000,
        |minute| {
            replay_prices.push(minute.price[0]);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(replay_prices.len(), 1440);
    assert!(replay_prices.iter().all(|price| *price == 1_100_000));
    fs::remove_dir_all(root).unwrap();
}
