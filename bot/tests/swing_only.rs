use forex_bot::{
    settings::Settings,
    state::{Outbound, Store},
};
use serde_json::json;
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn workspace() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "swing-only-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(path.join("strategy")).unwrap();
    fs::write(
        path.join("strategy/swing-rules.json"),
        include_str!("../../strategy/swing-rules.json"),
    )
    .unwrap();
    path
}
fn settings() -> &'static str {
    "TELEGRAM_BOT_TOKEN=fake-token\nTELEGRAM_CHAT_ID=-1\nOPENAI_API_KEY=fake-key\nOPENAI_MODEL=test-model\nOPENAI_MAX_TOKENS=1000\nDATABASE_PATH=data/test.sqlite\nSTRATEGY_PATH=strategy/swing-rules.json\nQUOTE_STALE_SECONDS=120\n"
}
#[test]
fn default_cli_and_service_accept_only_main_swing_document() {
    let path = workspace();
    fs::write(path.join("offline.env"), settings()).unwrap();
    for args in [vec!["check-config"], vec!["check-service", "offline.env"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_forex-bot"))
            .args(args)
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(path).unwrap();
}
#[test]
fn obsolete_secondary_strategy_and_price_tick_settings_are_rejected() {
    for suffix in ["SWING_STRATEGY=unused.json\n", "PRICE_TICK=1\n"] {
        assert!(
            Settings::parse(&format!("{}{suffix}", settings())).is_err(),
            "accepted obsolete {suffix}"
        );
    }
}
#[test]
fn migration_retires_only_legacy_notifications_preserving_swing_fifo() {
    let path = workspace();
    let db = path.join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    let messages = vec![
        Outbound {
            id: "tick:old:EURUSD:plan:1".into(),
            payload: json!({"signal_id":"EURUSD:plan:1", "text":"old trade"}),
        },
        Outbound {
            id: "tick:old:W_trend_new".into(),
            payload: json!({"text":"old weekly"}),
        },
        Outbound {
            id: "tick:new:EURUSD:swing:trend:1:123:active".into(),
            payload: json!({"signal_id":"EURUSD:swing:trend:1:123", "text":"swing"}),
        },
        Outbound {
            id: "system:feed".into(),
            payload: json!({"text":"feed status"}),
        },
    ];
    store.apply("legacy", &json!({}), &[], &messages).unwrap();
    store
        .retire_level_revisions(&json!({"calendar_version":4}))
        .unwrap();
    drop(store);
    let mut store = Store::open(&db).unwrap();
    store
        .retire_level_revisions(&json!({"calendar_version":4}))
        .unwrap();
    let ids: Vec<_> = store
        .pending()
        .unwrap()
        .into_iter()
        .map(|message| message.id)
        .collect();
    assert_eq!(
        ids,
        vec!["tick:new:EURUSD:swing:trend:1:123:active", "system:feed"]
    );
    fs::remove_dir_all(path).unwrap();
}
