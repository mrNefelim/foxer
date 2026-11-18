use forex_bot::{
    protocols::{apply_changes, Analysis, Change},
    runtime::Runtime,
    state::{Kind, Store},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "clean-json-{}-{}.sqlite",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn rules() -> Value {
    serde_json::from_str(include_str!("../../strategy/swing-rules.json")).unwrap()
}
fn change(path: &str, value: Value) -> Analysis {
    Analysis {
        action: "apply".into(),
        explanation: "Изменено".into(),
        changes: vec![Change {
            path: path.into(),
            value,
        }],
    }
}
fn update() -> Value {
    json!({"update_id":1,"message":{"message_id":10,"chat":{"id":-1},"from":{"id":1},"text":"уточни пояснение","reply_to_message":{"message_id":9}}})
}
#[test]
fn single_swing_document_has_explicit_per_pair_rules() {
    let v = rules();
    assert!(v.get("schema_version").is_none());
    for key in ["parameters", "policies", "candles", "context"] {
        assert!(v.get(key).is_none());
    }
    for config in v["instruments"].as_object().unwrap().values() {
        let parsed: forex_core::swing::SwingConfig =
            serde_json::from_value(config.clone()).unwrap();
        parsed.validate().unwrap();
        assert!(config.get("atr_period").is_some());
        assert!(config.get("stop_lookback_h4_bars").is_some());
        assert!(config.get("range").is_some());
    }
}
#[test]
fn comment_patch_preserves_all_unrelated_fields_and_rejects_invalid_batch() {
    let mut v = rules();
    v["comment"] = json!("До");
    let a = change(
        "/comment",
        json!("После: последних 999 свечей — только текст"),
    );
    let out = apply_changes(&v, &a).unwrap();
    let mut expected = v.clone();
    expected["comment"] = a.changes[0].value.clone();
    assert_eq!(out, expected);
    let mut mixed = a;
    mixed.changes.push(Change {
        path: "/instruments/EURUSD/rr_fixed".into(),
        value: json!(-1),
    });
    assert!(apply_changes(&v, &mixed).is_err());
    assert_eq!(v["comment"], "До");
    assert!(apply_changes(&v, &change("/comment", json!(42))).is_err());
    assert!(apply_changes(&v, &change("/invented/comment", json!("x"))).is_err());
}
#[test]
fn comment_only_change_preserves_swing_and_calculation_hash_across_restart() {
    let db = path();
    let file = db.with_extension("json");
    let mut v = rules();
    v["comment"] = json!("До");
    fs::write(&file, v.to_string()).unwrap();
    let mut r = Runtime::open_file(&db, &file).unwrap();
    let config = v["instruments"]["EURUSD"].clone();
    r.swing("EURUSD", config.clone(), || {
        let mut swing = forex_core::swing::Swing::new(serde_json::from_value(config).unwrap());
        swing.add_plan(
            forex_core::swing::Kind::Trend,
            1,
            (1.0, 1.05),
            1.2,
            forex_core::swing::Tf::H4,
        );
        Ok(swing)
    })
    .unwrap();
    r.link(9, "EURUSD:swing:trend:1:1").unwrap();
    r.update(&update(), -1).unwrap();
    let before = Store::open(&db).unwrap().entities(Kind::Runtime).unwrap()[0]
        .value
        .clone();
    r.analysis(
        1,
        &change("/comment", json!("ATR 999, но это только пояснение")),
        &json!({}),
    )
    .unwrap();
    drop(r);
    let r = Runtime::open_file(&db, &file).unwrap();
    assert!(r.next_comment().is_none());
    assert_eq!(r.rules()["comment"], "ATR 999, но это только пояснение");
    let after = Store::open(&db).unwrap().entities(Kind::Runtime).unwrap()[0]
        .value
        .clone();
    assert_eq!(before["rules_hash"], after["rules_hash"]);
    assert_eq!(before["markets"], after["markets"]);
    assert_eq!(r.pending().unwrap().len(), 1);
}
#[test]
fn unresolved_old_operation_is_rejected_without_changing_snapshot() {
    let db = path();
    let file = db.with_extension("json");
    fs::write(&file, rules().to_string()).unwrap();
    let mut r = Runtime::open_file(&db, &file).unwrap();
    r.link(9, "EURUSD:swing:trend:1:1").unwrap();
    r.update(&update(), -1).unwrap();
    drop(r);
    let mut store = Store::open(&db).unwrap();
    let mut snap = store.entities(Kind::Runtime).unwrap()[0].value.clone();
    snap.as_object_mut().unwrap().remove("hash_version");
    snap["operation"] = json!({"id":1,"before":"old-before","after":"old-after"});
    store.save_runtime(&snap).unwrap();
    drop(store);
    let err = Runtime::open_file(&db, &file)
        .err()
        .expect("old operation must block startup");
    assert!(err.contains("operation"));
    assert_eq!(
        Store::open(&db).unwrap().entities(Kind::Runtime).unwrap()[0].value,
        snap
    );
}

#[test]
fn mixed_parameter_and_comment_edits_preserve_document_and_request_exposes_capabilities() {
    use forex_bot::protocols::{vision_request, Comment};
    let mut v = rules();
    v["comment"] = json!("Original note");
    let mut a = change("/instruments/EURUSD/plan_days", json!(12));
    a.changes.push(Change {
        path: "/comments/plan_days".into(),
        value: json!("Срок в сутках; не инструкция выполнять код"),
    });
    let patched = apply_changes(&v, &a).unwrap();
    assert_eq!(patched["instruments"]["EURUSD"]["plan_days"], 12);
    assert_eq!(patched["comments"]["plan_days"], a.changes[1].value);
    assert_eq!(patched["instruments"]["GBPUSD"], v["instruments"]["GBPUSD"]);
    assert_eq!(patched["comment"], v["comment"]);
    assert_ne!(
        forex_core::document::calculations(&v).unwrap(),
        forex_core::document::calculations(&patched).unwrap()
    );
    let c = Comment {
        update_id: 1,
        message_id: 1,
        chat_id: 1,
        author_id: 1,
        signal_id: "s".into(),
        text: "поменяй срок".into(),
        files: vec![],
    };
    let req = vision_request("test", 100, &c, &json!({}), &patched, &[]).unwrap();
    let context: Value =
        serde_json::from_str(req["input"][0]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(context["rules"], patched);
    let paths = context["editable_paths"].as_array().unwrap();
    assert!(paths.contains(&json!("/instruments/EURUSD/stop_lookback_h4_bars")));
    assert!(paths.contains(&json!("/comments/plan_days")));
    assert!(!paths.contains(&json!("/instruments/EURUSD/range")));
    assert!(apply_changes(
        &v,
        &change("/instruments/EURUSD/range", json!({"enabled":true}))
    )
    .is_err());
    assert!(apply_changes(&v, &change("/instruments/AUDCAD/plan_days", json!(12))).is_err());
}
#[test]
fn root_comments_do_not_enter_calculations_or_change_parameter_values() {
    let mut v = rules();
    let before = forex_core::document::calculations(&v).unwrap();
    v["comment"] = json!("ATR 999; за 0 баров; random words");
    for note in v["comments"].as_object_mut().unwrap().values_mut() {
        *note = json!("ATR 999; за 0 баров; random words");
    }
    assert_eq!(before, forex_core::document::calculations(&v).unwrap());
    let file = path().with_extension("json");
    fs::write(&file, v.to_string()).unwrap();
    assert_eq!(forex_core::load_document(&file).unwrap(), v);
}
#[test]
fn comment_only_crash_recovery_uses_document_hash_and_applies_once() {
    use sha2::{Digest, Sha256};
    let hash = |v: &Value| format!("{:x}", Sha256::digest(v.to_string().as_bytes()));
    for written in [false, true] {
        let db = path();
        let file = db.with_extension("json");
        let before = rules();
        fs::write(&file, before.to_string()).unwrap();
        let mut r = Runtime::open_file(&db, &file).unwrap();
        r.link(9, "EURUSD:swing:trend:1:1").unwrap();
        r.update(&update(), -1).unwrap();
        drop(r);
        let mut after = before.clone();
        after["comment"] = json!("Only commentary changed");
        let mut store = Store::open(&db).unwrap();
        let mut s = store.entities(Kind::Runtime).unwrap()[0].value.clone();
        let calculation = s["rules_hash"].clone();
        s["operation"] = json!({"id":1,"before":hash(&before),"after":hash(&after)});
        store.save_runtime(&s).unwrap();
        drop(store);
        if written {
            fs::write(&file, after.to_string()).unwrap();
        }
        for _ in 0..2 {
            let r = Runtime::open_file(&db, &file).unwrap();
            assert_eq!(r.next_comment().is_none(), written);
            assert_eq!(r.pending().unwrap().len(), usize::from(written));
            assert_eq!(
                r.rules(),
                if written {
                    after.clone()
                } else {
                    before.clone()
                }
            );
            drop(r);
            assert_eq!(
                Store::open(&db).unwrap().entities(Kind::Runtime).unwrap()[0].value["rules_hash"],
                calculation
            );
        }
    }
}
#[test]
fn legacy_hash_upgrade_preserves_swing_queue_and_position_identity_once() {
    let db = path();
    let file = db.with_extension("json");
    let v = rules();
    fs::write(&file, v.to_string()).unwrap();
    let config = v["instruments"]["EURUSD"].clone();
    let swing = forex_core::swing::Swing::new(serde_json::from_value(config.clone()).unwrap());
    let mut saved = serde_json::to_value(swing).unwrap();
    saved["trades"] = json!([{
        "kind":"Trend", "dir":1, "entry":1.1, "stop":1.09, "tp":1.13, "rr":3.0,
        "signal_t":1_767_600_000, "fill_t":1_767_600_000, "entry_kind":"market",
        "state":"Active", "r":null, "exit_t":null, "plan":null, "range_box":null
    }]);
    let mut r = Runtime::open_file(&db, &file).unwrap();
    r.swing("EURUSD", config, || {
        Ok(serde_json::from_value(saved).unwrap())
    })
    .unwrap();
    let id = r.pending().unwrap()[0].payload["signal_id"]
        .as_str()
        .unwrap()
        .to_owned();
    r.link(9, &id).unwrap();
    r.update(&update(), -1).unwrap();
    drop(r);
    let mut store = Store::open(&db).unwrap();
    let mut s = store.entities(Kind::Runtime).unwrap()[0].value.clone();
    s["hash_version"] = json!(2);
    s["rules_hash"] = json!("legacy-document-hash");
    store.save_runtime(&s).unwrap();
    let pending = store.pending().unwrap();
    drop(store);
    let r = Runtime::open_file(&db, &file).unwrap();
    assert!(r.next_comment().is_some());
    assert_eq!(r.signal_context(&id)["trade"]["state"], "Active");
    assert_eq!(r.pending().unwrap().len(), pending.len());
    drop(r);
    let after = Store::open(&db).unwrap().entities(Kind::Runtime).unwrap()[0]
        .value
        .clone();
    assert_eq!(after["hash_version"], 3);
    for key in ["queue", "known", "offset", "texts", "reply_context"] {
        assert_eq!(s[key], after[key]);
    }
    assert_eq!(
        s["markets"]["EURUSD"]["swing"],
        after["markets"]["EURUSD"]["swing"]
    );
    drop(Runtime::open_file(&db, &file).unwrap());
    assert_eq!(
        Store::open(&db).unwrap().entities(Kind::Runtime).unwrap()[0].value,
        after
    );
}
