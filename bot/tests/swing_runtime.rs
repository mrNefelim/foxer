//! Swing rules in the live runtime (requirements: Правила «свинг» в работе бота).
use forex_bot::{engine::Engine, feed::Tick, runtime::Runtime};
use forex_core::swing::{Bar, Kind, Swing, SwingConfig, Tf, TradeState};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "swing-runtime-{}-{}.sqlite",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn rules() -> Value {
    serde_json::from_str(include_str!("../../strategy/fvg-bot-rules.json")).unwrap()
}
fn legacy_fixture() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/swing-legacy-rules.json")).unwrap()
}
fn working_file() -> Value {
    serde_json::from_str(include_str!("../../strategy/swing-rules.json")).unwrap()
}
fn instrument_config(symbol: &str) -> Value {
    legacy_fixture()["instruments"][symbol].clone()
}
fn config() -> Value {
    instrument_config("EURUSD")
}
fn trend_config_for(symbol: &str) -> Value {
    let mut config = instrument_config(symbol);
    config["family"] = json!("trend_pullback");
    config["trend_pullback"] = json!({"fast_ema_bars": 8, "slow_ema_bars": 32});
    config["atr_period"] = json!(14);
    config["stop_lookback_h4_bars"] = json!(10);
    config["stop_buffer_atr"] = json!(0.1);
    config["rr_fixed"] = json!(2.0);
    config["take_profit"] = json!("fixed");
    config
}
fn trend_config() -> Value {
    trend_config_for("EURUSD")
}
fn trend_config_with_limit(limit: usize, cost: f64) -> Value {
    let mut config = trend_config();
    config["cost"] = json!(cost);
    config["trend_pullback"]["max_open_positions"] = json!(limit);
    config
}
fn warm_trend(config: &Value, start: i64) -> Swing {
    let parsed = serde_json::from_value::<SwingConfig>(config.clone()).unwrap();
    let mut swing = Swing::new(parsed);
    let mut closes = vec![1.1; 32];
    closes.extend((1..=12).map(|i| 1.1 + f64::from(i) * 0.001));
    closes.extend([1.107, 1.109]);
    for (index, close) in closes.into_iter().enumerate() {
        let end = start + (index as i64 + 1) * 14_400;
        let bar = Bar {
            end,
            open: close,
            high: close + 0.0002,
            low: close - 0.0002,
            close,
        };
        swing.close(Tf::H4, bar, end);
    }
    swing
}
fn warm_trend_before_signal(config: &Value, start: i64) -> Swing {
    let parsed = serde_json::from_value::<SwingConfig>(config.clone()).unwrap();
    let mut swing = Swing::new(parsed);
    let mut closes = vec![1.1; 32];
    closes.extend((1..=12).map(|i| 1.1 + f64::from(i) * 0.001));
    closes.push(1.107);
    for (index, close) in closes.into_iter().enumerate() {
        let end = start + (index as i64 + 1) * 14_400;
        let bar = Bar {
            end,
            open: close,
            high: close + 0.0002,
            low: close - 0.0002,
            close,
        };
        swing.close(Tf::H4, bar, end);
    }
    swing
}
fn close_h4_for_runtime_ticks(r: &mut Runtime, symbol: &str, boundary_utc: &str, close: f64) {
    let boundary_ms = chrono::DateTime::parse_from_rfc3339(boundary_utc)
        .unwrap()
        .timestamp_millis();
    let before_close = chrono::DateTime::from_timestamp_millis(boundary_ms - 60_000)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    r.tick(tick_for(symbol, &before_close, close), true)
        .unwrap();
    r.tick(tick_for(symbol, boundary_utc, close), true).unwrap();
}
fn close_h4_with_runtime_ticks(r: &mut Runtime, boundary_utc: &str, close: f64) {
    close_h4_for_runtime_ticks(r, "EURUSD", boundary_utc, close);
}
/// Rules with one long trend plan confirmed and entered on an H4 gap.
fn planned() -> Swing {
    planned_for("EURUSD")
}
fn planned_for(symbol: &str) -> Swing {
    let mut s =
        Swing::new(serde_json::from_value::<SwingConfig>(instrument_config(symbol)).unwrap());
    s.add_plan(Kind::Trend, 1, (1.0, 1.05), 1.2, Tf::H4);
    s
}
fn planned_limit() -> Swing {
    planned_limit_for("EURUSD")
}
fn planned_limit_for(symbol: &str) -> Swing {
    let mut s =
        Swing::new(serde_json::from_value::<SwingConfig>(instrument_config(symbol)).unwrap());
    s.add_plan(Kind::Trend, 1, (1.0, 1.05), 1.105, Tf::H4);
    s
}
fn tick(at: &str, price: f64) -> Tick {
    tick_for("EURUSD", at, price)
}
fn tick_for(symbol: &str, at: &str, price: f64) -> Tick {
    Tick {
        symbol: symbol.into(),
        millis: chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .timestamp_millis(),
        price: (price * 1_000_000.0).round() as i64,
        source: "upscale:index".into(),
    }
}
/// Three H4 bars on 2026-01-05 whose third leaves a bullish gap; the 22:00Z tick closes it.
fn gap_ticks(r: &mut Runtime) {
    for (at, p) in [
        ("2026-01-05T10:00:00Z", 1.10000),
        ("2026-01-05T10:30:00Z", 1.10100),
        ("2026-01-05T11:00:00Z", 1.09900),
        ("2026-01-05T13:59:00Z", 1.10050),
        ("2026-01-05T14:00:00Z", 1.10050),
        ("2026-01-05T15:00:00Z", 1.10300),
        ("2026-01-05T17:59:00Z", 1.10280),
        ("2026-01-05T18:00:00Z", 1.10280),
        ("2026-01-05T19:00:00Z", 1.10300),
        ("2026-01-05T21:59:00Z", 1.10250),
        ("2026-01-05T22:00:00Z", 1.10250),
    ] {
        r.tick(tick(at, p), true).unwrap();
    }
}
fn gap_ticks_on(r: &mut Runtime, day: &str) {
    for (time, p) in [
        ("10:00:00", 1.10000),
        ("10:30:00", 1.10100),
        ("11:00:00", 1.09900),
        ("13:59:00", 1.10050),
        ("14:00:00", 1.10050),
        ("15:00:00", 1.10300),
        ("17:59:00", 1.10280),
        ("18:00:00", 1.10280),
        ("19:00:00", 1.10300),
        ("21:59:00", 1.10250),
        ("22:00:00", 1.10250),
    ] {
        r.tick(tick(&format!("{day}T{time}Z"), p), true).unwrap();
    }
}
fn texts(r: &Runtime) -> Vec<String> {
    r.pending()
        .unwrap()
        .into_iter()
        .map(|o| o.payload["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}
fn count(r: &Runtime, needle: &str) -> usize {
    texts(r).iter().filter(|t| t.contains(needle)).count()
}
fn pending_state(r: &Runtime) -> Vec<(String, Value)> {
    r.pending()
        .unwrap()
        .into_iter()
        .map(|message| (message.id, message.payload))
        .collect()
}
fn acknowledge_pending(r: &mut Runtime) {
    let ids: Vec<_> = r.pending().unwrap().into_iter().map(|m| m.id).collect();
    for (index, id) in ids.into_iter().enumerate() {
        r.delivered(&id, 50_000 + index as i64).unwrap();
    }
}
fn first_quote_limit_fill(
    dir: i8,
) -> (std::path::PathBuf, forex_core::swing::Trade, String, String) {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    let mut initial = Swing::new(serde_json::from_value::<SwingConfig>(config()).unwrap());
    if dir > 0 {
        initial.add_plan(Kind::Trend, 1, (1.0, 1.05), 1.105, Tf::H4);
    } else {
        initial.add_plan(Kind::Trend, -1, (1.15, 1.2), 1.095, Tf::H4);
    }
    r.swing("EURUSD", config(), || Ok(initial)).unwrap();
    let ticks = [
        ("2026-01-05T10:00:00Z", 1.10000),
        ("2026-01-05T10:30:00Z", 1.10100),
        ("2026-01-05T11:00:00Z", 1.09900),
        ("2026-01-05T13:59:00Z", 1.10050),
        ("2026-01-05T14:00:00Z", 1.10050),
        ("2026-01-05T15:00:00Z", 1.10300),
        ("2026-01-05T17:59:00Z", 1.10280),
        ("2026-01-05T18:00:00Z", 1.10280),
        ("2026-01-05T19:00:00Z", 1.10300),
        ("2026-01-05T21:59:00Z", 1.10250),
    ];
    for (at, value) in ticks {
        let price = if dir > 0 { value } else { 2.2 - value };
        r.tick(tick(at, price), true).unwrap();
    }
    assert_eq!(count(&r, "Лимитная заявка"), 0);
    let close = if dir > 0 { 1.10150 } else { 2.2 - 1.10150 };
    r.tick(tick("2026-01-05T22:00:00Z", close), true).unwrap();
    let trade = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
    assert_eq!(trade.state, forex_core::swing::TradeState::Active);
    let messages = texts(&r);
    let limit = messages
        .iter()
        .find(|text| text.contains("Лимитная заявка"))
        .expect("initial limit notification")
        .clone();
    let fill = messages
        .iter()
        .find(|text| text.contains("Заявка исполнена"))
        .expect("initial fill notification")
        .clone();
    (p, trade, limit, fill)
}

#[test]
fn swing_trade_is_published_with_its_levels() {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Ok(planned())).unwrap();
    gap_ticks(&mut r);
    assert_eq!(count(&r, "EUR/USD · покупка · тренд"), 1);
    assert_eq!(count(&r, "Сделка открыта"), 1);
    let open = texts(&r)
        .into_iter()
        .find(|t| t.contains("Сделка открыта"))
        .unwrap();
    assert!(open.contains("Вход 1.10260"), "{open}");
    std::fs::remove_file(p).ok();
}

#[test]
fn state_survives_a_restart_without_repeating_messages() {
    let p = path();
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks(&mut r);
    }
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Err("must not warm up again".into()))
        .unwrap();
    assert_eq!(count(&r, "Сделка открыта"), 1);
    r.tick(tick("2026-01-05T22:30:00Z", 1.09000), true).unwrap();
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

#[test]
fn same_quote_fill_and_stop_are_both_published_once_across_restart() {
    let p = path();
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
        gap_ticks(&mut r);
        assert_eq!(count(&r, "Лимитная заявка"), 1);
    }

    let exit = tick("2026-01-05T22:30:00Z", 1.09800);
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Err("must restore snapshot".into()))
            .unwrap();
        r.tick(exit.clone(), true).unwrap();
        assert_eq!(count(&r, "Заявка исполнена"), 1);
        assert_eq!(count(&r, "Стоп"), 1);
        let messages = texts(&r);
        let filled = messages.iter().position(|m| m.contains("Заявка исполнена"));
        let stopped = messages.iter().position(|m| m.contains("Стоп"));
        assert!(filled.zip(stopped).is_some_and(|(a, b)| a < b));

        r.tick(exit, true).unwrap();
        assert_eq!(count(&r, "Заявка исполнена"), 1);
        assert_eq!(count(&r, "Стоп"), 1);
    }

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

