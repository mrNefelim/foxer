//! RED contract for selecting and replaying a per-instrument trend-pullback family.
use forex_backtest::swing_replay;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn path(ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "trend-pullback-red-{}-{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        ext
    ))
}

fn swing_config() -> Value {
    let file: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swing-legacy-rules.json")).unwrap();
    let mut config = file["instruments"]["EURUSD"].clone();
    config["start"] = serde_json::json!(0);
    config["range"]["enabled"] = serde_json::json!(false);
    config
}

fn minutes() -> Vec<(i64, f64)> {
    let mut closes = vec![1.1; 32];
    closes.extend((1..=12).map(|i| 1.1 + f64::from(i) * 0.001));
    closes.extend([1.107, 1.109]);
    let start = chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    closes
        .into_iter()
        .enumerate()
        .map(|(i, close)| (start + i as i64 * 4 * 3600, close))
        .collect()
}

fn write_series(path: &PathBuf, boundary_offset_seconds: i64, mirror: bool, close_count: usize) {
    let rows = minutes().into_iter().take(close_count).collect::<Vec<_>>();
    let mut output = String::new();
    for (time, price) in &rows {
        let dt = chrono::DateTime::from_timestamp(*time, 0).unwrap();
        let (open, high, low, close) = if mirror {
            (
                2.2 - price,
                2.2 - (price - 0.0002),
                2.2 - (price + 0.0002),
                2.2 - price,
            )
        } else {
            (*price, *price + 0.0002, *price - 0.0002, *price)
        };
        output.push_str(&format!(
            "{}\t{}\t{open:.5}\t{high:.5}\t{low:.5}\t{close:.5}\t1\t0\t1\n",
            dt.format("%Y.%m.%d"),
            dt.format("%H:%M:%S"),
        ));
    }
    // This boundary minute is observed exactly when the signal H4 bar closes.
    let (last_time, _) = rows.last().unwrap();
    let entry_time = last_time + 4 * 3600 + boundary_offset_seconds;
    let dt = chrono::DateTime::from_timestamp(entry_time, 0).unwrap();
    let (open, high, low) = if mirror {
        (1.0905, 1.0910, 1.0900)
    } else {
        (1.1095, 1.1100, 1.1090)
    };
    output.push_str(&format!(
        "{}\t{}\t{open:.5}\t{high:.5}\t{low:.5}\t{open:.5}\t1\t0\t1\n",
        dt.format("%Y.%m.%d"),
        dt.format("%H:%M:%S")
    ));
    fs::write(path, output).unwrap();
}

fn replay_variant(
    config: &Value,
    offset_seconds: i64,
    mirror: bool,
    close_count: usize,
) -> Result<String, String> {
    let csv = path("csv");
    let config_path = path("json");
    let output = path("out.csv");
    write_series(&csv, offset_seconds, mirror, close_count);
    fs::write(&config_path, serde_json::to_vec(config).unwrap()).unwrap();
    let result = swing_replay::run(&csv, &config_path, &output, None);
    let trades = fs::read_to_string(&output).unwrap_or_default();
    for file in [&csv, &config_path, &output] {
        fs::remove_file(file).ok();
    }
    result.map(|()| trades)
}

fn replay(config: &Value) -> Result<String, String> {
    replay_variant(config, 0, false, minutes().len())
}

fn replay_at_boundary(config: &Value, offset_seconds: i64) -> Result<String, String> {
    replay_variant(config, offset_seconds, false, minutes().len())
}

fn trend_config() -> Value {
    let mut config = swing_config();
    config["family"] = serde_json::json!("trend_pullback");
    config["trend_pullback"] = serde_json::json!({
        "fast_ema_bars": 8,
        "slow_ema_bars": 32
    });
    config["atr_period"] = serde_json::json!(14);
    config["stop_lookback_h4_bars"] = serde_json::json!(10);
    config["stop_buffer_atr"] = serde_json::json!(0.1);
    config["rr_fixed"] = serde_json::json!(2.0);
    config["take_profit"] = serde_json::json!("fixed");
    config
}

