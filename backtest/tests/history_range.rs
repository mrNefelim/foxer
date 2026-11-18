use forex_backtest::history::read_minutes;
use serde_json::json;
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
