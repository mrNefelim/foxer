//! Streaming-state, duplicate-close, and snapshot contracts for trend_pullback.
use forex_core::swing::{Bar, StrategyFamily, Swing, SwingConfig, Tf, TradeState};
use serde_json::Value;

fn config_value() -> Value {
    let file: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/swing-legacy-rules.json")).unwrap();
    let mut config = file["instruments"]["EURUSD"].clone();
    config["family"] = serde_json::json!("trend_pullback");
    config["trend_pullback"] = serde_json::json!({
        "fast_ema_bars": 8,
        "slow_ema_bars": 32
    });
    config["start"] = serde_json::json!(0);
    config["atr_period"] = serde_json::json!(14);
    config["stop_lookback_h4_bars"] = serde_json::json!(10);
    config["stop_buffer_atr"] = serde_json::json!(0.1);
    config["rr_fixed"] = serde_json::json!(2.0);
    config["take_profit"] = serde_json::json!("fixed");
    config
}

fn config() -> SwingConfig {
    let config: SwingConfig = serde_json::from_value(config_value()).unwrap();
    config.validate().unwrap();
    assert_eq!(config.family, StrategyFamily::TrendPullback);
    config
}

fn limited_config(limit: usize) -> SwingConfig {
    let mut config = config_value();
    config["trend_pullback"]["max_open_positions"] = serde_json::json!(limit);
    let config: SwingConfig = serde_json::from_value(config).unwrap();
    config.validate().unwrap();
    config
}

#[test]
fn max_open_positions_accepts_one_and_legacy_omission_stays_unlimited() {
    let mut capped = config_value();
    capped["trend_pullback"]["max_open_positions"] = serde_json::json!(1);
    let parsed: SwingConfig = serde_json::from_value(capped).unwrap();
    parsed.validate().unwrap();

    let old = config_value();
    assert!(old["trend_pullback"].get("max_open_positions").is_none());
    let parsed: SwingConfig = serde_json::from_value(old).unwrap();
    parsed.validate().unwrap();
}

#[test]
fn max_open_positions_rejects_zero_negative_null_and_noninteger_values() {
    for invalid in [
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::Value::Null,
        serde_json::json!("1"),
        serde_json::json!(1.5),
    ] {
        let mut config = config_value();
        config["trend_pullback"]["max_open_positions"] = invalid.clone();
        let result = serde_json::from_value::<SwingConfig>(config)
            .map_err(|_| "deserialize")
            .and_then(|parsed| parsed.validate().map_err(|_| "validate"));
        assert!(result.is_err(), "invalid cap must fail: {invalid}");
    }
}

#[test]
fn max_open_positions_blocks_second_long_signal_updates_ema_and_releases_slot() {
    assert_position_cap_sequence(&repeated_pullbacks(), 1);
}

#[test]
fn max_open_positions_applies_to_short_trend_signals() {
    let mirrored = repeated_pullbacks()
        .into_iter()
        .map(|price| 2.2 - price)
        .collect::<Vec<_>>();
    assert_position_cap_sequence(&mirrored, -1);
}

fn bars(closes: &[f64], start: i64) -> Vec<Bar> {
    closes
        .iter()
        .enumerate()
        .map(|(i, close)| Bar {
            end: start + (i as i64 + 1) * 14_400,
            open: *close,
            high: *close + 0.0002,
            low: *close - 0.0002,
            close: *close,
        })
        .collect()
}

fn pullback() -> Vec<f64> {
    let mut closes = vec![1.1; 32];
    closes.extend((1..=12).map(|i| 1.1 + f64::from(i) * 0.001));
    closes.extend([1.107, 1.109]);
    closes
}

fn short_pullback() -> Vec<f64> {
    let mut closes = vec![1.1; 32];
    closes.extend((1..=12).map(|i| 1.1 - f64::from(i) * 0.001));
    closes.extend([1.093, 1.091]);
    closes
}

fn repeated_pullbacks() -> Vec<f64> {
    let mut closes = pullback();
    closes.extend([1.105, 1.104, 1.106, 1.112]);
    // A distinct pullback/recovery after index 49 supplies a fresh signal
    // once the test explicitly releases the first trade's slot.
    closes.extend([1.113, 1.114, 1.110, 1.109, 1.112]);
    closes
}

