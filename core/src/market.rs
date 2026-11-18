//! Shared market data. Prices are integer millionths; candle end is UTC seconds.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Timeframe {
    W,
    D,
    H4,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candle {
    pub end: i64,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
}
