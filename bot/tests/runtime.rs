use forex_bot::{
    engine::Engine,
    protocols::{Analysis, Change},
    runtime::Runtime,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "forex-runtime-{}-{}.sqlite",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn rules() -> Value {
    let v: Value = serde_json::from_str(include_str!("../../strategy/fvg-bot-rules.json")).unwrap();
    v
}
fn open_test(p: &std::path::Path) -> Result<Runtime, String> {
    let file = p.with_extension("json");
    if !file.exists() {
        std::fs::write(&file, rules().to_string()).unwrap();
    }
    Runtime::open_file(p, &file, Engine::default())
}
fn update(id: i64) -> Value {
    json!({"update_id":id,"message":{"message_id":id+100,"chat":{"id":-1},"from":{"id":if id%2==0 {22}else{11}},"text":"change stop","reply_to_message":{"message_id":9}}})
}
fn analysis(v: f64) -> Analysis {
    Analysis {
        action: "apply".into(),
        explanation: "Буфер стопа изменён".into(),
        changes: vec![Change {
            path: "/parameters/stop_buffer_atr/value".into(),
            value: json!(v),
        }],
    }
}
fn setup() -> (PathBuf, Runtime) {
    let p = path();
    let file = p.with_extension("json");
    std::fs::write(&file, rules().to_string()).unwrap();
    let mut r = Runtime::open_file(&p, &file, Engine::default()).unwrap();
    r.link(9, "signal-1").unwrap();
    (p, r)
}

#[test]
fn rule_change_reports_waiting_conditions_with_plan_identity_and_lifetime() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Side, Zone, ZoneKind},
    };

    let p = path();
    let file = p.with_extension("json");
    std::fs::write(&file, rules().to_string()).unwrap();
    let mut engine = Engine::default();
    engine.add_plan(
        "D_correction_end",
        Side::Long,
        Timeframe::D,
        Zone::new(Side::Long, 1_000_000, 1_010_000, 1, ZoneKind::Fvg),
        1_700_000_000,
    );
    let mut runtime = Runtime::open_file(&p, &file, engine).unwrap();
    runtime.link(9, "unrelated-comment-signal").unwrap();
    runtime.update(&update(1), -1).unwrap();
    let initial = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    let before = &initial["markets"]["EURUSD"]["engine"]["plans"][0];
    let plan_id = before["id"].as_str().unwrap().to_owned();
    let started = before["started"].as_i64().unwrap();

    runtime.analysis(1, &analysis(0.2), &json!({})).unwrap();
    let out = runtime.pending().unwrap();
    let texts: Vec<_> = out
        .iter()
        .map(|message| message.payload["text"].as_str().unwrap())
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text.contains("Условия плана") && text.contains(&plan_id)),
        "the updated waiting conditions and plan ID must be reported: {texts:?}"
    );
    assert!(out
        .iter()
        .all(|message| message.payload.get("revision").is_none()));

    let updated = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    let after = &updated["markets"]["EURUSD"]["engine"]["plans"][0];
    assert_eq!(after["id"], plan_id);
    assert_eq!(after["started"], started);
    assert_eq!(after["state"], "WaitEntry");

    drop(runtime);
    let runtime = Runtime::open_file(&p, &file, Engine::default()).unwrap();
    let repeated: Vec<_> = runtime.pending().unwrap();
    assert_eq!(repeated.len(), out.len());
    assert!(repeated
        .iter()
        .any(|message| message.payload["text"].as_str().unwrap().contains(&plan_id)));
    std::fs::remove_file(p).ok();
    std::fs::remove_file(file).ok();
}