#[test]
fn first_close_fill_and_stop_publishes_limit_fill_exit_in_order_across_restart() {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
    for (at, price) in [
        ("2026-01-05T10:00:00Z", 1.10000),
        ("2026-01-05T10:30:00Z", 1.10100),
        ("2026-01-05T11:00:00Z", 1.09900),
        ("2026-01-05T13:59:00Z", 1.10050),
        ("2026-01-05T14:00:00Z", 1.10050),
        ("2026-01-05T15:00:00Z", 1.10300),
        ("2026-01-05T17:59:00Z", 1.10280),
        ("2026-01-05T18:00:00Z", 1.10280),
        ("2026-01-05T19:00:00Z", 1.10300),
        ("2026-01-05T21:59:00Z", 1.10250),
    ] {
        r.tick(tick(at, price), true).unwrap();
    }
    assert_eq!(
        count(&r, "Лимитная заявка"),
        0,
        "order must be born on the final close tick"
    );

    let closing_tick = tick("2026-01-05T22:00:00Z", 1.10080);
    r.tick(closing_tick.clone(), true).unwrap();
    let messages = texts(&r);
    let limit = messages
        .iter()
        .position(|text| text.contains("Лимитная заявка"));
    let fill = messages
        .iter()
        .position(|text| text.contains("Заявка исполнена"));
    let exit = messages.iter().position(|text| text.contains("Стоп"));
    assert!(
        limit
            .zip(fill)
            .zip(exit)
            .is_some_and(|((a, b), c)| a < b && b < c),
        "first tick must publish limit → fill → stop: {messages:?}"
    );
    for text in messages.iter().filter(|text| {
        text.contains("Лимитная заявка")
            || text.contains("Заявка исполнена")
            || text.contains("Стоп")
    }) {
        assert!(
            text.contains("(лимит)"),
            "same-timestamp fill must remain a limit order: {text}"
        );
    }
    r.tick(closing_tick, true).unwrap();
    assert_eq!(count(&r, "Лимитная заявка"), 1);
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 1);
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(count(&r, "Лимитная заявка"), 1);
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

