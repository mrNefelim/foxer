//! Strict prepared M1 input shared by Swing history replay.
use forex_core::prices;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

pub fn hash(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
pub fn verify(path: &Path, expected: &str) -> Result<(), String> {
    if hash(path)? != expected {
        return Err(format!("checksum mismatch: {}", path.display()));
    }
    Ok(())
}
pub fn json_file(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| e.to_string())
}
/// Streams verified prepared minutes whose start lies in [from, to) milliseconds.
pub fn read_minutes(
    files: &[(PathBuf, Value)],
    from: i64,
    to: i64,
    mut visit: impl FnMut(prices::Minute) -> Result<(), String>,
) -> Result<(), String> {
    let mut previous = None;
    for (path, meta) in files {
        verify(
            path,
            meta["sha256"].as_str().ok_or("missing prepared hash")?,
        )?;
        let mut reader = csv::Reader::from_path(path).map_err(|e| e.to_string())?;
        let headers = reader.headers().map_err(|e| e.to_string())?.clone();
        for row in reader.records() {
            let m = prices::parse_record(&headers, &row.map_err(|e| e.to_string())?)?;
            if previous.is_some_and(|p| p >= m.timestamp) {
                return Err("unordered or overlapping prepared minutes".into());
            }
            previous = Some(m.timestamp);
            let ms = m.timestamp * 1000;
            if ms >= to {
                return Ok(());
            }
            if ms >= from {
                visit(m)?;
            }
        }
    }
    Ok(())
}