#[test]
fn repeated_rule_cycles_have_distinct_condition_notifications_after_delivery() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Side, Zone, ZoneKind},
    };
    use std::collections::BTreeSet;

    let p = path();
    let file = p.with_extension("json");
    std::fs::write(&file, rules().to_string()).unwrap();
    let mut engine = Engine::default();
    engine.add_plan(
        "D_correction_end",
        Side::Long,
        Timeframe::D,
        Zone::new(Side::Long, 1_000_000, 1_010_000, 1, ZoneKind::Fvg),
        1_700_000_000,
    );
    let mut runtime = Runtime::open_file(&p, &file, engine).unwrap();
    runtime.link(9, "cycle-plan-signal").unwrap();
    let plan_id = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value["markets"]["EURUSD"]["engine"]["plans"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut seen_condition_ids = BTreeSet::new();
    for (update_id, value) in [(1, 0.2), (2, 0.1), (3, 0.2), (4, 0.1)] {
        runtime.update(&update(update_id), -1).unwrap();
        runtime
            .analysis(update_id, &analysis(value), &json!({}))
            .unwrap();
        let condition = runtime
            .pending()
            .unwrap()
            .into_iter()
            .find(|message| {
                message.payload["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Условия плана"))
            })
            .expect("each recalculation publishes waiting conditions");
        assert!(condition.payload["text"]
            .as_str()
            .unwrap()
            .contains(&plan_id));
        assert!(
            seen_condition_ids.insert(condition.id.clone()),
            "duplicate event ID {}",
            condition.id
        );
        runtime.delivered(&condition.id, 200 + update_id).unwrap();
        drop(runtime);
        runtime = Runtime::open_file(&p, &file, Engine::default()).unwrap();
    }

    assert_eq!(
        runtime.rules()["parameters"]["stop_buffer_atr"]["value"],
        0.1
    );
    let stored = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    assert!(stored["rules_hash"]
        .as_str()
        .is_some_and(|hash| !hash.is_empty()));
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        runtime.rules()
    );
    assert!(runtime
        .pending()
        .unwrap()
        .iter()
        .all(|message| !message.payload["text"]
            .as_str()
            .unwrap()
            .contains("Условия плана")));
    std::fs::remove_file(p).ok();
    std::fs::remove_file(file).ok();
}

#[test]
fn wait_confirm_notification_describes_current_confirmation_window_and_direction() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Side, Zone, ZoneKind},
    };

    let p = path();
    let file = p.with_extension("json");
    std::fs::write(&file, rules().to_string()).unwrap();
    let mut engine = Engine::default();
    engine.add_plan(
        "W_point_A",
        Side::Long,
        Timeframe::W,
        Zone::new(Side::Long, 1_000_000, 1_010_000, 1, ZoneKind::Fvg),
        1_700_000_000,
    );
    let mut runtime = Runtime::open_file(&p, &file, engine).unwrap();
    runtime.link(9, "wait-confirm-signal").unwrap();
    runtime.update(&update(1), -1).unwrap();
    runtime
        .analysis(
            1,
            &Analysis {
                action: "apply".into(),
                explanation: String::new(),
                changes: vec![Change {
                    path: "/parameters/confirmation_inversion_h4_bars/value".into(),
                    value: json!(31),
                }],
            },
            &json!({}),
        )
        .unwrap();
    let message = runtime
        .pending()
        .unwrap()
        .into_iter()
        .find(|message| {
            message.payload["text"]
                .as_str()
                .is_some_and(|text| text.contains("Условия плана"))
        })
        .expect("wait_confirm condition notification");
    let text = message.payload["text"].as_str().unwrap();
    assert!(text.contains("W_point_A"), "{text}");
    assert!(text.to_lowercase().contains("покуп"), "{text}");
    assert!(
        text.contains("31 H4"),
        "new confirmation window must be explicit: {text}"
    );
    assert!(
        text.to_lowercase().contains("инверс"),
        "confirmation rule must be explicit: {text}"
    );
    let stored = forex_bot::state::Store::open(&p)
        .unwrap()
        .entities(forex_bot::state::Kind::Runtime)
        .unwrap()[0]
        .value
        .clone();
    assert_eq!(
        stored["markets"]["EURUSD"]["engine"]["plans"][0]["state"],
        "WaitConfirm"
    );
    std::fs::remove_file(p).ok();
    std::fs::remove_file(file).ok();
}