#[test]
fn first_close_limit_fill_while_active_publishes_limit_then_fill_once() {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
    for (at, price) in [
        ("2026-01-05T10:00:00Z", 1.10000),
        ("2026-01-05T10:30:00Z", 1.10100),
        ("2026-01-05T11:00:00Z", 1.09900),
        ("2026-01-05T13:59:00Z", 1.10050),
        ("2026-01-05T14:00:00Z", 1.10050),
        ("2026-01-05T15:00:00Z", 1.10300),
        ("2026-01-05T17:59:00Z", 1.10280),
        ("2026-01-05T18:00:00Z", 1.10280),
        ("2026-01-05T19:00:00Z", 1.10300),
        ("2026-01-05T21:59:00Z", 1.10250),
    ] {
        r.tick(tick(at, price), true).unwrap();
    }
    assert_eq!(count(&r, "Лимитная заявка"), 0);

    let closing_tick = tick("2026-01-05T22:00:00Z", 1.10150);
    r.tick(closing_tick.clone(), true).unwrap();
    let trade = &r.swing_rules("EURUSD").unwrap().trades()[0];
    assert_eq!(trade.state, forex_core::swing::TradeState::Active);
    assert!(trade.stop < closing_tick.price as f64 / 1_000_000.0);
    let messages = texts(&r);
    let limit = messages
        .iter()
        .position(|text| text.contains("Лимитная заявка"));
    let fill = messages
        .iter()
        .position(|text| text.contains("Заявка исполнена"));
    assert!(
        limit.zip(fill).is_some_and(|(a, b)| a < b),
        "first active tick must publish limit → fill: {messages:?}"
    );
    let lifecycle: Vec<_> = messages
        .iter()
        .filter(|text| text.contains("Лимитная заявка") || text.contains("Заявка исполнена"))
        .collect();
    assert_eq!(lifecycle.len(), 2, "{messages:?}");
    assert!(
        lifecycle.iter().all(|text| text.contains("(лимит)")),
        "{lifecycle:?}"
    );
    assert!(
        lifecycle.iter().all(|text| !text.contains("Результат")),
        "{lifecycle:?}"
    );

    r.tick(closing_tick, true).unwrap();
    r.tick(tick("2026-01-05T22:01:00Z", 1.10150), true).unwrap();
    assert_eq!(count(&r, "Лимитная заявка"), 1);
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 0);
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(count(&r, "Лимитная заявка"), 1);
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 0);
    std::fs::remove_file(p).ok();
}

fn assert_limit_and_fill_prices(dir: i8) {
    let (p, trade, limit_text, fill_text) = first_quote_limit_fill(dir);
    let cost = serde_json::from_value::<SwingConfig>(config())
        .unwrap()
        .cost;
    assert!(cost > 0.0);
    let original_limit = trade.entry - f64::from(dir) * cost;
    let limit_rr = (trade.tp - original_limit).abs() / (original_limit - trade.stop).abs();
    let paid_rr = (trade.tp - trade.entry).abs() / (trade.entry - trade.stop).abs();
    assert_ne!(
        format!("{limit_rr:.2}"),
        format!("{paid_rr:.2}"),
        "fixture must expose cost effect"
    );
    assert!(limit_text.contains("(лимит)"), "{limit_text}");
    assert!(
        limit_text.contains(&format!("Вход {original_limit:.5}")),
        "{limit_text}"
    );
    assert!(
        limit_text.contains(&format!("RR {limit_rr:.2}")),
        "{limit_text}"
    );
    assert!(fill_text.contains("(лимит)"), "{fill_text}");
    assert!(
        fill_text.contains(&format!("Вход {:.5}", trade.entry)),
        "{fill_text}"
    );
    assert!(
        fill_text.contains(&format!("RR {paid_rr:.2}")),
        "{fill_text}"
    );
    assert!(!fill_text.contains("Результат"), "{fill_text}");
    std::fs::remove_file(p).ok();
}

