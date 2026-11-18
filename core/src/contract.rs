//! Swing field capabilities shared by diagnostics and Telegram patches.
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Serialize)]
pub struct ParameterSpec {
    pub name: &'static str,
    pub kind: &'static str,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub allowed: &'static [&'static str],
    pub telegram_editable: bool,
}
macro_rules! num {
    ($n:literal,$k:literal,$min:expr,$max:expr) => {
        ParameterSpec {
            name: $n,
            kind: $k,
            minimum: Some($min),
            maximum: Some($max),
            allowed: &[],
            telegram_editable: true,
        }
    };
}
macro_rules! flag {
    ($n:literal) => {
        ParameterSpec {
            name: $n,
            kind: "boolean",
            minimum: None,
            maximum: None,
            allowed: &[],
            telegram_editable: true,
        }
    };
}
pub const PARAMETERS: &[ParameterSpec] = &[
    num!("atr_period", "integer", 1.0, usize::MAX as f64),
    num!("rr_fixed", "number", 0.0, f64::MAX),
    ParameterSpec {
        name: "take_profit",
        kind: "string",
        minimum: None,
        maximum: None,
        allowed: &["fixed", "target"],
        telegram_editable: true,
    },
    num!("rr_target_cap", "number", 0.0, f64::MAX),
    num!("plan_days", "integer", 1.0, usize::MAX as f64),
    num!("stop_lookback_h4_bars", "integer", 1.0, usize::MAX as f64),
    num!("stop_buffer_atr", "number", 0.0, f64::MAX),
    flag!("correction_h4_confirm_without_opposing_zone"),
    num!("entry_close_hours/*", "integer", 0.0, 23.0),
    num!("cost", "number", 0.0, f64::MAX),
    flag!("range/enabled"),
    num!("range/min_h4_bars", "integer", 1.0, usize::MAX as f64),
    num!("range/window_min", "integer", 1.0, usize::MAX as f64),
    num!("range/window_max", "integer", 1.0, usize::MAX as f64),
    num!("range/height_min_atr_d", "number", 0.0, f64::MAX),
    num!("range/height_max_atr_d", "number", 0.0, f64::MAX),
    num!("range/impulse_bars", "integer", 1.0, usize::MAX as f64),
    num!("range/impulse_min_atr_d", "number", 0.0, f64::MAX),
    num!("range/impulse_min_ratio", "number", 0.0, f64::MAX),
    num!("range/break_fraction", "number", 0.0, f64::MAX),
    num!("range/wick_min", "number", 0.0, f64::MAX),
    num!(
        "range/max_stops_per_border",
        "integer",
        1.0,
        u32::MAX as f64
    ),
    num!(
        "trend_pullback/fast_ema_bars",
        "integer",
        1.0,
        usize::MAX as f64
    ),
    num!(
        "trend_pullback/slow_ema_bars",
        "integer",
        2.0,
        usize::MAX as f64
    ),
    num!(
        "trend_pullback/max_open_positions",
        "integer",
        1.0,
        usize::MAX as f64
    ),
];
fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}
fn spec(path: &str) -> Option<&'static ParameterSpec> {
    let suffix = path.strip_prefix("/instruments/")?.split_once('/')?.1;
    PARAMETERS.iter().find(|p| {
        p.name == suffix
            || (p.name == "entry_close_hours/*"
                && suffix
                    .strip_prefix("entry_close_hours/")
                    .is_some_and(|i| i.parse::<usize>().is_ok()))
    })
}
pub fn comment_paths(v: &Value) -> Vec<String> {
    let mut out = vec!["/comment".into()];
    if let Some(comments) = v.get("comments").and_then(Value::as_object) {
        out.extend(comments.keys().map(|k| format!("/comments/{}", escape(k))));
    }
    out
}
fn leaves(v: &Value, path: &str, out: &mut Vec<Value>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                leaves(v, &format!("{path}/{}", escape(k)), out);
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                leaves(v, &format!("{path}/{i}"), out);
            }
        }
        _ => out.push(json!({"path":path,"value":v,"telegram_editable":false})),
    }
}
pub fn editable_paths(v: &Value) -> Vec<String> {
    let mut fields = vec![];
    leaves(v, "", &mut fields);
    let mut paths: Vec<_> = fields
        .iter()
        .filter_map(|field| {
            let path = field["path"].as_str()?;
            spec(path)
                .filter(|s| s.telegram_editable && !field["value"].is_null())
                .map(|_| path.to_owned())
        })
        .collect();
    paths.extend(comment_paths(v));
    paths
}
pub fn describe(c: &crate::model::Strategy) -> Result<Value, String> {
    crate::validate(c)?;
    let v = serde_json::to_value(c).map_err(|_| "serialization")?;
    let editable = editable_paths(&v);
    let mut fields = vec![];
    leaves(&v, "", &mut fields);
    for field in &mut fields {
        let path = field["path"].as_str().unwrap().to_owned();
        field["telegram_editable"] = json!(editable.contains(&path));
        if path == "/comment" || path.starts_with("/comments/") {
            field["value"] = Value::Null;
        }
        if let Some(s) = spec(&path) {
            field["constraints"] = serde_json::to_value(s).map_err(|_| "serialization")?;
        }
    }
    Ok(
        json!({"strategy":"swing","engine_implemented":true,"fields":fields,
        "parameters":PARAMETERS,"comment_paths":comment_paths(&v),
        "limitations":["family and start are file-only; only existing parameter leaves are editable",
            "RR and range height maximum must be positive; ranges, entry hours and family parameters are validated together",
            "reference_exit is supported only by swing-replay with index data"]}),
    )
}