#[test]
fn wait_entry_notification_describes_current_entry_conditions() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Side, Zone, ZoneKind},
    };

    let p = path();
    let file = p.with_extension("json");
    std::fs::write(&file, rules().to_string()).unwrap();
    let mut engine = Engine::default();
    engine.add_plan(
        "D_correction_end",
        Side::Short,
        Timeframe::D,
        Zone::new(Side::Short, 1_100_000, 1_110_000, 1, ZoneKind::Fvg),
        1_700_000_000,
    );
    let mut runtime = Runtime::open_file(&p, &file, engine).unwrap();
    runtime.link(9, "wait-entry-signal").unwrap();
    runtime.update(&update(1), -1).unwrap();
    runtime
        .analysis(
            1,
            &Analysis {
                action: "apply".into(),
                explanation: String::new(),
                changes: vec![
                    Change {
                        path: "/parameters/stop_buffer_atr/value".into(),
                        value: json!(0.07),
                    },
                    Change {
                        path: "/parameters/rr_min/value".into(),
                        value: json!(2.2),
                    },
                ],
            },
            &json!({}),
        )
        .unwrap();
    let message = runtime
        .pending()
        .unwrap()
        .into_iter()
        .find(|message| {
            message.payload["text"]
                .as_str()
                .is_some_and(|text| text.contains("Условия плана"))
        })
        .expect("wait_entry condition notification");
    let text = message.payload["text"].as_str().unwrap();
    assert!(text.to_lowercase().contains("продаж"), "{text}");
    assert!(
        text.contains("2.2"),
        "current rr_min must be stated: {text}"
    );
    assert!(
        text.contains("0.07"),
        "current stop buffer must be stated: {text}"
    );
    assert!(
        text.to_lowercase().contains("входн"),
        "current entry conditions must be stated: {text}"
    );
    std::fs::remove_file(p).ok();
    std::fs::remove_file(file).ok();
}
#[test]
fn comments_survive_restart_and_replay_and_only_head_can_apply() {
    let (p, mut r) = setup();
    r.update(&update(1), -1).unwrap();
    r.update(&update(2), -1).unwrap();
    r.update(&update(1), -1).unwrap();
    drop(r);
    let mut r = open_test(&p).unwrap();
    assert_eq!(r.offset(), 3);
    assert_eq!(r.next_comment().unwrap().update_id, 1);
    assert!(r.analysis(2, &analysis(0.2), &json!({})).is_err());
    r.analysis(1, &analysis(0.1), &json!({})).unwrap();
    assert_eq!(r.rules()["parameters"]["stop_buffer_atr"]["value"], 0.1);
    assert_eq!(r.next_comment().unwrap().update_id, 2);
    r.analysis(2, &analysis(0.2), &json!({})).unwrap();
    assert!(r.next_comment().is_none());
    assert_eq!(r.rules()["parameters"]["stop_buffer_atr"]["value"], 0.2);
}
#[test]
fn malformed_patch_leaves_rules_intact_and_produces_explanation() {
    let (_, mut r) = setup();
    r.update(&update(1), -1).unwrap();
    let old = r.rules();
    r.analysis(1, &analysis(-1.0), &json!({})).unwrap();
    assert_eq!(r.rules(), old);
    assert!(r.next_comment().is_none());
    assert_eq!(r.pending().unwrap().len(), 1);
}
#[test]
fn outage_notice_is_once_and_queue_is_retained() {
    let (p, mut r) = setup();
    r.update(&update(1), -1).unwrap();
    r.api_outage().unwrap();
    r.api_outage().unwrap();
    drop(r);
    let mut r = open_test(&p).unwrap();
    r.api_outage().unwrap();
    assert_eq!(r.pending().unwrap().len(), 1);
    assert_eq!(r.next_comment().unwrap().update_id, 1);
    r.analysis(1, &analysis(0.2), &json!({"input_tokens":10}))
        .unwrap();
    assert!(r.next_comment().is_none());
}
#[test]
fn acknowledgement_is_durable_and_maps_reply_after_restart() {
    let (p, mut r) = setup();
    r.update(&update(1), -1).unwrap();
    r.analysis(1, &analysis(0.2), &json!({})).unwrap();
    let id = r.pending().unwrap()[0].id.clone();
    r.delivered(&id, 55).unwrap();
    drop(r);
    let mut r = open_test(&p).unwrap();
    assert!(r.pending().unwrap().is_empty());
    let mut u = update(2);
    u["message"]["reply_to_message"]["message_id"] = json!(55);
    r.update(&u, -1).unwrap();
    assert_eq!(r.next_comment().unwrap().signal_id, "signal-1");
}