#[test]
fn limit_notification_uses_original_price_and_rr_for_buy() {
    assert_limit_and_fill_prices(1);
}

#[test]
fn limit_notification_uses_original_price_and_rr_for_sell() {
    assert_limit_and_fill_prices(-1);
}

#[test]
fn legacy_pending_snapshot_without_entry_kind_keeps_limit_identity() {
    let p = path();
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
        gap_ticks(&mut r);
        assert_eq!(
            r.swing_rules("EURUSD").unwrap().trades()[0].state,
            forex_core::swing::TradeState::Pending
        );
    }
    let mut store = forex_bot::state::Store::open(&p).unwrap();
    let mut snapshot = store.entities(forex_bot::state::Kind::Runtime).unwrap()[0]
        .value
        .clone();
    snapshot["markets"]["EURUSD"]["swing"]["rules"]["trades"][0]
        .as_object_mut()
        .unwrap()
        .remove("entry_kind");
    store.save_runtime(&snapshot).unwrap();
    drop(store);

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.tick(tick("2026-01-05T22:30:00Z", 1.09800), true).unwrap();
    let messages = texts(&r);
    let fill = messages
        .iter()
        .find(|text| text.contains("Заявка исполнена"))
        .expect("legacy pending order fill");
    assert!(fill.contains("(лимит)"), "{fill}");
    assert!(
        !fill.contains("Результат"),
        "fill notification precedes exit: {fill}"
    );
    assert_eq!(count(&r, "Стоп"), 1);
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

#[test]
fn disabled_swing_keeps_pending_order_and_tracks_it_after_restarts() {
    let p = path();
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
        gap_ticks(&mut r);
        assert_eq!(
            r.swing_rules("EURUSD").unwrap().trades()[0].state,
            forex_core::swing::TradeState::Pending
        );
    }

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    let before = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
    r.reconcile_swing(&BTreeSet::new()).unwrap();
    let pending = &r.swing_rules("EURUSD").unwrap().trades()[0];
    assert_eq!(pending.state, forex_core::swing::TradeState::Pending);
    assert_eq!(
        (pending.entry, pending.stop, pending.tp),
        (before.entry, before.stop, before.tp)
    );

    // Tick prices are quantized to 1e-6; cross the entry by one pipette so the
    // fixture does not depend on the binary representation of the limit.
    let fill = tick("2026-01-05T22:30:00Z", before.entry - 0.0001);
    r.tick(fill.clone(), true).unwrap();
    assert_eq!(
        r.swing_rules("EURUSD").unwrap().trades()[0].state,
        forex_core::swing::TradeState::Active
    );
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    r.tick(fill, true).unwrap();
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    let active_before_restart = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
    drop(r);

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    let active = &r.swing_rules("EURUSD").unwrap().trades()[0];
    assert_eq!(active.state, forex_core::swing::TradeState::Active);
    assert_eq!(
        (active.entry, active.stop, active.tp),
        (
            active_before_restart.entry,
            active_before_restart.stop,
            active_before_restart.tp
        )
    );
    let stop = before.stop - 0.0001;
    r.tick(tick("2026-01-05T22:31:00Z", stop), true).unwrap();
    assert_eq!(count(&r, "Стоп"), 1);
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(count(&r, "Заявка исполнена"), 1);
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

#[test]
fn open_trades_from_the_warm_up_are_announced_once() {
    let p = path();
    let warm = {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks(&mut r);
        r.swing_rules("EURUSD").unwrap().clone()
    };
    let p2 = path();
    let mut r = Runtime::open(&p2, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Ok(warm.clone())).unwrap();
    r.swing("EURUSD", config(), || Ok(warm.clone())).unwrap();
    assert_eq!(count(&r, "до запуска"), 1);
    for f in [p, p2] {
        std::fs::remove_file(f).ok();
    }
}

#[test]
fn changed_parameters_warm_up_again_and_index_exit_is_rejected() {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Ok(planned())).unwrap();
    let mut warmed = false;
    let mut changed = config();
    changed["cost"] = json!(0.0002);
    r.swing("EURUSD", changed, || {
        warmed = true;
        Ok(planned())
    })
    .unwrap();
    assert!(warmed);
    let mut index = config();
    index["reference_exit"] = json!({"sign": -1, "recent_events": 50});
    assert!(r.swing("EURUSD", index, || Ok(planned())).is_err());
    assert!(r
        .swing("EURUSD", json!({"cost": 0.0001}), || Ok(planned()))
        .is_err());
    assert!(r.swing("XAUUSD", config(), || Ok(planned())).is_err());
    std::fs::remove_file(p).ok();
}

#[test]
fn reconfigure_and_restart_keep_published_trade_on_its_original_levels() {
    let p = path();
    let before = {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks(&mut r);
        let trade = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
        assert_eq!(trade.state, forex_core::swing::TradeState::Active);

        let next = trend_config();
        r.swing("EURUSD", next.clone(), || {
            let parsed = serde_json::from_value::<SwingConfig>(next.clone()).unwrap();
            Ok(Swing::new(parsed))
        })
        .unwrap();
        trade
    };

    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        let stop_quote = before.stop - 0.0001;
        r.tick(tick("2026-01-05T22:30:00Z", stop_quote), true)
            .unwrap();
        assert_eq!(
            count(&r, "Стоп"),
            1,
            "retired strategy must keep managing the old trade"
        );
    }
    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(count(&r, "Стоп"), 1, "restart must not duplicate the exit");
    let _ = std::fs::remove_file(p);
}

