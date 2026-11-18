//! Swing rules over prepared minutes, with bars from the bot's own aggregator.
use crate::history::{json_file, read_minutes};
use forex_core::{
    engine::Timeframe,
    feed::Aggregator,
    swing::{grid_time, rules_bars, Bar, Swing, Tf},
    trading::Candle,
};
use std::path::{Path, PathBuf};

/// Aggregator bars converted for the rules and ordered by close time, then W, D, H4.
pub fn to_rules(bars: &[(Timeframe, Candle, bool)]) -> Vec<(Tf, Bar)> {
    rules_bars(bars)
}
pub fn run(root: &Path, symbol: &str, config: &Path, output: &Path) -> Result<(), String> {
    if output.exists() {
        return Err("output already exists".into());
    }
    let quality = json_file(&root.join("quality-report.json"))?;
    if quality["symbol"].as_str() != Some(symbol) {
        return Err("prepared history symbol mismatch".into());
    }
    if quality["timezone"].as_str() != Some("UTC") {
        return Err("prepared history must use UTC".into());
    }
    if quality["price_basis"].as_str() != Some("index") {
        return Err("prepared history must use index prices".into());
    }
    let source = quality["source"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or("missing source")?
        .to_owned();
    let mut files: Vec<(PathBuf, serde_json::Value)> = quality["outputs"]
        .as_object()
        .ok_or("missing prepared outputs")?
        .iter()
        .filter(|(name, _)| {
            name.starts_with(&format!("{symbol}-"))
                && name.ends_with(".csv")
                && name.contains("-M1-")
        })
        .map(|(name, meta)| (root.join("prepared").join(name), meta.clone()))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.is_empty() {
        return Err("no prepared M1 files for requested symbol".into());
    }
    let cfg: forex_core::swing::SwingConfig =
        serde_json::from_slice(&std::fs::read(config).map_err(|_| "cannot read swing config")?)
            .map_err(|e| format!("invalid swing config: {e}"))?;
    cfg.validate()?;
    if cfg.reference_exit.is_some() {
        return Err("swing-history does not support reference_exit".into());
    }
    let mut swing = Swing::new(cfg);
    let mut aggregator = Aggregator::default();
    let mut quote_count = 0usize;
    read_minutes(&files, i64::MIN, i64::MAX, |m| {
        quote_count += 1;
        let bars = aggregator.minute_all(symbol, &source, &m)?;
        let now = grid_time(m.timestamp);
        for (t, bar) in to_rules(&bars) {
            swing.close(t, bar, now);
        }
        swing.minute(now, m.price[1] as f64 / 1e6, m.price[2] as f64 / 1e6);
        Ok(())
    })?;
    if quote_count == 0 {
        return Err("prepared history contains no quotes".into());
    }
    crate::swing_replay::write_trades(&swing, output)
}