#[test]
fn quotes_track_published_signal_during_api_outage_and_data_gate() {
    let p = path();
    let cfg: forex_bot::model::Strategy = serde_json::from_value(rules()).unwrap();
    let signal = forex_bot::trading::Signal::new(
        "s",
        forex_bot::trading::Side::Long,
        forex_bot::trading::Quote {
            time: 1,
            price: 100,
        },
        90,
        150,
        1,
        &cfg,
    )
    .unwrap();
    let mut value = serde_json::to_value(Engine::default()).unwrap();
    value["signals"] = json!([signal]);
    let mut r = Runtime::open(&p, rules(), serde_json::from_value(value).unwrap()).unwrap();
    r.api_outage().unwrap();
    let t = forex_bot::feed::Tick {
        symbol: "EURUSD".into(),
        millis: 2000,
        price: 89,

        source: "upscale:index".into(),
    };
    r.tick(t.clone(), false).unwrap();
    r.tick(t, false).unwrap();
    assert_eq!(r.pending().unwrap().len(), 2);
    drop(r);
    let r = open_test(&p).unwrap();
    assert_eq!(r.signal_context("EURUSD:s")["state"], "Stop");
}
#[test]
fn reply_to_clarification_retains_question_after_restart() {
    let (p, mut r) = setup();
    r.update(&update(1), -1).unwrap();
    r.analysis(
        1,
        &Analysis {
            action: "clarify".into(),
            explanation: "Буфер стопа сделать 0.2 ATR?".into(),
            changes: vec![],
        },
        &json!({}),
    )
    .unwrap();
    let id = r.pending().unwrap()[0].id.clone();
    r.delivered(&id, 55).unwrap();
    drop(r);
    let mut r = open_test(&p).unwrap();
    let mut u = update(2);
    u["message"]["text"] = json!("Да");
    u["message"]["reply_to_message"]["message_id"] = json!(55);
    r.update(&u, -1).unwrap();
    let context = r.comment_context(&r.next_comment().unwrap());
    assert!(context.to_string().contains("Буфер стопа сделать 0.2 ATR?"));
}
#[test]
fn long_explanations_are_deliverable_without_losing_text() {
    let (_, mut r) = setup();
    r.update(&update(1), -1).unwrap();
    let text = "📈".repeat(3000);
    r.analysis(
        1,
        &Analysis {
            action: "no_change".into(),
            explanation: text.clone(),
            changes: vec![],
        },
        &json!({}),
    )
    .unwrap();
    let messages = r.pending().unwrap();
    assert!(messages.len() > 1);
    let mut collected = String::new();
    for m in messages {
        let part = m.payload["text"].as_str().unwrap();
        assert!(part.encode_utf16().count() <= 3500);
        collected.push_str(part);
    }
    assert_eq!(collected, text);
}
#[test]
fn restart_does_not_publish_a_bar_that_spans_process_downtime() {
    let p = path();
    let mut r = open_test(&p).unwrap();
    let start = 79200_i64 * 1000;
    r.tick(
        forex_bot::feed::Tick {
            symbol: "EURUSD".into(),
            millis: start,
            price: 100,

            source: "upscale:index".into(),
        },
        true,
    )
    .unwrap();
    drop(r);
    let mut r = open_test(&p).unwrap();
    r.tick(
        forex_bot::feed::Tick {
            symbol: "EURUSD".into(),
            millis: start + 14400000,
            price: 105,

            source: "upscale:index".into(),
        },
        true,
    )
    .unwrap();
    drop(r);
    let store = forex_bot::state::Store::open(&p).unwrap();
    let snapshots = store.entities(forex_bot::state::Kind::Runtime).unwrap();
    let snapshot = &snapshots
        .iter()
        .find(|s| s.key == "coordinator")
        .unwrap()
        .value;
    assert!(snapshot["markets"]["EURUSD"]["engine"]["frames"]
        .get("H4")
        .is_none());
}
#[test]
fn weekly_state_change_is_announced_once_even_without_trade_entry() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Candle, Side},
    };
    let mut rules = rules();
    rules["parameters"]["atr_period"]["value"] = json!(1);
    let cfg: forex_bot::model::Strategy = serde_json::from_value(rules.clone()).unwrap();
    let mut engine = Engine::default();
    for (end, open, high, low, close) in [
        (1, 100, 110, 90, 105),
        (2, 105, 125, 100, 120),
        (3, 125, 135, 120, 130),
    ] {
        engine
            .seed(
                Timeframe::W,
                Candle {
                    end,
                    open,
                    high,
                    low,
                    close,
                },
                &cfg,
            )
            .unwrap();
    }
    assert_eq!(engine.trend(Timeframe::W), Some(Side::Long));
    let mut r = Runtime::open(&path(), rules, engine).unwrap();
    let start = 338400_i64 * 1000;
    for (millis, price) in [
        (start, 130),
        (start + 1000, 95),
        (start + 604800000, 96),
        (start + 604800000, 96),
    ] {
        r.tick(
            forex_bot::feed::Tick {
                symbol: "EURUSD".into(),
                millis,
                price,

                source: "upscale:index".into(),
            },
            true,
        )
        .unwrap();
    }
    let messages = r.pending().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].payload["text"]
        .as_str()
        .unwrap()
        .contains("W_trend_end"));
}