#[test]
fn a_pending_order_survives_family_change_with_its_frozen_execution_config() {
    let p = path();
    let before = {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
        gap_ticks(&mut r);
        let trade = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
        assert_eq!(trade.state, forex_core::swing::TradeState::Pending);

        let mut next = trend_config();
        next["cost"] = json!(0.0002);
        r.swing("EURUSD", next.clone(), || {
            let parsed = serde_json::from_value::<SwingConfig>(next.clone()).unwrap();
            Ok(Swing::new(parsed))
        })
        .unwrap();
        let snapshot = forex_bot::state::Store::open(&p)
            .unwrap()
            .entities(forex_bot::state::Kind::Runtime)
            .unwrap()[0]
            .value
            .clone();
        let retired = &snapshot["markets"]["EURUSD"]["swing"]["retired"][0]["rules"]["trades"][0];
        for (field, expected) in [
            ("entry", trade.entry),
            ("stop", trade.stop),
            ("tp", trade.tp),
        ] {
            let actual = retired[field].as_f64().unwrap();
            assert!(
                (actual - expected).abs() <= 1e-12,
                "{field} changed across snapshot: {actual} vs {expected}"
            );
        }
        trade
    };

    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.tick(tick("2026-01-05T22:30:00Z", before.entry - 0.0001), true)
            .unwrap();
        assert_eq!(count(&r, "Заявка исполнена"), 1);
    }
    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(
        count(&r, "Заявка исполнена"),
        1,
        "restart must preserve exactly one fill"
    );
    std::fs::remove_file(p).ok();
}

#[test]
fn runtime_h4_ticks_block_on_retired_slots_then_admit_after_release() {
    const HISTORY_START: i64 = 1_704_067_200;
    let p = path();
    let mut old_pending;
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
        gap_ticks(&mut r);
        old_pending = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
        assert_eq!(old_pending.state, TradeState::Pending);

        let tp = trend_config_with_limit(1, 0.0001);
        r.swing("EURUSD", tp.clone(), || {
            Ok(warm_trend_before_signal(&tp, HISTORY_START))
        })
        .unwrap();
        assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());

        // Keep this synthetic saved Pending trade away from the TP prices in
        // the test stream while preserving a coherent 2R legacy order.
        let old_stop = old_pending.entry - 0.01;
        let old_tp = old_pending.entry + 0.02;
        let mut store = forex_bot::state::Store::open(&p).unwrap();
        let mut snapshot = store.entities(forex_bot::state::Kind::Runtime).unwrap()[0]
            .value
            .clone();
        let saved = &mut snapshot["markets"]["EURUSD"]["swing"]["retired"][0]["rules"]["trades"][0];
        assert_eq!(saved["state"], "Pending");
        saved["stop"] = json!(old_stop);
        saved["tp"] = json!(old_tp);
        saved["rr"] = json!(2.0);
        store.save_runtime(&snapshot).unwrap();
        old_pending.stop = old_stop;
        old_pending.tp = old_tp;
        old_pending.rr = 2.0;
        drop(r);
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();

        // These bars would confirm the TP entry at 06:00Z without an external
        // slot; the retired Pending legacy order must reserve the only slot.
        close_h4_with_runtime_ticks(&mut r, "2026-01-06T02:00:00Z", 1.107);
        assert_eq!(r.swing_rules("EURUSD").unwrap().trades().len(), 0);
        close_h4_with_runtime_ticks(&mut r, "2026-01-06T06:00:00Z", 1.109);
        assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
        let pending = forex_bot::state::Store::open(&p)
            .unwrap()
            .entities(forex_bot::state::Kind::Runtime)
            .unwrap()[0]
            .value
            .clone();
        assert_eq!(
            pending["markets"]["EURUSD"]["swing"]["retired"][0]["rules"]["trades"][0]["state"],
            "Pending"
        );

        // Fill the retired order; it must keep exactly one slot after transition.
        r.tick(
            tick("2026-01-06T06:30:00Z", old_pending.entry - 0.0001),
            true,
        )
        .unwrap();
        assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    }

    // Restart must preserve the retired Active slot. A second actual TP signal
    // at 22:00Z is blocked while that position remains open.
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    let blocked_closes = [
        ("2026-01-06T10:00:00Z", 1.105),
        ("2026-01-06T14:00:00Z", 1.104),
        ("2026-01-06T18:00:00Z", 1.106),
        ("2026-01-06T22:00:00Z", 1.112),
    ];
    for (boundary, close) in blocked_closes {
        close_h4_with_runtime_ticks(&mut r, boundary, close);
        assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    }
    let active = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    assert_eq!(
        active["markets"]["EURUSD"]["swing"]["retired"][0]["rules"]["trades"][0]["state"],
        "Active"
    );

    // The old trade exits between H4 closes. The exit releases the slot for a
    // later confirmed signal, not retroactively for the 22:00Z signal above.
    r.tick(
        tick("2026-01-06T22:30:00Z", old_pending.stop - 0.0001),
        true,
    )
    .unwrap();
    let released_closes = [
        ("2026-01-07T02:00:00Z", 1.105),
        ("2026-01-07T06:00:00Z", 1.104),
        ("2026-01-07T10:00:00Z", 1.106),
        ("2026-01-07T14:00:00Z", 1.112),
    ];
    for (boundary, close) in released_closes {
        close_h4_with_runtime_ticks(&mut r, boundary, close);
    }
    let admitted = r.swing_rules("EURUSD").unwrap().trades();
    assert_eq!(
        admitted.len(),
        1,
        "the later signal is admitted after release"
    );
    assert_eq!(admitted[0].kind, Kind::TrendPullback);
    let final_close = chrono::DateTime::parse_from_rfc3339("2026-01-07T14:00:00Z")
        .unwrap()
        .timestamp();
    assert_eq!(
        admitted[0].signal_t,
        forex_core::swing::grid_time(final_close)
    );
    std::fs::remove_file(p).ok();
}

