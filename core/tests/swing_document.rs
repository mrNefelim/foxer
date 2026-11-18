use forex_core::{contract, document, load_document};
use serde_json::{json, Value};
use std::path::Path;

fn working_rules() -> Value {
    serde_json::from_str(include_str!("../../strategy/swing-rules.json")).unwrap()
}

#[test]
fn main_strategy_document_accepts_current_swing_rules() {
    let loaded = load_document(Path::new("../strategy/swing-rules.json")).unwrap();
    assert_eq!(loaded, working_rules());
    let calculation = document::calculations(&loaded).unwrap();
    assert_eq!(calculation["instruments"].as_object().unwrap().len(), 3);
    assert!(calculation.get("comment").is_none());
    assert!(calculation.get("comments").is_none());
}

#[test]
fn comments_do_not_change_calculation_but_pair_parameters_do() {
    let mut rules = working_rules();
    let original = document::calculations(&rules).unwrap();
    rules["comment"] = json!("Text-only change");
    rules["comments"]["rr_fixed"] = json!("Updated explanation");
    assert_eq!(original, document::calculations(&rules).unwrap());
    rules["instruments"]["EURUSD"]["rr_fixed"] = json!(2.5);
    let changed = document::calculations(&rules).unwrap();
    assert_ne!(
        original["instruments"]["EURUSD"],
        changed["instruments"]["EURUSD"]
    );
    assert_eq!(
        original["instruments"]["GBPUSD"],
        changed["instruments"]["GBPUSD"]
    );
}

#[test]
fn editable_contract_contains_real_swing_parameters_only() {
    let rules = working_rules();
    let paths = contract::editable_paths(&rules);
    assert!(paths.contains(&"/instruments/USDJPY/rr_fixed".to_owned()));
    assert!(paths.contains(&"/instruments/EURUSD/range/enabled".to_owned()));
    assert!(paths.contains(&"/comments/rr_fixed".to_owned()));
    assert!(!paths
        .iter()
        .any(|p| p.starts_with("/parameters/") || p.contains("ifvg")));
}

#[test]
fn invalid_swing_values_are_rejected_after_a_valid_control() {
    let rules = working_rules();
    document::calculations(&rules).unwrap();
    for (pointer, value) in [
        ("/instruments/EURUSD/atr_period", json!(0)),
        ("/instruments/EURUSD/rr_fixed", json!(0)),
        ("/instruments/EURUSD/cost", json!(-0.1)),
        ("/instruments/EURUSD/entry_close_hours", json!([0, 0])),
        ("/instruments/EURUSD/range/window_min", json!(100)),
    ] {
        let mut candidate = rules.clone();
        *candidate.pointer_mut(pointer).unwrap() = value;
        assert!(document::calculations(&candidate).is_err(), "{pointer}");
    }
    let mut candidate = rules;
    candidate["instruments"]["EURUSD"]["unsupported"] = json!(1);
    assert!(document::calculations(&candidate).is_err());
}