fn log_tick(seconds: i64) -> forex_bot::feed::Tick {
    forex_bot::feed::Tick {
        symbol: "EURUSD".into(),
        millis: seconds * 1000,
        price: 1100000,

        source: "upscale:index".into(),
    }
}
#[test]
fn heartbeat_uses_five_minutes_without_quotes_and_retains_quote_after_restart() {
    use std::time::Duration as D;
    let p = path();
    let mut r = open_test(&p).unwrap();
    assert!(r.status_log(D::from_secs(299)).is_none());
    let line = r.status_log(D::from_secs(300)).expect("heartbeat missing");
    assert!(
        line.contains("Работаю") && line.contains("котировка: нет") && line.contains("планов: 0")
    );
    assert!(r.status_log(D::from_secs(300)).is_none());
    assert!(r.status_log(D::from_secs(599)).is_none());
    r.tick(log_tick(79200), true).unwrap();
    assert!(r
        .status_log(D::from_secs(600))
        .unwrap()
        .contains("1970-01-01T22:00:00+00:00"));
    assert!(r.status_log(D::from_secs(1800)).is_some());
    assert!(r.status_log(D::from_secs(1800)).is_none());
    drop(r);
    let mut r = open_test(&p).unwrap();
    assert!(r.status_log(D::from_secs(299)).is_none());
    assert!(r
        .status_log(D::from_secs(300))
        .unwrap()
        .contains("1970-01-01T22:00:00+00:00"));
    assert!(r.pending().unwrap().is_empty());
}
#[test]
fn h4_log_is_once_for_complete_bar_and_never_for_replay_or_interrupted_bar() {
    let p = path();
    let mut r = open_test(&p).unwrap();
    assert!(r.tick_logged(log_tick(79200), true).unwrap().is_empty());
    assert!(r.tick_logged(log_tick(93599), true).unwrap().is_empty());
    let logs = r.tick_logged(log_tick(93600), true).unwrap();
    assert_eq!(logs.len(), 1, "closed H4 must log once");
    assert!(logs[0].contains("H4 1970-01-02T02:00:00+00:00"));
    assert!(logs[0].contains("недостаточно свечей H4"));
    assert!(r.tick_logged(log_tick(93600), true).unwrap().is_empty());
    assert!(r.pending().unwrap().is_empty());
    drop(r);
    let mut r = open_test(&p).unwrap();
    assert!(r.tick_logged(log_tick(93600), true).unwrap().is_empty());
    assert!(r.tick_logged(log_tick(108000), true).unwrap().is_empty());
    assert!(r.tick_logged(log_tick(122400), false).unwrap().is_empty());
}
#[test]
fn h4_log_distinguishes_absent_plans_from_unconfirmed_plan() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Candle, Side, Zone, ZoneKind, ZoneState},
    };
    let cfg = serde_json::from_value(rules()).unwrap();
    let mut e = Engine::default();
    for i in 0..30 {
        e.seed(
            Timeframe::H4,
            Candle {
                end: 79200 - (29 - i) * 14400,
                open: 1100000,
                high: 1100100,
                low: 1099900,
                close: 1100000,
            },
            &cfg,
        )
        .unwrap();
    }
    let mut r = Runtime::open(&path(), rules(), e.clone()).unwrap();
    r.tick_logged(log_tick(79200), true).unwrap();
    assert!(r.tick_logged(log_tick(93600), true).unwrap()[0].contains("нет ожидающих планов"));
    let z = Zone {
        kind: ZoneKind::Fvg,
        side: Side::Long,
        low: 1099000,
        high: 1100000,
        formed_at: 79200,
        state: ZoneState::Alive,
    };
    assert!(e.add_plan("W_point_A", Side::Long, Timeframe::W, z, 79200));
    let mut r = Runtime::open(&path(), rules(), e).unwrap();
    r.tick_logged(log_tick(79200), true).unwrap();
    assert!(r.tick_logged(log_tick(93600), true).unwrap()[0].contains("ожидание подтверждения H4"));
}