#[test]
fn legacy_json_without_family_still_uses_the_existing_swing_path() {
    let output = replay(&swing_config()).unwrap();
    assert_eq!(
        output.lines().count(),
        1,
        "fixture has no legacy weekly plan"
    );
}

#[test]
fn complete_trend_pullback_family_emits_a_trade_after_the_closed_bar_signal() {
    let config = trend_config();
    let output = replay(&config).unwrap();
    let trades = output.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(
        trades.len(),
        1,
        "only the confirmed post-pullback crossing should signal; got:\n{output}"
    );
    let trade = trades[0].split(',').collect::<Vec<_>>();
    assert_eq!(trade[0], "trend_pullback");
    let expected_signal =
        chrono::DateTime::from_timestamp(minutes().last().unwrap().0 + 4 * 3600, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
    assert_eq!(
        trade[2], expected_signal,
        "signal time is the confirmed H4 close"
    );
    let entry: f64 = trade[4].parse().unwrap();
    assert!(
        (entry - 1.1091).abs() < 1e-9,
        "entry must use H4 close plus configured cost: {entry}"
    );
}

#[test]
fn first_observation_inside_the_boundary_minute_keeps_the_real_fill_time() {
    let output = replay_at_boundary(&trend_config(), 5).unwrap();
    let trades = output.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(
        trades.len(),
        1,
        "boundary-minute observation is actionable: {output}"
    );
    let trade = trades[0].split(',').collect::<Vec<_>>();
    assert_eq!(trade[4], "1.1091");
    assert_eq!(trade[3], "2024-01-08 16:00:05");
}

#[test]
fn missing_boundary_minute_does_not_backfill_the_closed_signal() {
    let output = replay_at_boundary(&trend_config(), 60).unwrap();
    assert_eq!(
        output.lines().count(),
        1,
        "a later bar cannot fill the missed signal: {output}"
    );
}

#[test]
fn short_pullback_is_the_mirror_of_the_long_signal() {
    let output = replay_variant(&trend_config(), 0, true, minutes().len()).unwrap();
    let trades = output.lines().skip(1).collect::<Vec<_>>();
    assert_eq!(
        trades.len(),
        1,
        "expected one short pullback trade: {output}"
    );
    let trade = trades[0].split(',').collect::<Vec<_>>();
    assert_eq!(trade[0], "trend_pullback");
    assert_eq!(trade[1], "-1");
    assert_eq!(trade[2], "2024-01-08 16:00:00");
    assert_eq!(trade[4], "1.0909");
}

#[test]
fn insufficient_slow_ema_history_produces_no_trade() {
    let output = replay_variant(&trend_config(), 0, false, 20).unwrap();
    assert_eq!(
        output.lines().count(),
        1,
        "slow EMA must be fully seeded: {output}"
    );
}

#[test]
fn pullback_without_the_recovery_close_produces_no_trade() {
    let output = replay_variant(&trend_config(), 0, false, 45).unwrap();
    assert_eq!(
        output.lines().count(),
        1,
        "signal requires a closed recovery bar: {output}"
    );
}

#[test]
fn signal_before_configured_start_is_not_executed() {
    let mut config = trend_config();
    let signal_time = minutes().last().unwrap().0 + 4 * 3600;
    config["start"] = serde_json::json!(signal_time + 1);
    let output = replay(&config).unwrap();
    assert_eq!(
        output.lines().count(),
        1,
        "pre-start crossing is context only: {output}"
    );
}

#[test]
fn partial_family_parameters_are_rejected_instead_of_ignored() {
    let mut config = swing_config();
    config["family"] = serde_json::json!("trend_pullback");
    config["trend_pullback"] = serde_json::json!({ "fast_ema_bars": 8 });
    assert!(replay(&config).is_err(), "slow EMA is required");
}

#[test]
fn unknown_family_is_rejected_instead_of_falling_back_to_legacy() {
    let mut config = swing_config();
    config["family"] = serde_json::json!("made_up");
    assert!(
        replay(&config).is_err(),
        "unsupported family must fail closed"
    );
}