fn assert_position_cap_sequence(closes: &[f64], direction: i8) {
    let input = bars(closes, 1_704_067_200);
    let mut capped = Swing::new(limited_config(1));
    let mut uncapped = Swing::new(config());
    for bar in &input[..50] {
        capped.close(Tf::H4, *bar, bar.end);
        uncapped.close(Tf::H4, *bar, bar.end);
    }

    // Admission precedes the same-time minute event that exits the first trade.
    assert_eq!(capped.trades().len(), 1);
    assert_eq!(uncapped.trades().len(), 2);
    assert_eq!(capped.trades()[0].dir, direction);
    assert_eq!(uncapped.trades()[1].dir, direction);
    let capped_snapshot = serde_json::to_value(&capped).unwrap();
    let uncapped_snapshot = serde_json::to_value(&uncapped).unwrap();
    assert_eq!(
        capped_snapshot["trend_pullback"], uncapped_snapshot["trend_pullback"],
        "EMA state must advance identically while capacity is full"
    );

    let first = capped.trades()[0].clone();
    let blocked_signal_end = input[49].end;
    capped.minute(blocked_signal_end, first.stop, first.stop);
    assert_eq!(capped.trades()[0].state, TradeState::Stop);
    assert_eq!(
        capped.trades().len(),
        1,
        "blocked signal must not be deferred"
    );

    // A fresh recovery signal is accepted after the prior slot has been released.
    for bar in &input[50..] {
        capped.close(Tf::H4, *bar, bar.end);
    }
    assert_eq!(capped.trades().len(), 2);
    assert_eq!(capped.trades()[1].dir, direction);
    assert_eq!(capped.trades()[1].signal_t, input.last().unwrap().end);
    assert_eq!(capped.trades()[1].state, TradeState::Active);
}

#[test]
fn duplicate_h4_close_does_not_advance_ema_or_emit_a_second_trade() {
    let input = bars(&pullback(), 1_704_067_200);
    let mut swing = Swing::new(config());
    for bar in &input {
        swing.close(Tf::H4, *bar, bar.end);
    }
    assert_eq!(swing.trades().len(), 1);
    let last = *input.last().unwrap();
    swing.close(Tf::H4, last, last.end);
    assert_eq!(swing.trades().len(), 1);
}

#[test]
fn ema_state_survives_snapshot_and_h4_frame_compaction() {
    let mut history = vec![1.1; 140];
    history.extend(pullback().into_iter().skip(32));
    let input = bars(&history, 1_704_067_200);
    let mut uninterrupted = Swing::new(config());
    let mut restarted = Swing::new(config());
    for bar in &input[..132] {
        uninterrupted.close(Tf::H4, *bar, bar.end);
        restarted.close(Tf::H4, *bar, bar.end);
    }
    assert!(uninterrupted.frame(Tf::H4).stored() <= 128);
    let snapshot = serde_json::to_value(&restarted).unwrap();
    restarted = serde_json::from_value(snapshot).unwrap();
    for bar in &input[132..] {
        uninterrupted.close(Tf::H4, *bar, bar.end);
        restarted.close(Tf::H4, *bar, bar.end);
    }
    assert_eq!(
        serde_json::to_value(uninterrupted.trades()).unwrap(),
        serde_json::to_value(restarted.trades()).unwrap()
    );
    assert_eq!(restarted.trades().len(), 1);
    assert_eq!(restarted.trades()[0].state, TradeState::Active);
}

#[test]
fn trend_pullback_rejects_negative_and_nonfinite_cost() {
    let mut c = config();
    c.cost = -0.0001;
    assert!(c.validate().is_err(), "negative cost must be rejected");
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut c = config();
        c.cost = invalid;
        assert!(
            c.validate().is_err(),
            "nonfinite cost {invalid:?} must be rejected"
        );
    }
}

#[test]
fn trend_pullback_never_emits_trade_with_overflowed_levels() {
    let mut c = config();
    c.cost = f64::MAX / 2.0;
    c.validate()
        .expect("a finite nonnegative cost is structurally valid");
    let mut swing = Swing::new(c);
    for bar in bars(&pullback(), 1_704_067_200) {
        swing.close(Tf::H4, bar, bar.end);
    }
    assert!(
        swing.trades().is_empty(),
        "overflowed target must suppress trade"
    );
}

#[test]
fn trend_pullback_rejects_nonpositive_price_levels_from_finite_cost() {
    let mut c = config();
    c.cost = 2.0;
    c.validate().unwrap();
    let mut swing = Swing::new(c);
    for bar in bars(&short_pullback(), 1_704_067_200) {
        swing.close(Tf::H4, bar, bar.end);
    }
    assert!(
        swing.trades().is_empty(),
        "finite but nonpositive entry/target prices must not create an FX trade"
    );
}

#[test]
fn trend_pullback_keeps_finite_levels_at_cost_multipliers_one_two_three() {
    let base_cost = config().cost;
    for multiplier in [1.0, 2.0, 3.0] {
        let mut c = config();
        c.cost = base_cost * multiplier;
        c.validate().unwrap();
        let mut swing = Swing::new(c);
        for bar in bars(&pullback(), 1_704_067_200) {
            swing.close(Tf::H4, bar, bar.end);
        }
        assert_eq!(swing.trades().len(), 1, "cost ×{multiplier}");
        let trade = &swing.trades()[0];
        assert!(trade.entry.is_finite());
        assert!(trade.stop.is_finite());
        assert!(trade.tp.is_finite());
        assert!((trade.entry - trade.stop) * f64::from(trade.dir) > 0.0);
    }
}