#[test]
fn h4_log_reports_new_signal_not_existing_signal_as_new() {
    use forex_bot::{
        engine::Timeframe,
        trading::{Candle, Side, Zone, ZoneKind},
    };
    let mut c: forex_bot::model::Strategy = serde_json::from_value(rules()).unwrap();
    c.parameters.atr_period.value = 1;
    c.context.minimum_bars.h4 = 1;
    let mut e = Engine::default();
    for (end, high) in [(1, 300), (2, 310), (3, 300)] {
        e.seed(
            Timeframe::D,
            Candle {
                end,
                open: 280,
                high,
                low: 270,
                close: 280,
            },
            &c,
        )
        .unwrap();
    }
    for i in 0..10 {
        e.seed(
            Timeframe::H4,
            Candle {
                end: 79200 - (9 - i) * 14400,
                open: 100,
                high: 110,
                low: 90,
                close: 100,
            },
            &c,
        )
        .unwrap();
    }
    e.add_plan(
        "D_correction_end",
        Side::Long,
        Timeframe::D,
        Zone::new(Side::Long, 90, 100, 79200, ZoneKind::Fvg),
        79200,
    );
    let mut r = Runtime::open(&path(), serde_json::to_value(c).unwrap(), e).unwrap();
    let t = |sec: i64| forex_bot::feed::Tick {
        symbol: "EURUSD".into(),
        millis: sec * 1000,
        price: 130,

        source: "upscale:index".into(),
    };
    r.tick_logged(t(79200), true).unwrap();
    let logs = r.tick_logged(t(93600), true).unwrap();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].contains("новых сигналов: 1"), "{}", logs[0]);
    assert_eq!(r.pending().unwrap().len(), 1);
    let logs = r.tick_logged(t(108000), true).unwrap();
    assert_eq!(logs.len(), 1);
    assert!(!logs[0].contains("новых сигналов: 1"));
}

#[test]
fn startup_notice_is_once_per_run_durable_and_not_a_signal() {
    let p = path();
    let mut r = open_test(&p).unwrap();
    assert!(r.pending().unwrap().is_empty());
    r.started(79200).unwrap();
    r.started(79201).unwrap();
    let out = r.pending().unwrap();
    assert_eq!(out.len(), 1, "one startup notice must be queued");
    assert!(out[0].payload["text"]
        .as_str()
        .unwrap()
        .contains("Бот запущен"));
    assert!(out[0].payload["text"]
        .as_str()
        .unwrap()
        .contains("1970-01-01T22:00:00+00:00"));
    assert!(out[0].payload["signal_id"].is_null());
    let first = out[0].id.clone();
    drop(r);
    let mut r = open_test(&p).unwrap();
    assert_eq!(r.pending().unwrap()[0].id, first);
    r.started(79200).unwrap(); // Clock may repeat; restart must still be distinct.
    let out = r.pending().unwrap();
    assert_eq!(out.len(), 2);
    assert_ne!(out[0].id, out[1].id);
    r.delivered(&first, 9).unwrap();
    r.delivered(&first, 9).unwrap();
    r.update(&update(1), -1).unwrap();
    assert!(r.next_comment().is_none());
    assert_eq!(r.pending().unwrap().len(), 1);
    drop(r);
    let r = open_test(&p).unwrap();
    assert_eq!(r.pending().unwrap().len(), 1);
    assert_ne!(r.pending().unwrap()[0].id, first);
}
