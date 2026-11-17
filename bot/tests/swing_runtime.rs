//! Swing rules in the live runtime (requirements: Правила «свинг» в работе бота).
use forex_bot::{engine::Engine, feed::Tick, runtime::Runtime};
use forex_core::swing::{Kind, Swing, SwingConfig, Tf};
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
fn file() -> Value {
    serde_json::from_str(include_str!("../../strategy/swing-rules.json")).unwrap()
}
fn config() -> Value {
    file()["instruments"]["EURUSD"].clone()
}
/// Rules with one long trend plan confirmed and entered on an H4 gap.
fn planned() -> Swing {
    let mut s = Swing::new(serde_json::from_value::<SwingConfig>(config()).unwrap());
    s.add_plan(Kind::Trend, 1, (1.0, 1.05), 1.2, Tf::H4);
    s
}
fn planned_limit() -> Swing {
    let mut s = Swing::new(serde_json::from_value::<SwingConfig>(config()).unwrap());
    s.add_plan(Kind::Trend, 1, (1.0, 1.05), 1.105, Tf::H4);
    s
}
fn tick(at: &str, price: f64) -> Tick {
    Tick {
        symbol: "EURUSD".into(),
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
    let f = file();
    let mut explained: Vec<String> = f["comments"].as_object().unwrap().keys().cloned().collect();
    explained.sort();
    for symbol in ["EURUSD", "GBPUSD", "USDJPY"] {
        let c = &f["instruments"][symbol];
        serde_json::from_value::<SwingConfig>(c.clone()).unwrap();
        let mut set = vec![];
        keys(c, "", &mut set);
        set.sort();
        assert_eq!(set, explained, "{symbol}");
        for key in &set {
            let mut short = c.clone();
            let path: Vec<&str> = key.split('.').collect();
            let mut node = &mut short;
            for p in &path[..path.len() - 1] {
                node = &mut node[*p];
            }
            node.as_object_mut().unwrap().remove(path[path.len() - 1]);
            assert!(
                serde_json::from_value::<SwingConfig>(short).is_err(),
                "{symbol}: {key} must be required"
            );
        }
    }
}
