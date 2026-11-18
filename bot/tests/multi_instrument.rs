use forex_bot::{
    runtime::Runtime,
    state::{Kind, Store},
};
use serde_json::{json, Value};
fn rules() -> Value {
    let v: Value = serde_json::from_str(include_str!("../../strategy/swing-rules.json")).unwrap();
    v
}
fn path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("multi-{}-{name}.sqlite", std::process::id()))
}
#[test]
fn restart_uses_supplied_json_not_database_rules() {
    let p = path("source");
    let r = Runtime::open(&p, rules()).unwrap();
    drop(r);
    let mut updated = rules();
    updated["instruments"]["EURUSD"]["stop_buffer_atr"] = json!(0.7);
    let r = Runtime::open(&p, updated.clone()).unwrap();
    assert_eq!(r.rules(), updated);
}
#[test]
fn snapshot_does_not_store_rules() {
    let p = path("no-rules");
    drop(Runtime::open(&p, rules()).unwrap());
    let s = Store::open(&p).unwrap();
    assert!(s.entities(Kind::Runtime).unwrap()[0]
        .value
        .get("rules")
        .is_none());
}
#[test]
fn configuration_accepts_multiple_pairs() {
    let v = rules();
    assert_eq!(v["instruments"].as_object().unwrap().len(), 3);
    let c = serde_json::from_value(v).unwrap();
    assert!(forex_bot::validate(&c).is_ok());
}
fn fixture_tick(symbol: &str, price: i64) -> forex_bot::feed::Tick {
    forex_bot::feed::Tick {
        symbol: symbol.into(),
        millis: 1_794_225_600_000,
        price,
        source: "upscale:index".into(),
    }
}
#[test]
fn aggregator_accepts_gbp_and_jpy_prices() {
    for (symbol, price) in [("GBPUSD", 1_250_000), ("USDJPY", 153_123_000)] {
        let mut a = forex_bot::feed::Aggregator::default();
        a.push(fixture_tick(symbol, price)).unwrap();
        let snapshot = serde_json::to_value(a).unwrap();
        assert_eq!(snapshot["last"]["symbol"], symbol);
        assert_eq!(snapshot["last"]["price"], price);
    }
}
#[test]
fn different_symbols_are_not_duplicate_quotes() {
    let p = path("isolation");
    let mut r = Runtime::open(&p, rules()).unwrap();
    for (symbol, price) in [
        ("EURUSD", 1_100_000),
        ("GBPUSD", 1_300_000),
        ("USDJPY", 153_123_000),
    ] {
        let t = fixture_tick(symbol, price);
        r.tick(t, true).unwrap();
    }
    drop(r);
    let s = Store::open(&p).unwrap();
    let entries = s.entities(Kind::Runtime).unwrap();
    let markets = &entries[0].value["markets"];
    assert_eq!(markets.as_object().unwrap().len(), 3);
    assert_ne!(
        markets["EURUSD"]["aggregator"]["last"]["price"],
        markets["USDJPY"]["aggregator"]["last"]["price"]
    );
}
fn file_runtime(name: &str) -> (std::path::PathBuf, std::path::PathBuf, Runtime) {
    let db = path(name);
    let file = db.with_extension("json");
    std::fs::write(&file, rules().to_string()).unwrap();
    let mut r = Runtime::open_file(&db, &file).unwrap();
    r.link(9, "EURUSD:swing:trend:1:1767600000").unwrap();
    r.update(&json!({"update_id":1,"message":{"message_id":10,"chat":{"id":-1},"from":{"id":1},"text":"buffer","reply_to_message":{"message_id":9}}}),-1).unwrap();
    (db, file, r)
}
fn change() -> forex_bot::protocols::Analysis {
    forex_bot::protocols::Analysis {
        action: "apply".into(),
        explanation: "changed".into(),
        changes: vec![forex_bot::protocols::Change {
            path: "/instruments/EURUSD/stop_buffer_atr".into(),
            value: json!(0.7),
        }],
    }
}
#[test]
fn file_write_failure_keeps_rules_queue_and_outbox() {
    let (_db, file, mut r) = file_runtime("write-fail");
    let old = r.rules();
    std::fs::create_dir(file.with_extension("json.pending")).unwrap();
    assert!(r.analysis(1, &change(), &json!({})).is_err());
    assert_eq!(r.rules(), old);
    assert!(r.next_comment().is_some());
    assert!(r.pending().unwrap().is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(file).unwrap()).unwrap(),
        old
    );
}
#[test]
fn applied_change_is_in_json_and_never_in_snapshot_or_event_patch() {
    let (db, file, mut r) = file_runtime("apply");
    r.analysis(1, &change(), &json!({})).unwrap();
    drop(r);
    let r = Runtime::open_file(&db, &file).unwrap();
    assert_eq!(r.rules()["instruments"]["EURUSD"]["stop_buffer_atr"], 0.7);
    assert!(r.next_comment().is_none());
    let db = rusqlite::Connection::open(db).unwrap();
    let patch: String = db
        .query_row("SELECT payload FROM inbox WHERE id='analysis:1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!patch.contains("changes"));
}
#[test]
fn corrupted_or_missing_file_never_falls_back_to_snapshot() {
    let (db, file, r) = file_runtime("invalid-file");
    drop(r);
    std::fs::write(&file, "broken").unwrap();
    assert!(Runtime::open_file(&db, &file).is_err());
    std::fs::remove_file(&file).unwrap();
    assert!(Runtime::open_file(&db, &file).is_err());
}
#[test]
fn interruption_before_file_write_retains_comment() {
    let (db, file, r) = file_runtime("before-file");
    drop(r);
    let mut store = Store::open(&db).unwrap();
    let mut snap = store.entities(Kind::Runtime).unwrap()[0].value.clone();
    use sha2::{Digest, Sha256};
    let current: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    snap["operation"] = json!({"id":1,"before":format!("{:x}",Sha256::digest(current.to_string().as_bytes())),"after":"not-written"});
    store.save_runtime(&snap).unwrap();
    drop(store);
    let r = Runtime::open_file(&db, &file).unwrap();
    assert!(
        r.next_comment().is_some(),
        "unwritten change must remain queued"
    );
}
#[test]
fn interruption_after_file_write_uses_json_once_without_patch() {
    let (db, file, r) = file_runtime("after-file");
    let before = r.rules();
    drop(r);
    let mut after = before.clone();
    after["instruments"]["EURUSD"]["stop_buffer_atr"] = json!(0.7);
    use sha2::{Digest, Sha256};
    let hash = |v: &Value| format!("{:x}", Sha256::digest(v.to_string().as_bytes()));
    let mut store = Store::open(&db).unwrap();
    let mut snap = store.entities(Kind::Runtime).unwrap()[0].value.clone();
    snap["operation"] = json!({"id":1,"before":hash(&before),"after":hash(&after)});
    store.save_runtime(&snap).unwrap();
    drop(store);
    std::fs::write(&file, after.to_string()).unwrap();
    let r = Runtime::open_file(&db, &file).unwrap();
    assert_eq!(r.rules(), after);
    assert!(r.next_comment().is_none());
    assert_eq!(r.pending().unwrap().len(), 1);
    drop(r);
    let r = Runtime::open_file(&db, &file).unwrap();
    assert_eq!(r.pending().unwrap().len(), 1);
}
#[test]
fn previous_database_is_not_rewritten_on_open() {
    let p = path("previous-schema");
    let c = rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("PRAGMA application_id=1180190513; PRAGMA user_version=3; CREATE TABLE minutes(payload TEXT); INSERT INTO minutes VALUES('original-bytes');").unwrap();
    drop(c);
    let before = std::fs::read(&p).unwrap();
    assert!(forex_bot::data::open(&p).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), before);
}
fn active_swing(config: &Value, entry: f64, stop: f64, tp: f64) -> forex_core::swing::Swing {
    let swing = forex_core::swing::Swing::new(serde_json::from_value(config.clone()).unwrap());
    let mut saved = serde_json::to_value(swing).unwrap();
    saved["trades"] = json!([{
        "kind":"Trend", "dir":1, "entry":entry, "stop":stop, "tp":tp,
        "rr":(tp-entry)/(entry-stop), "signal_t":1_767_600_000, "fill_t":1_767_600_000,
        "entry_kind":"market", "state":"Active", "r":null, "exit_t":null,
        "plan":null, "range_box":null
    }]);
    serde_json::from_value(saved).unwrap()
}
#[test]
fn quotes_only_track_their_instrument_and_repeat_safely_after_restart() {
    let p = path("separate-signals");
    let v = rules();
    let mut r = Runtime::open(&p, v.clone()).unwrap();
    for (symbol, entry, stop, tp) in [
        ("EURUSD", 1.1, 1.09, 1.13),
        ("GBPUSD", 1.3, 1.29, 1.33),
        ("USDJPY", 153.0, 152.0, 155.0),
    ] {
        let config = v["instruments"][symbol].clone();
        r.swing(symbol, config.clone(), || {
            Ok(active_swing(&config, entry, stop, tp))
        })
        .unwrap();
        r.tick(fixture_tick(symbol, (entry * 1_000_000.0) as i64), false)
            .unwrap();
    }
    let mut stop = fixture_tick("USDJPY", 151_900_000);
    stop.millis += 1000;
    r.tick(stop.clone(), false).unwrap();
    assert!(r.swing_rules("USDJPY").unwrap().trades().is_empty());
    for symbol in ["EURUSD", "GBPUSD"] {
        assert_eq!(
            r.signal_context(&format!("{symbol}:swing:trend:1:1767600000"))["trade"]["state"],
            "Active"
        );
    }
    let pending = r.pending().unwrap();
    let stopped: Vec<_> = pending
        .iter()
        .filter(|m| m.payload["text"].as_str().unwrap().contains("Стоп"))
        .collect();
    assert_eq!(stopped.len(), 1);
    assert!(stopped[0].payload["text"]
        .as_str()
        .unwrap()
        .starts_with("USD/JPY"));
    drop(r);
    let mut r = Runtime::open(&p, rules()).unwrap();
    r.tick(stop, false).unwrap();
    let after = r.pending().unwrap();
    assert_eq!(after.len(), pending.len());
    for (expected, actual) in pending.iter().zip(after) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.payload, expected.payload);
    }
}
#[test]
fn symbols_are_configurable_but_malformed_names_and_unknown_tick_symbols_fail() {
    let mut v = rules();
    let config = v["instruments"]
        .as_object_mut()
        .unwrap()
        .remove("EURUSD")
        .unwrap();
    v["instruments"]["AUDCAD"] = config.clone();
    assert!(forex_bot::validate(&serde_json::from_value(v.clone()).unwrap()).is_ok());
    for bad in ["audcad", "../ABC", "AUD"] {
        let mut b = v.clone();
        b["instruments"].as_object_mut().unwrap().remove("AUDCAD");
        b["instruments"][bad] = config.clone();
        assert!(forex_bot::validate(&serde_json::from_value(b).unwrap()).is_err());
    }
    let mut empty = v.clone();
    empty["instruments"] = json!({});
    assert!(forex_bot::validate(&serde_json::from_value(empty).unwrap()).is_err());
    let mut r = Runtime::open(&path("unknown-symbol"), v).unwrap();
    assert!(r.tick(fixture_tick("EURUSD", 1_100_000), false).is_err());
    r.tick(fixture_tick("AUDCAD", 900_000), false).unwrap();
}
#[test]
fn telegram_rule_change_preserves_published_levels_and_other_pair_contexts() {
    let db = path("all-rules");
    let file = db.with_extension("json");
    let v = rules();
    std::fs::write(&file, v.to_string()).unwrap();
    let mut r = Runtime::open_file(&db, &file).unwrap();
    let mut originals = Vec::new();
    for (symbol, entry, stop, tp) in [
        ("EURUSD", 1.1, 1.09, 1.13),
        ("GBPUSD", 1.3, 1.29, 1.33),
        ("USDJPY", 153.0, 152.0, 155.0),
    ] {
        let config = v["instruments"][symbol].clone();
        r.swing(symbol, config.clone(), || {
            Ok(active_swing(&config, entry, stop, tp))
        })
        .unwrap();
        let id = format!("{symbol}:swing:trend:1:1767600000");
        originals.push((id.clone(), r.signal_context(&id)));
    }
    r.link(9, &originals[0].0).unwrap();
    r.update(&json!({"update_id":1,"message":{"message_id":10,"chat":{"id":-1},"from":{"id":1},"text":"buffer","reply_to_message":{"message_id":9}}}),-1).unwrap();
    r.analysis(1, &change(), &json!({})).unwrap();
    let revised = r.rules()["instruments"]["EURUSD"].clone();
    r.swing("EURUSD", revised.clone(), || {
        Ok(forex_core::swing::Swing::new(
            serde_json::from_value(revised).unwrap(),
        ))
    })
    .unwrap();
    for symbol in ["GBPUSD", "USDJPY"] {
        r.swing(symbol, v["instruments"][symbol].clone(), || {
            Err("unchanged pair must not restart".into())
        })
        .unwrap();
    }
    let out = r.pending().unwrap();
    assert_eq!(
        out.len(),
        4,
        "three original trades and one analysis reply only"
    );
    assert!(out.iter().all(|m| m.payload.get("revision").is_none()));
    for (id, context) in &originals {
        assert_eq!(r.signal_context(id), *context);
    }
    drop(r);
    let r = Runtime::open_file(&db, &file).unwrap();
    for (id, context) in originals {
        assert_eq!(r.signal_context(&id), context);
    }
    assert_eq!(r.pending().unwrap().len(), 4);
}
#[test]
fn swing_message_uses_its_actual_pair() {
    let v = rules();
    let swing = active_swing(&v["instruments"]["EURUSD"], 1.1, 1.09, 1.13);
    for (symbol, label) in [
        ("EURUSD", "EUR/USD"),
        ("GBPUSD", "GBP/USD"),
        ("USDJPY", "USD/JPY"),
        ("AUDCAD", "AUD/CAD"),
    ] {
        let text =
            forex_bot::runtime::swing_text(symbol, &swing.trades()[0], "Сделка открыта", None);
        assert!(text.starts_with(label));
    }
}