#[test]
fn warm_up_does_not_republish_a_trade_already_retired_from_the_old_family() {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    r.swing("EURUSD", config(), || Ok(planned())).unwrap();
    gap_ticks(&mut r);
    let warm = r.swing_rules("EURUSD").unwrap().clone();
    assert_eq!(count(&r, "Сделка открыта"), 1);

    let mut changed = config();
    changed["cost"] = json!(0.0002);
    r.swing("EURUSD", changed, || Ok(warm)).unwrap();
    assert_eq!(
        count(&r, "Сделка открыта"),
        1,
        "warm-up duplicate is not republished"
    );
    let old_trade = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value["markets"]["EURUSD"]["swing"]["retired"][0]["rules"]["trades"][0]
        .clone();
    let stop = old_trade["stop"].as_f64().unwrap() - 0.0001;
    r.tick(tick("2026-01-05T22:30:00Z", stop), true).unwrap();
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

fn terminal_signal_warmup_case(deliver_terminal: bool) {
    let p = path();
    let warm_active;
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks(&mut r);
        let active = r.swing_rules("EURUSD").unwrap().clone();
        assert_eq!(active.trades().len(), 1);
        assert_eq!(
            active.trades()[0].state,
            forex_core::swing::TradeState::Active
        );
        warm_active = active;

        // Record the original publication so a terminal-state warm-up cannot
        // treat the stable signal ID as new after the position leaves `published`.
        acknowledge_pending(&mut r);
        let trade = warm_active.trades()[0].clone();
        r.tick(tick("2026-01-05T22:30:00Z", trade.stop - 0.0001), true)
            .unwrap();
        let stop = r
            .pending()
            .unwrap()
            .into_iter()
            .find(|message| message.payload["text"].as_str().unwrap().contains("Стоп"))
            .expect("stop event must be queued");
        if deliver_terminal {
            r.delivered(&stop.id, 60_001).unwrap();
        }
    }

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    let queue_before = pending_state(&r);
    let mut changed = config();
    changed["cost"] = json!(0.0002);
    r.swing("EURUSD", changed, || Ok(warm_active)).unwrap();
    assert_eq!(
        pending_state(&r),
        queue_before,
        "reconfigure must preserve the outbox without re-emitting a registered terminal ID"
    );
    assert!(
        r.swing_rules("EURUSD").unwrap().trades().is_empty(),
        "registered terminal signal must not return to current or retired monitoring"
    );
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(
        pending_state(&r),
        queue_before,
        "restart preserves the exact queue"
    );
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    std::fs::remove_file(p).ok();
}

#[test]
fn delivered_terminal_signal_is_not_republished_or_reactivated_after_restart_and_reconfigure() {
    terminal_signal_warmup_case(true);
}

#[test]
fn pending_terminal_signal_is_not_republished_or_reactivated_and_outbox_is_unchanged() {
    terminal_signal_warmup_case(false);
}

#[test]
fn pending_open_and_stop_survive_restart_reconfigure_without_republication() {
    let p = path();
    let warm_active;
    let queued_before;
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks(&mut r);
        warm_active = r.swing_rules("EURUSD").unwrap().clone();
        let opening_queue = pending_state(&r);
        assert_eq!(opening_queue.len(), 1);
        assert!(opening_queue[0].1["text"]
            .as_str()
            .unwrap()
            .contains("Сделка открыта"));
        assert!(opening_queue[0].1["signal_id"].is_string());

        let store = forex_bot::state::Store::open(&p).unwrap();
        let snapshot = store.entities(forex_bot::state::Kind::Runtime).unwrap()[0]
            .value
            .clone();
        assert_eq!(snapshot["known"].as_object().unwrap().len(), 0);
        drop(store);

        let trade = warm_active.trades()[0].clone();
        r.tick(tick("2026-01-05T22:30:00Z", trade.stop - 0.0001), true)
            .unwrap();
        queued_before = pending_state(&r);
        assert_eq!(
            queued_before.len(),
            2,
            "both opening and stop remain pending"
        );
        let signal_ids = queued_before
            .iter()
            .map(|(_, payload)| payload["signal_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            signal_ids.len(),
            1,
            "both events refer to the same stable ID"
        );
        assert!(queued_before
            .iter()
            .any(|(_, payload)| payload["text"].as_str().unwrap().contains("Стоп")));

        let snapshot = forex_bot::state::Store::open(&p)
            .unwrap()
            .entities(forex_bot::state::Kind::Runtime)
            .unwrap()[0]
            .value
            .clone();
        assert_eq!(snapshot["known"].as_object().unwrap().len(), 0);
    }

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(
        pending_state(&r),
        queued_before,
        "restart keeps both undelivered events"
    );
    let mut changed = config();
    changed["cost"] = json!(0.0002);
    r.swing("EURUSD", changed, || Ok(warm_active)).unwrap();
    assert_eq!(
        pending_state(&r),
        queued_before,
        "reconfigure must not republish"
    );
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    let snapshot = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    assert_eq!(snapshot["known"].as_object().unwrap().len(), 0);
    assert_eq!(
        snapshot["markets"]["EURUSD"]["swing"]["retired"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(
        pending_state(&r),
        queued_before,
        "second restart preserves the exact outbox"
    );
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    std::fs::remove_file(p).ok();
}

#[test]
fn legacy_snapshot_without_retired_field_still_reads_and_manages_current_position() {
    let p = path();
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks(&mut r);
    }
    let mut store = forex_bot::state::Store::open(&p).unwrap();
    let mut snapshot = store.entities(forex_bot::state::Kind::Runtime).unwrap()[0]
        .value
        .clone();
    snapshot["markets"]["EURUSD"]["swing"]
        .as_object_mut()
        .unwrap()
        .remove("retired");
    store.save_runtime(&snapshot).unwrap();
    drop(store);

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert_eq!(
        r.swing_rules("EURUSD").unwrap().trades()[0].state,
        forex_core::swing::TradeState::Active
    );
    r.tick(tick("2026-01-05T22:30:00Z", 1.09000), true).unwrap();
    assert_eq!(count(&r, "Стоп"), 1);
    std::fs::remove_file(p).ok();
}

#[test]
fn retired_swing_exit_messages_use_actual_utc_quote_time_for_winter_and_summer() {
    for (day, expected) in [
        ("2026-01-05", "2026-01-05T22:30:00+00:00"),
        ("2026-07-06", "2026-07-06T22:30:00+00:00"),
    ] {
        let p = path();
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned())).unwrap();
        gap_ticks_on(&mut r, day);
        let old = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
        let next = trend_config();
        r.swing("EURUSD", next.clone(), || {
            let parsed = serde_json::from_value::<SwingConfig>(next).unwrap();
            Ok(Swing::new(parsed))
        })
        .unwrap();

        r.tick(tick(&format!("{day}T22:30:00Z"), old.stop - 0.0001), true)
            .unwrap();
        let stop = texts(&r)
            .into_iter()
            .find(|text| text.contains("Стоп"))
            .expect("retired stop event must be published");
        assert!(
            stop.contains(expected),
            "expected UTC {expected}, got: {stop}"
        );
        std::fs::remove_file(p).ok();
    }
}

