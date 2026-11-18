use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "forex-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(p.join("strategy")).unwrap();
        Self(p)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_forex-bot"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
    fn write(&self, value: &Value) {
        fs::write(
            self.0.join("strategy/swing-rules.json"),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn working() -> Value {
    serde_json::from_str(include_str!("../../strategy/swing-rules.json")).unwrap()
}
fn rejection(value: Value, category: &str) {
    let w = Workspace::new();
    w.write(&value);
    let before = fs::read(w.0.join("strategy/swing-rules.json")).unwrap();
    let out = w.run(&["check-config"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(category),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read(w.0.join("strategy/swing-rules.json")).unwrap(),
        before
    );
}
#[test]
fn reads_working_swing_document() {
    let w = Workspace::new();
    w.write(&working());
    let out = w.run(&["check-config"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "Configuration contract OK."
    );
}
#[test]
fn explicit_path_is_used() {
    let w = Workspace::new();
    fs::write(w.0.join("custom.json"), working().to_string()).unwrap();
    assert!(w.run(&["check-config", "custom.json"]).status.success());
}
#[test]
fn published_example_is_a_valid_strategy() {
    let w = Workspace::new();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../example.json");
    let out = w.run(&["check-config", path.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
#[test]
fn accepts_changed_numeric_parameters() {
    let mut v = working();
    v["instruments"]["EURUSD"]["plan_days"] = json!(12);
    v["instruments"]["GBPUSD"]["stop_buffer_atr"] = json!(0);
    v["instruments"]["USDJPY"]["rr_fixed"] = json!(2.5);
    let w = Workspace::new();
    w.write(&v);
    assert!(w.run(&["check-config"]).status.success());
}
#[test]
fn missing_working_file_never_falls_back_to_example() {
    let w = Workspace::new();
    fs::write(w.0.join("example.json"), working().to_string()).unwrap();
    let out = w.run(&["check-config"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("read"));
}
#[test]
fn incomplete_spec_is_not_a_working_config() {
    rejection(json!({"comment": "Incomplete"}), "structure");
}
#[test]
fn malformed_and_duplicate_json_are_rejected_without_echoing_values() {
    for content in [
        "{\"secret\":\"DO_NOT_ECHO\",",
        "{\"instruments\":{},\"instruments\":{}}",
    ] {
        let w = Workspace::new();
        fs::write(w.0.join("strategy/swing-rules.json"), content).unwrap();
        let out = w.run(&["check-config"]);
        assert_eq!(out.status.code(), Some(1));
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains("structure"));
        assert!(!error.contains("DO_NOT_ECHO"));
    }
}
#[test]
fn rejects_unknown_fields_and_comment_types_at_all_supported_depths() {
    for path in ["", "/instruments/EURUSD", "/instruments/GBPUSD/range"] {
        let mut v = working();
        v.pointer_mut(path).unwrap()["unknown"] = json!(true);
        rejection(v, "structure");
    }
    for path in ["/comment", "/comments/plan_days"] {
        let mut v = working();
        *v.pointer_mut(path).unwrap() = json!(42);
        rejection(v, "structure");
    }
    let mut v = working();
    v["instruments"]["EURUSD"]["comment"] = json!("Nested notes are not configuration");
    rejection(v, "structure");
}
#[test]
fn rejects_missing_fields_and_wrong_types() {
    for path in [
        "/instruments/EURUSD/plan_days",
        "/instruments/EURUSD/take_profit",
        "/instruments/GBPUSD/range/enabled",
        "/instruments/USDJPY/entry_close_hours",
    ] {
        let mut v = working();
        *v.pointer_mut(path).unwrap() = json!(["wrong"]);
        rejection(v, "structure");
    }
    for field in ["atr_period", "rr_fixed", "range", "cost"] {
        let mut v = working();
        v["instruments"]["EURUSD"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        rejection(v, "structure");
    }
    let mut v = working();
    v["instruments"]["GBPUSD"]["range"]
        .as_object_mut()
        .unwrap()
        .remove("window_min");
    rejection(v, "structure");
}
#[test]
fn rejects_unknown_family_and_incomplete_family_configuration() {
    let mut v = working();
    v["instruments"]["EURUSD"]["family"] = json!("invented");
    rejection(v, "structure");
    let mut v = working();
    v["instruments"]["EURUSD"]["family"] = json!("trend_pullback");
    rejection(v, "requires trend_pullback parameters");
    let mut v = working();
    v["instruments"]["EURUSD"]["take_profit"] = json!("last_close");
    rejection(v, "structure");
}
#[test]
fn rejects_invalid_periods_prices_hours_and_range_boundaries() {
    for (path, value, diagnostic) in [
        ("/instruments/EURUSD/plan_days", json!(0), "positive"),
        ("/instruments/EURUSD/atr_period", json!(0), "positive"),
        (
            "/instruments/GBPUSD/stop_lookback_h4_bars",
            json!(0),
            "positive",
        ),
        (
            "/instruments/GBPUSD/stop_buffer_atr",
            json!(-0.1),
            "stop buffer",
        ),
        ("/instruments/USDJPY/rr_fixed", json!(0), "positive"),
        ("/instruments/USDJPY/rr_target_cap", json!(-1), "positive"),
        ("/instruments/USDJPY/cost", json!(-0.1), "nonnegative"),
        ("/instruments/EURUSD/entry_close_hours", json!([]), "hours"),
        (
            "/instruments/EURUSD/entry_close_hours",
            json!([0, 24]),
            "hours",
        ),
        (
            "/instruments/EURUSD/entry_close_hours",
            json!([0, 0]),
            "hours",
        ),
        ("/instruments/EURUSD/range/window_max", json!(1), "range"),
        ("/instruments/EURUSD/range/wick_min", json!(-1), "range"),
        (
            "/instruments/EURUSD/range/height_max_atr_d",
            json!(0.01),
            "range",
        ),
    ] {
        let mut v = working();
        *v.pointer_mut(path).unwrap() = value;
        rejection(v, diagnostic);
    }
    let mut v = working();
    v["instruments"]["EURUSD"]["atr_period"] = json!(1.5);
    rejection(v, "structure");
}
#[test]
fn nested_duplicates_and_duplicate_comments_are_rejected() {
    for content in [
        "{\"instruments\":{\"EURUSD\":{},\"EURUSD\":{}}}",
        "{\"comment\":\"a\",\"comment\":\"b\"}",
    ] {
        let w = Workspace::new();
        fs::write(w.0.join("strategy/swing-rules.json"), content).unwrap();
        assert!(!w.run(&["check-config"]).status.success());
    }
}
#[test]
fn usage_errors_do_not_start_service() {
    let w = Workspace::new();
    for args in [vec![], vec!["run"], vec!["check-config", "one", "two"]] {
        assert_eq!(w.run(&args).status.code(), Some(2));
    }
}
fn describe(v: &Value) -> Value {
    let w = Workspace::new();
    w.write(v);
    let before = fs::read(w.0.join("strategy/swing-rules.json")).unwrap();
    let out = w.run(&["describe-config"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read(w.0.join("strategy/swing-rules.json")).unwrap(),
        before
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn describe_covers_swing_values_constraints_and_comments_without_echoing_prose() {
    let mut v = working();
    v["comment"] = json!("PRIVATE_TEXT");
    v["comments"]["plan_days"] = json!("PRIVATE_TEXT");
    v["instruments"]["EURUSD"]["plan_days"] = json!(17);
    let r = describe(&v);
    assert_eq!(r["strategy"], "swing");
    assert!(!r.to_string().contains("PRIVATE_TEXT"));
    let fields = r["fields"].as_array().unwrap();
    let parameter = fields
        .iter()
        .find(|f| f["path"] == "/instruments/EURUSD/plan_days")
        .unwrap();
    assert_eq!(parameter["value"], 17);
    assert_eq!(parameter["telegram_editable"], true);
    assert_eq!(parameter["constraints"]["minimum"], 1.0);
    fn leaves(v: &Value, path: String, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    if k != "comment" && k != "comments" {
                        leaves(v, format!("{path}/{k}"), out)
                    }
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    leaves(v, format!("{path}/{i}"), out)
                }
            }
            _ => out.push(path),
        }
    }
    let mut paths = vec![];
    leaves(&v, String::new(), &mut paths);
    for path in paths {
        assert!(fields.iter().any(|f| f["path"] == path), "{path}");
    }
    assert!(r["comment_paths"]
        .as_array()
        .unwrap()
        .contains(&json!("/comments/plan_days")));
    let start = fields
        .iter()
        .find(|f| f["path"] == "/instruments/EURUSD/start")
        .unwrap();
    assert_eq!(start["telegram_editable"], false);
}
#[test]
fn comment_edits_do_not_change_calculation_configuration() {
    let mut v = working();
    let before = forex_core::document::calculations(&v).unwrap();
    v["comment"] = json!("Private explanation");
    v["comments"]["plan_days"] = json!("500 bars — prose only");
    assert_eq!(forex_core::document::calculations(&v).unwrap(), before);
    describe(&v);
}
#[test]
fn describe_explicit_path_and_invalid_config() {
    let w = Workspace::new();
    fs::write(w.0.join("custom.json"), working().to_string()).unwrap();
    assert!(w.run(&["describe-config", "custom.json"]).status.success());
    let mut v = working();
    v["instruments"]["EURUSD"]["rr_fixed"] = json!(0);
    w.write(&v);
    let out = w.run(&["describe-config"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}
