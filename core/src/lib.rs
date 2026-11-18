pub mod calendar;
pub mod contract;
pub mod document;
pub mod feed;
pub mod market;
pub mod model;
pub mod prices;
pub mod swing;

use model::Strategy;
use serde_json::Value;
use std::{fs, path::Path};

pub fn load_document(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|_| "read: cannot read strategy file")?;
    let document = serde_json::from_slice::<document::StrictValue>(&bytes)
        .map_err(|e| {
            format!(
                "structure: invalid JSON at line {}, column {}",
                e.line(),
                e.column()
            )
        })?
        .0;
    let strategy: Strategy = serde_json::from_value(document.clone())
        .map_err(|e| format!("structure: invalid Swing strategy: {e}"))?;
    validate(&strategy)?;
    Ok(document)
}

pub fn load(path: &Path) -> Result<Strategy, String> {
    serde_json::from_value(load_document(path)?)
        .map_err(|_| "structure: invalid Swing strategy".into())
}

pub fn valid_symbol(symbol: &str) -> bool {
    symbol.len() == 6 && symbol.bytes().all(|b| b.is_ascii_uppercase())
}

pub fn validate(strategy: &Strategy) -> Result<(), String> {
    if strategy.instruments.is_empty() {
        return Err("Swing instruments must not be empty".into());
    }
    for (symbol, rules) in &strategy.instruments {
        if !valid_symbol(symbol) {
            return Err(format!("invalid instrument: {symbol}"));
        }
        rules.validate().map_err(|e| format!("{symbol}: {e}"))?;
    }
    Ok(())
}
