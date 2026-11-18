//! Offline Swing replay and its verified market-data inputs.
pub mod history;
pub mod swing_history;
pub mod swing_replay;

/// Render saved Swing metrics without replaying quotes or changing the inputs.
pub fn report(directory: &std::path::Path) -> Result<(), String> {
    let status = std::process::Command::new("python3")
        .arg(
            std::env::var("FOREX_REPORT_SCRIPT")
                .unwrap_or_else(|_| "backtest/report/render_swing.py".into()),
        )
        .arg(directory.join("report.json"))
        .arg(directory.join("report.pdf"))
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("PDF failed; saved Swing metrics preserved for report-only retry".into())
    }
}