#[test]
fn family_choice_is_per_instrument_and_legacy_json_defaults_to_swing() {
    let p = path();
    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    for symbol in ["EURUSD", "GBPUSD", "USDJPY"] {
        let mut config = instrument_config(symbol);
        if symbol == "GBPUSD" {
            config["family"] = json!("trend_pullback");
            config["trend_pullback"] = json!({"fast_ema_bars": 8, "slow_ema_bars": 32});
            config["atr_period"] = json!(14);
            config["stop_lookback_h4_bars"] = json!(10);
            config["stop_buffer_atr"] = json!(0.1);
            config["rr_fixed"] = json!(2.0);
            config["take_profit"] = json!("fixed");
        }
        let parsed = serde_json::from_value::<SwingConfig>(config.clone()).unwrap();
        parsed.validate().unwrap();
        r.swing(symbol, config, || Ok(Swing::new(parsed))).unwrap();
    }
    let snapshot = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    assert_eq!(
        snapshot["markets"]["GBPUSD"]["swing"]["config"]["family"],
        "trend_pullback"
    );
    assert_eq!(
        snapshot["markets"]["GBPUSD"]["swing"]["rules"]["cfg"]["family"],
        "trend_pullback"
    );
    for symbol in ["EURUSD", "USDJPY"] {
        assert!(snapshot["markets"][symbol]["swing"]["config"]
            .get("family")
            .is_none());
        assert_eq!(
            snapshot["markets"][symbol]["swing"]["rules"]["cfg"]["family"],
            "legacy_swing"
        );
    }
    std::fs::remove_file(p).ok();
}
#[test]
fn warm_up_replays_stored_minutes_through_the_bot_aggregator() {
    let p = path();
    drop(Runtime::open(&p, rules(), Engine::default()).unwrap());
    let db = rusqlite::Connection::open(&p).unwrap();
    let start = forex_core::prices::utc("2026-01-05T00:00:00Z").unwrap();
    for i in 0..(5 * 24 * 60) {
        let m = forex_core::prices::Minute {
            timestamp: start + i * 60,
            price: [1_100_000; 4],
            quotes: 1,
            gap_before_minutes: 0,
        };
        db.execute(
            "INSERT INTO minutes VALUES('EURUSD','mt5',?1,?2)",
            rusqlite::params![m.timestamp, serde_json::to_string(&m).unwrap()],
        )
        .unwrap();
    }
    let swing = forex_bot::service::swing_warmup(&p, "EURUSD", &config()).unwrap();
    assert!(swing.frame(Tf::H4).len() >= 29);
    assert!(swing.frame(Tf::D).len() >= 4);
    assert!(forex_bot::service::swing_warmup(&p, "GBPUSD", &config()).is_err());
    db.execute(
        "INSERT INTO minutes VALUES('EURUSD','other',?1,'{}')",
        rusqlite::params![start],
    )
    .unwrap();
    assert!(forex_bot::service::swing_warmup(&p, "EURUSD", &config()).is_err());
}
#[test]
fn strategy_file_sets_and_explains_every_parameter() {
    fn keys(v: &Value, prefix: &str, out: &mut Vec<String>) {
        for (k, x) in v.as_object().unwrap() {
            if x.is_object() {
                keys(x, &format!("{prefix}{k}."), out);
            } else {
                out.push(format!("{prefix}{k}"));
            }
        }
    }
    fn check(f: Value) {
        let mut used = BTreeSet::new();
        // The existing working JSON remains a valid all-legacy configuration.
        for symbol in ["EURUSD", "GBPUSD", "USDJPY"] {
            let c = &f["instruments"][symbol];
            let parsed: SwingConfig = serde_json::from_value(c.clone()).unwrap();
            parsed.validate().unwrap();
            let mut set = vec![];
            keys(c, "", &mut set);
            for key in set {
                assert!(f["comments"][&key].is_string(), "missing comment for {key}");
                used.insert(key);
            }
        }
        let explained: BTreeSet<String> =
            f["comments"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            used, explained,
            "comments must cover the union of used leaves"
        );

        // Family-specific required fields fail at deserialization or validation.
        for symbol in ["EURUSD", "GBPUSD", "USDJPY"] {
            let c = &f["instruments"][symbol];
            let mut set = vec![];
            keys(c, "", &mut set);
            for key in set {
                let mut short = c.clone();
                let path: Vec<&str> = key.split('.').collect();
                let mut node = &mut short;
                for p in &path[..path.len() - 1] {
                    node = &mut node[*p];
                }
                node.as_object_mut().unwrap().remove(path[path.len() - 1]);
                let result = serde_json::from_value::<SwingConfig>(short)
                    .map_err(|_| "deserialize")
                    .and_then(|parsed| parsed.validate().map_err(|_| "validate"));
                assert!(result.is_err(), "{symbol}: {key} must be required");
            }
        }
    }

    // Test local working JSON independently; behavioral legacy tests use the
    // pinned fixture above and cannot change when this file selects a family.
    check(working_file());
    check(legacy_fixture());

    for symbol in ["EURUSD", "GBPUSD"] {
        let mut mixed = legacy_fixture();
        mixed["instruments"][symbol] = trend_config();
        mixed["comments"]["family"] = json!("Selected strategy family for this instrument.");
        mixed["comments"]["trend_pullback.fast_ema_bars"] = json!("Fast H4 EMA period.");
        mixed["comments"]["trend_pullback.slow_ema_bars"] = json!("Slow H4 EMA period.");
        check(mixed);
    }
}

