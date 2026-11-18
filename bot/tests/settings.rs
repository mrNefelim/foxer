use forex_bot::settings::Settings;
fn valid() -> String {
    "TELEGRAM_BOT_TOKEN=fake-token\nTELEGRAM_CHAT_ID=-100123\nOPENAI_API_KEY=fake-key\nOPENAI_MODEL=test-model\nOPENAI_MAX_TOKENS=1500\nDATABASE_PATH=data/signals.sqlite\nSTRATEGY_PATH=strategy/swing-rules.json\nQUOTE_STALE_SECONDS=120\n".into()
}
#[test]
fn parses_settings_without_author_allowlist_or_manual_data_gate() {
    let c = Settings::parse(&valid()).unwrap();
    assert_eq!(c.chat, -100123);
}
#[test]
fn rejects_empty_missing_duplicate_unknown_and_invalid_settings() {
    for s in [
        String::new(),
        valid().replace("fake-token", ""),
        format!("{}OPENAI_MODEL=duplicate\n", valid()),
        format!("{}UNKNOWN=private-value\n", valid()),
        valid().replace("SECONDS=120", "SECONDS=0"),
    ] {
        let e = Settings::parse(&s).err().expect("must reject");
        assert!(!e.contains("private-value"));
        assert!(!e.contains("fake-token"));
    }
}
#[test]
fn quotes_are_literal_and_not_shell_expanded() {
    let c = Settings::parse(&valid().replace("test-model", "'$(do-not-run)' ")).unwrap();
    assert_eq!(c.model, "$(do-not-run)");
}
#[test]
fn single_strategy_path_needs_no_secondary_strategy_or_price_tick() {
    assert!(Settings::parse(&valid()).is_ok());
}

#[test]
fn optional_upscale_key_does_not_block_existing_service_settings() {
    for suffix in [
        "",
        "UPSCALE_API_KEY=fake-upscale-key\n",
        "UPSCALE_API_KEY='fake-upscale-key'\n",
    ] {
        assert!(Settings::parse(&format!("{}{suffix}", valid())).is_ok());
    }
}

#[test]
fn upscale_key_preserves_strict_validation_without_exposing_values() {
    for (suffix, expected) in [
        ("UPSCALE_API_KEY=\n", "missing setting: UPSCALE_API_KEY"),
        ("UPSCALE_API_KEY=''\n", "missing setting: UPSCALE_API_KEY"),
        (
            "UPSCALE_API_KEY=fake-upscale-key\nUPSCALE_API_KEY=other-private-value\n",
            "duplicate setting: UPSCALE_API_KEY",
        ),
        ("UPSCALE_API_KE=fake-upscale-key\n", "unknown setting name"),
    ] {
        let error = Settings::parse(&format!("{}{suffix}", valid()))
            .err()
            .expect("must reject");
        assert_eq!(error, expected);
        assert!(!error.contains("fake-upscale-key"));
        assert!(!error.contains("other-private-value"));
    }
}
#[test]
fn removed_secondary_strategy_setting_is_rejected() {
    assert!(Settings::parse(&format!("{}SWING_STRATEGY=strategy/swing.json\n", valid())).is_err());
    assert!(Settings::parse(&format!("{}PRICE_TICK=1\n", valid())).is_err());
}