#[test]
fn retired_pending_and_active_positions_reserve_new_trend_cap_across_restart() {
    const HISTORY_START: i64 = 1_704_067_200;
    let p = path();
    let old_pending;
    let outbox_before;
    {
        let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
        r.swing("EURUSD", config(), || Ok(planned_limit())).unwrap();
        gap_ticks(&mut r);
        old_pending = r.swing_rules("EURUSD").unwrap().trades()[0].clone();
        assert_eq!(old_pending.state, TradeState::Pending);
        outbox_before = pending_state(&r);

        let cap_one = trend_config_with_limit(1, 0.0001);
        r.swing("EURUSD", cap_one.clone(), || {
            Ok(warm_trend(&cap_one, HISTORY_START))
        })
        .unwrap();
        assert!(
            r.swing_rules("EURUSD").unwrap().trades().is_empty(),
            "retired Pending order already owns the only position slot"
        );
        assert_eq!(pending_state(&r), outbox_before);
    }

    let mut r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    let cap_one_after_restart = trend_config_with_limit(1, 0.0002);
    r.swing("EURUSD", cap_one_after_restart.clone(), || {
        Ok(warm_trend(
            &cap_one_after_restart,
            HISTORY_START + 4 * 3_600,
        ))
    })
    .unwrap();
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    assert_eq!(pending_state(&r), outbox_before);

    r.tick(
        tick("2026-01-05T22:30:00Z", old_pending.entry - 0.0001),
        true,
    )
    .unwrap();
    let snapshot = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    let old_after_fill =
        &snapshot["markets"]["EURUSD"]["swing"]["retired"][0]["rules"]["trades"][0];
    assert_eq!(old_after_fill["state"], "Active");
    let old_cost = serde_json::from_value::<SwingConfig>(config())
        .unwrap()
        .cost;
    let close_float =
        |actual: &Value, expected: f64| (actual.as_f64().unwrap() - expected).abs() <= 1e-12;
    assert!(close_float(
        &old_after_fill["entry"],
        old_pending.entry + f64::from(old_pending.dir) * old_cost
    ));
    assert!(close_float(&old_after_fill["stop"], old_pending.stop));
    assert!(close_float(&old_after_fill["tp"], old_pending.tp));
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());

    // Pending -> Active remains one occupied slot: with a limit of two, one
    // new warm Active position can be admitted beside the old retired trade.
    let cap_two = trend_config_with_limit(2, 0.0003);
    r.swing("EURUSD", cap_two.clone(), || {
        Ok(warm_trend(&cap_two, HISTORY_START + 8 * 3_600))
    })
    .unwrap();
    assert_eq!(r.swing_rules("EURUSD").unwrap().trades().len(), 1);
    assert_eq!(
        r.swing_rules("EURUSD").unwrap().trades()[0].state,
        TradeState::Active
    );

    // Lowering a cap below already-occupied slots never cancels either trade.
    let cap_one_again = trend_config_with_limit(1, 0.0004);
    r.swing("EURUSD", cap_one_again.clone(), || {
        Ok(warm_trend(&cap_one_again, HISTORY_START + 12 * 3_600))
    })
    .unwrap();
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    let snapshot = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    let retired = snapshot["markets"]["EURUSD"]["swing"]["retired"]
        .as_array()
        .unwrap();
    let active = retired
        .iter()
        .flat_map(|old| old["rules"]["trades"].as_array().unwrap())
        .filter(|trade| trade["state"] == "Active")
        .count();
    assert_eq!(
        active, 2,
        "lowering cap does not force-close saved positions"
    );
    drop(r);

    let r = Runtime::open(&p, rules(), Engine::default()).unwrap();
    assert!(r.swing_rules("EURUSD").unwrap().trades().is_empty());
    let snapshot = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    let active_after_restart = snapshot["markets"]["EURUSD"]["swing"]["retired"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|old| old["rules"]["trades"].as_array().unwrap())
        .filter(|trade| trade["state"] == "Active")
        .count();
    assert_eq!(active_after_restart, 2);
    std::fs::remove_file(p).ok();
}
