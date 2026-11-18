//! "Swing" rule set (requirements: Правила «свинг»). Pure calculation: closed bars and
//! minute extremes in, plans and trades out. Prices are floating point; time is seconds
//! on the source bar grid.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tf {
    W,
    D,
    H4,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub end: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}
/// UTC time on the source grid: New York wall time plus seven hours, as naive seconds.
pub fn grid_time(utc: i64) -> i64 {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_opt(utc, 0)
        .single()
        .map(|t| {
            t.with_timezone(&chrono_tz::America::New_York)
                .naive_local()
                .and_utc()
                .timestamp()
                + 7 * 3600
        })
        .unwrap_or(utc)
}
/// Bot candles (UTC close time, prices in millionths) as rule bars on the source grid,
/// ordered by close time, then W, D, H4.
pub fn rules_bars(
    bars: &[(crate::market::Timeframe, crate::market::Candle, bool)],
) -> Vec<(Tf, Bar)> {
    use crate::market::Timeframe;
    const SCALE: f64 = 1_000_000.0;
    let mut out: Vec<(Tf, Bar)> = bars
        .iter()
        .map(|(t, c, _)| {
            (
                match t {
                    Timeframe::W => Tf::W,
                    Timeframe::D => Tf::D,
                    Timeframe::H4 => Tf::H4,
                },
                Bar {
                    end: grid_time(c.end),
                    open: c.open as f64 / SCALE,
                    high: c.high as f64 / SCALE,
                    low: c.low as f64 / SCALE,
                    close: c.close as f64 / SCALE,
                },
            )
        })
        .collect();
    out.sort_by(|a, b| (a.1.end, a.0).cmp(&(b.1.end, b.0)));
    out
}
/// One minute of an instrument without its time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ohlc {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}
/// ICE dollar index weights in the order EURUSD, USDJPY, GBPUSD, USDCAD, USDSEK, USDCHF.
pub const DOLLAR_INDEX_WEIGHTS: [f64; 6] = [-0.576, 0.136, -0.119, 0.091, 0.042, 0.036];
pub const DOLLAR_INDEX_CONSTANT: f64 = 50.14348112;
/// Synthetic dollar index for one minute of the six pairs (order of DOLLAR_INDEX_WEIGHTS).
pub fn dollar_index(pairs: &[Ohlc; 6]) -> Ohlc {
    let base = DOLLAR_INDEX_CONSTANT.ln();
    let (mut o, mut c, mut h, mut l) = (base, base, base, base);
    for (p, w) in pairs.iter().zip(DOLLAR_INDEX_WEIGHTS) {
        o += w * p.open.ln();
        c += w * p.close.ln();
        let (up, down) = if w > 0.0 {
            (p.high, p.low)
        } else {
            (p.low, p.high)
        };
        h += w * up.ln();
        l += w * down.ln();
    }
    let (open, close) = (o.exp(), c.exp());
    Ohlc {
        open,
        high: h.exp().max(open.max(close)),
        low: l.exp().min(open.min(close)),
        close,
    }
}
/// Event recorded by a rule set; the exit by the index reads them from the index run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Event {
    Confirm,
    Sweep,
    Target,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceExit {
    /// Index direction matching a trade direction (-1 for EURUSD).
    pub sign: i8,
    /// Number of the latest known index events that are examined.
    pub recent_events: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeProfit {
    Fixed,
    Target,
}
/// Range trading parameters (requirements: Торговля в боковике). Every field is required in JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeConfig {
    pub enabled: bool,
    pub min_h4_bars: usize,
    pub window_min: usize,
    pub window_max: usize,
    pub height_min_atr_d: f64,
    pub height_max_atr_d: f64,
    pub impulse_bars: usize,
    pub impulse_min_atr_d: f64,
    pub impulse_min_ratio: f64,
    pub break_fraction: f64,
    /// Minimal wick beyond a border, in price units.
    pub wick_min: f64,
    pub max_stops_per_border: u32,
}
impl Default for RangeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_h4_bars: 32,
            window_min: 12,
            window_max: 60,
            height_min_atr_d: 0.4,
            height_max_atr_d: 1.5,
            impulse_bars: 12,
            impulse_min_atr_d: 1.5,
            impulse_min_ratio: 1.2,
            break_fraction: 0.5,
            wick_min: 0.001,
            max_stops_per_border: 2,
        }
    }
}

/// Signal-generation family selected independently for each instrument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyFamily {
    #[default]
    LegacySwing,
    TrendPullback,
}

/// Parameters that affect only trend-pullback signal generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrendPullbackConfig {
    pub fast_ema_bars: usize,
    pub slow_ema_bars: usize,
    /// Optional per-instrument cap; absence preserves old unbounded snapshots.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_open_position_limit"
    )]
    pub max_open_positions: Option<usize>,
}

fn deserialize_open_position_limit<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let limit = usize::deserialize(deserializer)?;
    if limit == 0 {
        return Err(serde::de::Error::custom(
            "trend_pullback.max_open_positions must be positive",
        ));
    }
    Ok(Some(limit))
}

/// Swing rule parameters. Every field except `reference_exit` is required in JSON;
/// `Default` exists only for building rules in code.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwingConfig {
    /// Missing in legacy files and snapshots written before family selection existed.
    #[serde(default)]
    pub family: StrategyFamily,
    /// Required exactly when family is `trend_pullback`.
    #[serde(default)]
    pub trend_pullback: Option<TrendPullbackConfig>,
    pub atr_period: usize,
    pub rr_fixed: f64,
    pub take_profit: TakeProfit,
    pub rr_target_cap: f64,
    pub plan_days: usize,
    pub stop_lookback_h4_bars: usize,
    pub stop_buffer_atr: f64,
    /// Correction plans confirm on H4 when no live daily zone opposes them before the target.
    pub correction_h4_confirm_without_opposing_zone: bool,
    /// Hours of day (source grid) of H4 closes at which an entry may be taken.
    pub entry_close_hours: Vec<i64>,
    /// Execution cost in price units, paid against the trade on entry.
    pub cost: f64,
    /// Bars before this time only build context.
    pub start: i64,
    pub range: RangeConfig,
    pub reference_exit: Option<ReferenceExit>,
}
impl Default for SwingConfig {
    fn default() -> Self {
        Self {
            family: StrategyFamily::LegacySwing,
            trend_pullback: None,
            atr_period: 14,
            rr_fixed: 2.0,
            take_profit: TakeProfit::Fixed,
            rr_target_cap: 5.0,
            plan_days: 10,
            stop_lookback_h4_bars: 30,
            stop_buffer_atr: 0.05,
            correction_h4_confirm_without_opposing_zone: true,
            entry_close_hours: vec![0, 8, 12, 16, 20],
            cost: 0.0,
            start: 0,
            range: RangeConfig::default(),
            reference_exit: None,
        }
    }
}

impl SwingConfig {
    /// Validates family-specific parameters after backwards-compatible deserialization.
    pub fn validate(&self) -> Result<(), String> {
        if self.atr_period == 0 || self.plan_days == 0 || self.stop_lookback_h4_bars == 0 {
            return Err("Swing periods must be positive".into());
        }
        if !self.rr_fixed.is_finite()
            || self.rr_fixed <= 0.0
            || !self.rr_target_cap.is_finite()
            || self.rr_target_cap <= 0.0
        {
            return Err("Swing RR and target cap must be finite and positive".into());
        }
        if !self.stop_buffer_atr.is_finite() || self.stop_buffer_atr < 0.0 || self.start < 0 {
            return Err("invalid Swing stop buffer or start".into());
        }
        let mut hours = std::collections::BTreeSet::new();
        if self.entry_close_hours.is_empty()
            || self
                .entry_close_hours
                .iter()
                .any(|hour| !(0..24).contains(hour) || !hours.insert(*hour))
        {
            return Err("Swing entry hours must be nonempty, unique and within 0..23".into());
        }
        let range = &self.range;
        if range.min_h4_bars == 0
            || range.window_min == 0
            || range.window_max < range.window_min
            || range.impulse_bars == 0
            || range.max_stops_per_border == 0
            || range.height_max_atr_d <= 0.0
            || range.height_max_atr_d < range.height_min_atr_d
            || [
                range.height_min_atr_d,
                range.height_max_atr_d,
                range.impulse_min_atr_d,
                range.impulse_min_ratio,
                range.break_fraction,
                range.wick_min,
            ]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("invalid Swing range parameters".into());
        }
        if !self.cost.is_finite() || self.cost < 0.0 {
            return Err("swing cost must be finite and nonnegative".into());
        }
        match (self.family, &self.trend_pullback) {
            (StrategyFamily::LegacySwing, None) => Ok(()),
            (StrategyFamily::LegacySwing, Some(_)) => {
                Err("trend_pullback parameters require family trend_pullback".into())
            }
            (StrategyFamily::TrendPullback, None) => {
                Err("family trend_pullback requires trend_pullback parameters".into())
            }
            (StrategyFamily::TrendPullback, Some(params)) => {
                let fast_ok = [8, 12, 16, 20].contains(&params.fast_ema_bars);
                let slow_ok = [32, 40, 50].contains(&params.slow_ema_bars);
                if !fast_ok || !slow_ok || params.fast_ema_bars >= params.slow_ema_bars {
                    return Err("trend_pullback EMA periods are unsupported".into());
                }
                if params.max_open_positions == Some(0) {
                    return Err("trend_pullback.max_open_positions must be positive".into());
                }
                if self.atr_period != 14
                    || self.stop_lookback_h4_bars != 10
                    || (self.stop_buffer_atr - 0.1).abs() > f64::EPSILON
                    || (self.rr_fixed - 2.0).abs() > f64::EPSILON
                    || self.take_profit != TakeProfit::Fixed
                {
                    return Err("trend_pullback shared execution parameters are unsupported".into());
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fractal {
    pub idx: usize,
    pub price: f64,
    pub swept: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    pub dir: i8,
    pub lo: f64,
    pub hi: f64,
    pub idx: usize,
    pub alive: bool,
    pub inverted_at: Option<usize>,
}
/// Closed bars of one timeframe with fractals, gaps and ATR.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Frame {
    atr_period: usize,
    /// Absolute index of the first stored bar; older bars were compacted away.
    #[serde(default)]
    offset: usize,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    pub fr_hi: Vec<Fractal>,
    pub fr_lo: Vec<Fractal>,
    pub fvg: Vec<Gap>,
}
impl Frame {
    pub fn new(atr_period: usize) -> Self {
        Self {
            atr_period,
            ..Self::default()
        }
    }
    /// Number of bars pushed so far, including compacted ones.
    pub fn len(&self) -> usize {
        self.offset + self.close.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Number of bars still stored.
    pub fn stored(&self) -> usize {
        self.close.len()
    }
    pub fn high(&self, k: usize) -> f64 {
        self.high[k - self.offset]
    }
    pub fn low(&self, k: usize) -> f64 {
        self.low[k - self.offset]
    }
    pub fn close_at(&self, k: usize) -> f64 {
        self.close[k - self.offset]
    }
    pub fn last_close(&self) -> Option<f64> {
        self.close.last().copied()
    }
    fn extremes(&self, from: usize, to: usize) -> (f64, f64) {
        let (a, b) = (from - self.offset, to - self.offset);
        (
            self.high[a..b]
                .iter()
                .fold(f64::NEG_INFINITY, |m, v| m.max(*v)),
            self.low[a..b].iter().fold(f64::INFINITY, |m, v| m.min(*v)),
        )
    }
    /// Drops stored bars older than `keep`, inverted gaps and fractals that no rule can reach.
    /// `fractals`: `None` keeps all fractals; `Some(lookback)` keeps the last two of each side
    /// and those within `lookback` bars.
    fn compact(&mut self, keep: usize, fractals: Option<usize>) {
        let drop = self.close.len().saturating_sub(keep);
        if drop > 0 {
            self.high.drain(..drop);
            self.low.drain(..drop);
            self.close.drain(..drop);
            self.offset += drop;
        }
        self.fvg.retain(|z| z.alive);
        if let Some(lookback) = fractals {
            let recent = self.len().saturating_sub(lookback);
            for list in [&mut self.fr_hi, &mut self.fr_lo] {
                let last_two = list.len().saturating_sub(2);
                let mut k = 0;
                list.retain(|f| {
                    k += 1;
                    k > last_two || f.idx >= recent
                });
            }
        }
    }
    pub fn push(&mut self, b: &Bar) {
        self.high.push(b.high);
        self.low.push(b.low);
        self.close.push(b.close);
        let i = self.len() - 1;
        let j = self.close.len() - 1;
        for f in &mut self.fr_hi {
            if !f.swept && b.high > f.price {
                f.swept = true;
            }
        }
        for f in &mut self.fr_lo {
            if !f.swept && b.low < f.price {
                f.swept = true;
            }
        }
        for z in &mut self.fvg {
            if z.alive && ((z.dir > 0 && b.close < z.lo) || (z.dir < 0 && b.close > z.hi)) {
                z.alive = false;
                z.inverted_at = Some(i);
            }
        }
        if j >= 2 {
            let (h, l) = (&self.high, &self.low);
            if h[j - 1] > h[j - 2] && h[j - 1] > h[j] {
                self.fr_hi.push(Fractal {
                    idx: i - 1,
                    price: h[j - 1],
                    swept: false,
                });
            }
            if l[j - 1] < l[j - 2] && l[j - 1] < l[j] {
                self.fr_lo.push(Fractal {
                    idx: i - 1,
                    price: l[j - 1],
                    swept: false,
                });
            }
            if l[j] > h[j - 2] {
                self.fvg.push(Gap {
                    dir: 1,
                    lo: h[j - 2],
                    hi: l[j],
                    idx: i,
                    alive: true,
                    inverted_at: None,
                });
            }
            if h[j] < l[j - 2] {
                self.fvg.push(Gap {
                    dir: -1,
                    lo: h[j],
                    hi: l[j - 2],
                    idx: i,
                    alive: true,
                    inverted_at: None,
                });
            }
        }
    }
    /// Mean bar range of the last `atr_period` bars. The summation order matches the
    /// reference prototype so that results are reproducible bit for bit.
    pub fn atr(&self) -> Option<f64> {
        let n = self.high.len();
        if n < 2 {
            return None;
        }
        let from = n.saturating_sub(self.atr_period);
        let a: Vec<f64> = (from..n).map(|k| self.high[k] - self.low[k]).collect();
        let sum = if a.len() < 8 {
            a.iter().fold(0.0, |s, v| s + v)
        } else {
            let mut r = [0.0; 8];
            r.copy_from_slice(&a[..8]);
            let whole = a.len() - a.len() % 8;
            let mut k = 8;
            while k < whole {
                for j in 0..8 {
                    r[j] += a[k + j];
                }
                k += 8;
            }
            let mut s = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
            for v in &a[whole..] {
                s += v;
            }
            s
        };
        Some(sum / a.len() as f64)
    }
    /// +1 up, -1 down, 0 none.
    pub fn structure(&self) -> i8 {
        let (nh, nl) = (self.fr_hi.len(), self.fr_lo.len());
        if nh < 2 || nl < 2 {
            return 0;
        }
        let (h2, h1) = (self.fr_hi[nh - 2].price, self.fr_hi[nh - 1].price);
        let (l2, l1) = (self.fr_lo[nl - 2].price, self.fr_lo[nl - 1].price);
        if h1 > h2 && l1 > l2 {
            1
        } else if h1 < h2 && l1 < l2 {
            -1
        } else {
            0
        }
    }
    fn latest_live_gap_behind(&self, dir: i8, price: f64) -> Option<&Gap> {
        self.fvg.iter().rev().find(|z| {
            z.alive
                && z.dir == dir
                && if dir > 0 {
                    z.hi <= price
                } else {
                    z.lo >= price
                }
        })
    }
    fn nearest_unswept_fractal(&self, dir: i8, price: f64) -> Option<f64> {
        let mut best: Option<f64> = None;
        if dir > 0 {
            for f in self.fr_hi.iter().filter(|f| !f.swept && f.price > price) {
                if best.is_none_or(|b| f.price < b) {
                    best = Some(f.price);
                }
            }
        } else {
            for f in self.fr_lo.iter().filter(|f| !f.swept && f.price < price) {
                if best.is_none_or(|b| f.price > b) {
                    best = Some(f.price);
                }
            }
        }
        best
    }
    fn nearest_fractal_beyond(&self, dir: i8, level: f64, lookback: usize) -> Option<f64> {
        let n = self.len() as i64;
        let recent = |f: &&Fractal| f.idx as i64 >= n - lookback as i64;
        let mut best: Option<f64> = None;
        if dir > 0 {
            for f in self.fr_lo.iter().filter(recent).filter(|f| f.price < level) {
                if best.is_none_or(|b| f.price > b) {
                    best = Some(f.price);
                }
            }
        } else {
            for f in self.fr_hi.iter().filter(recent).filter(|f| f.price > level) {
                if best.is_none_or(|b| f.price < b) {
                    best = Some(f.price);
                }
            }
        }
        best
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Trend,
    Correction,
    Range,
    TrendPullback,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RangeBox {
    id: u64,
    top: f64,
    bot: f64,
    /// Stops taken from the bottom (index 0, longs) and the top (index 1, shorts).
    stops: [u32; 2],
}
fn border(dir: i8) -> usize {
    usize::from(dir < 0)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PlanState {
    WaitConfirm,
    WaitEntry,
    Active,
    Done,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Plan {
    #[serde(default)]
    id: u64,
    kind: Kind,
    dir: i8,
    poi: (f64, f64),
    target: f64,
    conf_tf: Tf,
    state: PlanState,
    created_day: usize,
    h4_idx: usize,
    #[serde(default)]
    target_hit: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeState {
    Pending,
    Active,
    Stop,
    Target,
    Cancelled,
    ReferenceExit,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Market,
    Limit,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trade {
    pub kind: Kind,
    pub dir: i8,
    pub entry: f64,
    pub stop: f64,
    pub tp: f64,
    pub rr: f64,
    pub signal_t: i64,
    pub fill_t: Option<i64>,
    /// `None` is accepted only for snapshots written before order type was stored.
    #[serde(default)]
    pub entry_kind: Option<EntryKind>,
    pub state: TradeState,
    pub r: Option<f64>,
    pub exit_t: Option<i64>,
    plan: Option<u64>,
    range_box: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizing {
    pub entry: f64,
    pub tp: f64,
    pub rr: f64,
    pub limit: bool,
}
/// Entry price, take profit and order type for a signal at `close` with a fixed `stop`.
pub fn size_entry(dir: i8, close: f64, stop: f64, target: f64, c: &SwingConfig) -> Option<Sizing> {
    let d = f64::from(dir);
    let risk = (close - stop) * d;
    if risk <= 0.0 {
        return None;
    }
    if (target - close) * d / risk >= c.rr_fixed {
        let (tp, rr) = match c.take_profit {
            TakeProfit::Fixed => (close + c.rr_fixed * risk * d, c.rr_fixed),
            TakeProfit::Target => {
                let cap = close + c.rr_target_cap * risk * d;
                let tp = if dir > 0 {
                    target.min(cap)
                } else {
                    target.max(cap)
                };
                (tp, (tp - close) * d / risk)
            }
        };
        return Some(Sizing {
            entry: close,
            tp,
            rr,
            limit: false,
        });
    }
    let entry = (target + c.rr_fixed * stop) / (1.0 + c.rr_fixed);
    if (entry - stop) * d <= 0.0 || (dir > 0 && entry > close) || (dir < 0 && entry < close) {
        return None;
    }
    Some(Sizing {
        entry,
        tp: target,
        rr: c.rr_fixed,
        limit: true,
    })
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct EmaState {
    count: usize,
    seed_sum: f64,
    #[serde(default)]
    seed_mean: Option<f64>,
    value: Option<f64>,
}
impl EmaState {
    fn update(&mut self, close: f64, period: usize) -> Option<f64> {
        if let Some(previous) = self.value {
            let alpha = 2.0 / (period as f64 + 1.0);
            let value = alpha * close + (1.0 - alpha) * previous;
            self.value = Some(value);
            return Some(value);
        }
        self.count += 1;
        self.seed_sum += close;
        let previous_mean = self
            .seed_mean
            .unwrap_or_else(|| (self.seed_sum - close) / (self.count - 1).max(1) as f64);
        let mean = if self.count == 1 {
            close
        } else {
            previous_mean + (close - previous_mean) / self.count as f64
        };
        self.seed_mean = Some(mean);
        if self.count == period {
            self.value = Some(mean);
            Some(mean)
        } else {
            None
        }
    }
}

/// Streaming EMA history is snapshot state because H4 frame compaction discards old bars.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TrendPullbackState {
    #[serde(default)]
    fast: EmaState,
    #[serde(default)]
    slow: EmaState,
    #[serde(default)]
    previous_close: Option<f64>,
    #[serde(default)]
    last_bar_end: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Swing {
    cfg: SwingConfig,
    #[serde(default = "default_true")]
    allow_new_plans: bool,
    w: Frame,
    d: Frame,
    h4: Frame,
    plans: Vec<Plan>,
    #[serde(default)]
    next_plan: u64,
    trades: Vec<Trade>,
    d_count: usize,
    rbox: Option<RangeBox>,
    boxes: u64,
    last_impulse: Option<i64>,
    events: Vec<(i64, Event, i8)>,
    #[serde(default)]
    trend_pullback: TrendPullbackState,
}
fn default_true() -> bool {
    true
}
impl Swing {
    pub fn new(cfg: SwingConfig) -> Self {
        let n = cfg.atr_period;
        Self {
            cfg,
            allow_new_plans: true,
            w: Frame::new(n),
            d: Frame::new(n),
            h4: Frame::new(n),
            plans: vec![],
            next_plan: 0,
            trades: vec![],
            d_count: 0,
            rbox: None,
            boxes: 0,
            last_impulse: None,
            events: vec![],
            trend_pullback: TrendPullbackState::default(),
        }
    }
    pub fn trades(&self) -> &[Trade] {
        &self.trades
    }
    /// Removes warm-up trades already being managed by a retired configuration instance.
    pub fn remove_duplicate_open_trades(&mut self, existing: &[Trade]) {
        self.trades.retain(|trade| {
            !matches!(trade.state, TradeState::Pending | TradeState::Active)
                || !existing.iter().any(|old| {
                    matches!(old.state, TradeState::Pending | TradeState::Active)
                        && old.kind == trade.kind
                        && old.dir == trade.dir
                        && old.signal_t == trade.signal_t
                })
        });
    }
    /// Applies a trend-pullback cap to newly reconstructed trades while preserving
    /// the existing signal context. `external_open_positions` are slots held by
    /// other runtime instances for the same instrument.
    pub fn limit_warm_open_positions(&mut self, external_open_positions: usize) {
        let Some(limit) = self
            .cfg
            .trend_pullback
            .as_ref()
            .and_then(|parameters| parameters.max_open_positions)
        else {
            return;
        };
        let mut occupied = external_open_positions;
        self.trades.retain(|trade| {
            if !matches!(trade.state, TradeState::Pending | TradeState::Active) {
                return true;
            }
            if occupied >= limit {
                return false;
            }
            occupied += 1;
            true
        });
    }
    /// Fill order type for snapshots created before it was stored explicitly.
    pub fn normalize_legacy_entry_kinds(&mut self) {
        for trade in &mut self.trades {
            if trade.entry_kind.is_none() && trade.state == TradeState::Pending {
                trade.entry_kind = Some(EntryKind::Limit);
            }
        }
    }
    /// Stop creating plans while keeping existing Pending/Active trades manageable.
    pub fn stop_new_plans(&mut self) {
        self.allow_new_plans = false;
        let open_plans: std::collections::BTreeSet<u64> = self
            .trades
            .iter()
            .filter(|trade| matches!(trade.state, TradeState::Pending | TradeState::Active))
            .filter_map(|trade| trade.plan)
            .collect();
        self.plans.retain(|plan| open_plans.contains(&plan.id));
    }
    /// Resume plan creation after this swing configuration is selected again.
    pub fn resume_new_plans(&mut self) {
        self.allow_new_plans = true;
    }
    /// Events recorded so far: (time, event, direction).
    pub fn events(&self) -> &[(i64, Event, i8)] {
        &self.events
    }
    /// Current range borders (top, bottom).
    pub fn range_box(&self) -> Option<(f64, f64)> {
        self.rbox.as_ref().map(|b| (b.top, b.bot))
    }
    pub fn frame(&self, tf: Tf) -> &Frame {
        match tf {
            Tf::W => &self.w,
            Tf::D => &self.d,
            Tf::H4 => &self.h4,
        }
    }
    /// Registers a plan unless the same idea is still open. Returns whether it was added.
    pub fn add_plan(
        &mut self,
        kind: Kind,
        dir: i8,
        poi: (f64, f64),
        target: f64,
        conf_tf: Tf,
    ) -> bool {
        if !self.allow_new_plans {
            return false;
        }
        if self
            .plans
            .iter()
            .any(|p| p.kind == kind && p.dir == dir && p.poi == poi && p.state != PlanState::Done)
        {
            return false;
        }
        let mut conf_tf = conf_tf;
        if self.cfg.correction_h4_confirm_without_opposing_zone && conf_tf == Tf::D {
            let opposed = self.d.last_close().is_some_and(|px| {
                self.d.fvg.iter().any(|z| {
                    z.alive
                        && z.dir == -dir
                        && if dir > 0 {
                            px <= z.lo && z.lo < target
                        } else {
                            target < z.hi && z.hi <= px
                        }
                })
            });
            if !opposed {
                conf_tf = Tf::H4;
            }
        }
        self.next_plan += 1;
        self.plans.push(Plan {
            id: self.next_plan,
            kind,
            dir,
            poi,
            target,
            conf_tf,
            state: PlanState::WaitConfirm,
            created_day: self.d_count,
            h4_idx: self.h4.len(),
            target_hit: false,
        });
        true
    }
    /// A bar of `tf` closed; `now` is the time at which the close is observed.
    pub fn close(&mut self, tf: Tf, b: Bar, now: i64) {
        self.close_with_external_open_positions(tf, b, now, 0);
    }
    /// Close a bar while counting Pending/Active trades managed by other runtime
    /// instances for this instrument against a family-specific position cap.
    pub fn close_with_external_open_positions(
        &mut self,
        tf: Tf,
        b: Bar,
        now: i64,
        external_open_positions: usize,
    ) {
        if self.cfg.family == StrategyFamily::TrendPullback {
            if tf == Tf::H4
                && self
                    .trend_pullback
                    .last_bar_end
                    .is_some_and(|last| b.end <= last)
            {
                return;
            }
            match tf {
                Tf::W => self.w.push(&b),
                Tf::D => self.d.push(&b),
                Tf::H4 => {
                    self.h4.push(&b);
                    self.trend_pullback_step(&b, now, external_open_positions);
                }
            }
            self.compact();
            return;
        }
        match tf {
            Tf::W => self.w.push(&b),
            Tf::D => self.d.push(&b),
            Tf::H4 => self.h4.push(&b),
        }
        if now >= self.cfg.start {
            self.decide(tf, &b);
        }
        self.compact();
    }
    /// Removes data that can no longer change a decision (requirements: Сохранение).
    fn compact(&mut self) {
        let keep = 128
            .max(self.cfg.range.window_max + self.cfg.range.impulse_bars + 2)
            .max(self.cfg.atr_period + 2);
        self.w.compact(keep, None);
        self.d.compact(keep, Some(0));
        self.h4
            .compact(keep, Some(self.cfg.stop_lookback_h4_bars + 1));
        let extra = self.events.len().saturating_sub(100);
        self.events.drain(..extra);
        self.plans.retain(|p| p.state != PlanState::Done);
    }
    /// Removes and returns trades that are finished (stop, target, cancelled, index exit).
    pub fn drain_finished(&mut self) -> Vec<Trade> {
        let (done, open): (Vec<Trade>, Vec<Trade>) = std::mem::take(&mut self.trades)
            .into_iter()
            .partition(|t| !matches!(t.state, TradeState::Pending | TradeState::Active));
        self.trades = open;
        done
    }
    /// Drains recorded events (time, event, direction).
    pub fn take_events(&mut self) -> Vec<(i64, Event, i8)> {
        std::mem::take(&mut self.events)
    }
    fn decide(&mut self, tf: Tf, b: &Bar) {
        let b = *b;
        if tf == Tf::D {
            self.d_count += 1;
        }
        if self.allow_new_plans {
            if tf == Tf::W {
                self.weekly_plans(&b);
            }
            if tf == Tf::H4 && self.cfg.range.enabled && self.h4.len() > self.cfg.range.min_h4_bars
            {
                self.range_step(&b);
            }
        }
        for k in 0..self.plans.len() {
            self.advance(k, tf, &b);
        }
    }
    fn trend_pullback_step(&mut self, bar: &Bar, now: i64, external_open_positions: usize) {
        let Some(parameters) = self.cfg.trend_pullback.as_ref() else {
            return;
        };
        let open_position_limit = parameters.max_open_positions;
        let state = &mut self.trend_pullback;
        let previous_close = state.previous_close;
        let previous_fast = state.fast.value;
        let previous_slow = state.slow.value;
        let fast = state.fast.update(bar.close, parameters.fast_ema_bars);
        let slow = state.slow.update(bar.close, parameters.slow_ema_bars);
        state.previous_close = Some(bar.close);
        state.last_bar_end = Some(bar.end);

        let (
            Some(previous_close),
            Some(previous_fast),
            Some(previous_slow),
            Some(fast),
            Some(slow),
        ) = (previous_close, previous_fast, previous_slow, fast, slow)
        else {
            return;
        };
        if now.div_euclid(60) != bar.end.div_euclid(60)
            || now < self.cfg.start
            || !self.allow_new_plans
        {
            return;
        }
        let direction = if previous_fast > previous_slow
            && fast > slow
            && previous_close <= previous_fast
            && bar.close > fast
            && bar.close > slow
        {
            1
        } else if previous_fast < previous_slow
            && fast < slow
            && previous_close >= previous_fast
            && bar.close < fast
            && bar.close < slow
        {
            -1
        } else {
            return;
        };

        // Closed-bar EMA state above is updated even when a confirmed entry is
        // discarded. Check before the same-timestamp minute exits are applied.
        if open_position_limit.is_some_and(|limit| {
            self.trades
                .iter()
                .filter(|trade| matches!(trade.state, TradeState::Pending | TradeState::Active))
                .count()
                .saturating_add(external_open_positions)
                >= limit
        }) {
            return;
        }

        let lookback = self.cfg.stop_lookback_h4_bars;
        let n = self.h4.len();
        if n < self.cfg.atr_period || n < lookback {
            return;
        }
        let (high, low) = self.h4.extremes(n - lookback, n);
        let Some(atr) = self.h4.atr().filter(|value| *value > 0.0) else {
            return;
        };
        let d = f64::from(direction);
        let stop = if direction > 0 {
            low - self.cfg.stop_buffer_atr * atr
        } else {
            high + self.cfg.stop_buffer_atr * atr
        };
        let entry = bar.close + d * self.cfg.cost;
        if !entry.is_finite() || entry <= 0.0 || !stop.is_finite() || stop <= 0.0 {
            return;
        }
        let risk = (entry - stop) * d;
        if !risk.is_finite() || risk <= 0.0 {
            return;
        }
        let tp = entry + d * self.cfg.rr_fixed * risk;
        if !tp.is_finite() || tp <= 0.0 || (tp - entry) * d <= 0.0 {
            return;
        }
        self.trades.push(Trade {
            kind: Kind::TrendPullback,
            dir: direction,
            entry,
            stop,
            tp,
            rr: self.cfg.rr_fixed,
            signal_t: bar.end,
            fill_t: Some(now),
            entry_kind: Some(EntryKind::Market),
            state: TradeState::Active,
            r: None,
            exit_t: None,
            plan: None,
            range_box: None,
        });
    }
    fn weekly_plans(&mut self, b: &Bar) {
        let s = self.w.structure();
        if s == 0 {
            return;
        }
        let zone = self
            .w
            .latest_live_gap_behind(s, b.close)
            .map(|z| (z.lo, z.hi));
        let target = self.w.nearest_unswept_fractal(s, b.close);
        if let (Some((lo, hi)), Some(target)) = (zone, target) {
            if (s > 0 && b.low <= hi) || (s < 0 && b.high >= lo) {
                self.add_plan(Kind::Trend, s, (lo, hi), target, Tf::H4);
            }
        }
        let n = self.w.len();
        let (prev_high, prev_low) = (self.w.high(n - 2), self.w.low(n - 2));
        let fractals = if s > 0 { &self.w.fr_hi } else { &self.w.fr_lo };
        let passed: Vec<f64> = fractals
            .iter()
            .filter(|f| {
                f.swept
                    && if s > 0 {
                        b.high > f.price && f.price >= prev_high
                    } else {
                        b.low < f.price && f.price <= prev_low
                    }
            })
            .map(|f| f.price)
            .collect();
        for price in passed {
            if let Some((lo, hi)) = zone {
                let target = if s > 0 { hi } else { lo };
                self.add_plan(Kind::Correction, -s, (price, price), target, Tf::D);
            }
        }
    }
    fn advance(&mut self, k: usize, tf: Tf, b: &Bar) {
        let p = self.plans[k].clone();
        if p.state == PlanState::Done {
            return;
        }
        let trade_is_active = self
            .trades
            .iter()
            .any(|trade| trade.plan == Some(p.id) && trade.state == TradeState::Active);
        if self.d_count as i64 - p.created_day as i64 >= self.cfg.plan_days as i64
            && !trade_is_active
        {
            for trade in &mut self.trades {
                if trade.plan == Some(p.id) && trade.state == TradeState::Pending {
                    trade.state = TradeState::Cancelled;
                }
            }
            self.plans[k].state = PlanState::Done;
            return;
        }
        let beyond = (p.dir > 0 && b.close < p.poi.0) || (p.dir < 0 && b.close > p.poi.1);
        if beyond
            && ((tf == Tf::W && p.kind == Kind::Trend) || (tf == Tf::D && p.kind != Kind::Trend))
        {
            self.plans[k].state = PlanState::Done;
            return;
        }
        if self.plans[k].state == PlanState::WaitConfirm && tf == p.conf_tf {
            let frame = self.frame(tf);
            let i = frame.len() - 1;
            if frame.fvg.iter().any(|z| {
                (z.dir == p.dir && z.idx == i) || (z.dir == -p.dir && z.inverted_at == Some(i))
            }) {
                self.plans[k].state = PlanState::WaitEntry;
                self.plans[k].h4_idx = self.h4.len() - usize::from(tf == Tf::H4);
                self.events.push((b.end, Event::Confirm, p.dir));
            }
        }
        if self.plans[k].state != PlanState::WaitEntry || tf != Tf::H4 {
            return;
        }
        let i = self.h4.len() - 1;
        let from = self.plans[k].h4_idx;
        let Some(zone) = self
            .h4
            .fvg
            .iter()
            .rev()
            .find(|z| z.dir == p.dir && z.idx == i && z.idx >= from)
            .cloned()
        else {
            return;
        };
        if !self.entry_hour(b.end) {
            return;
        }
        let d = f64::from(p.dir);
        let level = if p.dir > 0 {
            b.close.min(zone.lo)
        } else {
            b.close.max(zone.hi)
        };
        let reference = self
            .h4
            .nearest_fractal_beyond(p.dir, level, self.cfg.stop_lookback_h4_bars)
            .unwrap_or(if p.dir > 0 { zone.lo } else { zone.hi });
        let stop = reference - d * self.cfg.stop_buffer_atr * self.h4.atr().unwrap_or(0.0);
        let Some(sizing) = size_entry(p.dir, b.close, stop, p.target, &self.cfg) else {
            return;
        };
        let market = !sizing.limit;
        self.trades.push(Trade {
            kind: p.kind,
            dir: p.dir,
            entry: if market {
                b.close + d * self.cfg.cost
            } else {
                sizing.entry
            },
            stop,
            tp: sizing.tp,
            rr: sizing.rr,
            signal_t: b.end,
            fill_t: market.then_some(b.end),
            entry_kind: Some(if market {
                EntryKind::Market
            } else {
                EntryKind::Limit
            }),
            state: if market {
                TradeState::Active
            } else {
                TradeState::Pending
            },
            r: None,
            exit_t: None,
            plan: Some(self.plans[k].id),
            range_box: None,
        });
        self.plans[k].state = PlanState::Active;
    }
    /// Exit by the index: `known` are the index events with time not after `t`, in order.
    pub fn reference_minute(&mut self, t: i64, close: f64, known: &[(i64, Event, i8)]) {
        let Some(rx) = &self.cfg.reference_exit else {
            return;
        };
        if t < self.cfg.start {
            return;
        }
        let recent = &known[known.len().saturating_sub(rx.recent_events)..];
        let sign = rx.sign;
        for tr in &mut self.trades {
            if tr.state != TradeState::Active {
                continue;
            }
            let filled = tr.fill_t.unwrap_or(tr.signal_t);
            let want = tr.dir * sign;
            if !recent.iter().any(|&(at, ev, dir)| {
                matches!(ev, Event::Target | Event::Sweep) && dir == want && filled < at && at <= t
            }) {
                continue;
            }
            let d = f64::from(tr.dir);
            tr.state = TradeState::ReferenceExit;
            tr.r = Some((close - tr.entry) * d / ((tr.entry - tr.stop) * d));
            tr.exit_t = Some(t);
            if let Some(p) = tr
                .plan
                .and_then(|id| self.plans.iter_mut().find(|p| p.id == id))
            {
                p.state = PlanState::Done;
            }
            if tr.kind == Kind::Range {
                self.rbox = None;
            }
        }
    }
    fn entry_hour(&self, end: i64) -> bool {
        self.cfg
            .entry_close_hours
            .contains(&(end.rem_euclid(86_400) / 3600))
    }
    fn estimate_box(&mut self) -> Option<RangeBox> {
        let rc = &self.cfg.range;
        let a = self.d.atr().filter(|a| *a > 0.0)?;
        let n = self.h4.len();
        let mut best: Option<(usize, f64, f64)> = None;
        for k in rc.window_min..=rc.window_max.min(n - 1) {
            let (from, to) = (n - k - 1, n - 1);
            let (top, bot) = self.h4.extremes(from, to);
            if top - bot > rc.height_max_atr_d * a {
                break;
            }
            if top - bot >= rc.height_min_atr_d * a {
                best = Some((k, top, bot));
            }
        }
        let (k, top, bot) = best?;
        if n - k - 1 < rc.impulse_bars {
            return None;
        }
        let (from, to) = (n - k - 1 - rc.impulse_bars, n - k - 1);
        let (high, low) = self.h4.extremes(from, to);
        let travel = high - low;
        if travel < rc.impulse_min_atr_d * a || travel < rc.impulse_min_ratio * (top - bot) {
            return None;
        }
        let impulse = (n - k - rc.impulse_bars) as i64;
        if self
            .last_impulse
            .is_some_and(|last| impulse <= last + rc.impulse_bars as i64)
        {
            return None;
        }
        self.last_impulse = Some(impulse);
        self.boxes += 1;
        Some(RangeBox {
            id: self.boxes,
            top,
            bot,
            stops: [0, 0],
        })
    }
    fn range_step(&mut self, b: &Bar) {
        match &self.rbox {
            None => self.rbox = self.estimate_box(),
            Some(r) => {
                let height = r.top - r.bot;
                let f = self.cfg.range.break_fraction;
                if b.close > r.top + f * height || b.close < r.bot - f * height {
                    self.rbox = None;
                }
            }
        }
        let Some(r) = self.rbox.clone() else { return };
        if self.trades.iter().any(|t| {
            t.kind == Kind::Range && matches!(t.state, TradeState::Pending | TradeState::Active)
        }) {
            return;
        }
        let n = self.h4.len();
        let (prev_close, prev_high, prev_low) = (
            self.h4.close_at(n - 2),
            self.h4.high(n - 2),
            self.h4.low(n - 2),
        );
        let a4 = self.h4.atr().unwrap_or(0.0);
        let rc = &self.cfg.range;
        for (dir, edge, extreme) in [(-1i8, r.top, b.high), (1i8, r.bot, b.low)] {
            let d = f64::from(dir);
            let deviation = if dir < 0 {
                extreme > edge
            } else {
                extreme < edge
            };
            let inside = r.bot <= b.close && b.close <= r.top;
            let prev_out = if dir < 0 {
                prev_close > r.top
            } else {
                prev_close < r.bot
            };
            let wick = (extreme - edge).abs() >= rc.wick_min;
            if !inside || r.stops[border(dir)] >= rc.max_stops_per_border {
                continue;
            }
            if !((deviation && wick) || prev_out) || !self.entry_hour(b.end) {
                continue;
            }
            let dev_ext = if dir < 0 {
                extreme.max(prev_high)
            } else {
                extreme.min(prev_low)
            };
            let stop = if dir < 0 {
                dev_ext + self.cfg.stop_buffer_atr * a4
            } else {
                dev_ext - self.cfg.stop_buffer_atr * a4
            };
            let target = if dir < 0 { r.bot } else { r.top };
            let mut entry = b.close;
            let mut risk = (entry - stop) * d;
            if risk <= 0.0 {
                continue;
            }
            let rr_target = (target - entry) * d / risk;
            let rr_fixed = self.cfg.rr_fixed;
            let limit = rr_target < rr_fixed;
            if limit {
                entry = (target + rr_fixed * stop) / (1.0 + rr_fixed);
                risk = (entry - stop) * d;
                if risk <= 0.0 || !(r.bot <= entry && entry <= r.top) {
                    continue;
                }
            }
            let tp = if (target - entry).abs() > rr_fixed * risk {
                entry + rr_fixed * risk * d
            } else {
                target
            };
            self.trades.push(Trade {
                kind: Kind::Range,
                dir,
                entry: if limit {
                    entry
                } else {
                    entry + d * self.cfg.cost
                },
                stop,
                tp,
                rr: if limit { rr_fixed } else { rr_target },
                signal_t: b.end,
                fill_t: (!limit).then_some(b.end),
                entry_kind: Some(if limit {
                    EntryKind::Limit
                } else {
                    EntryKind::Market
                }),
                state: if limit {
                    TradeState::Pending
                } else {
                    TradeState::Active
                },
                r: None,
                exit_t: None,
                plan: None,
                range_box: Some(r.id),
            });
            self.events.push((b.end, Event::Sweep, dir));
            break;
        }
    }
    /// One minute of quotes: `high` and `low` of the minute starting at `t`.
    pub fn minute(&mut self, t: i64, high: f64, low: f64) {
        if t < self.cfg.start {
            return;
        }
        for p in &mut self.plans {
            if p.state != PlanState::Done
                && !p.target_hit
                && ((p.dir > 0 && high >= p.target) || (p.dir < 0 && low <= p.target))
            {
                p.target_hit = true;
                self.events.push((t, Event::Target, p.dir));
            }
        }
        for tr in &mut self.trades {
            let d = f64::from(tr.dir);
            let long = tr.dir > 0;
            if tr.state == TradeState::Pending {
                let target_first = if long { high >= tr.tp } else { low <= tr.tp };
                let plan_done = tr.plan.is_some_and(|id| {
                    self.plans
                        .iter()
                        .find(|p| p.id == id)
                        .is_none_or(|p| p.state == PlanState::Done)
                });
                if plan_done || target_first {
                    tr.state = TradeState::Cancelled;
                    if let Some(p) = tr
                        .plan
                        .and_then(|id| self.plans.iter_mut().find(|p| p.id == id))
                    {
                        p.state = PlanState::Done;
                    }
                    continue;
                }
                if if long {
                    low <= tr.entry
                } else {
                    high >= tr.entry
                } {
                    tr.state = TradeState::Active;
                    tr.fill_t = Some(t);
                    tr.entry += d * self.cfg.cost;
                }
            }
            if tr.state != TradeState::Active {
                continue;
            }
            let risk = (tr.entry - tr.stop) * d;
            if if long {
                low <= tr.stop
            } else {
                high >= tr.stop
            } {
                tr.state = TradeState::Stop;
                tr.r = Some(-1.0);
            } else if if long { high >= tr.tp } else { low <= tr.tp } {
                tr.state = TradeState::Target;
                tr.r = Some((tr.tp - tr.entry) * d / risk);
            } else {
                continue;
            }
            tr.exit_t = Some(t);
            if let Some(p) = tr
                .plan
                .and_then(|id| self.plans.iter_mut().find(|p| p.id == id))
            {
                p.state = PlanState::Done;
            }
            if tr.kind == Kind::Range {
                if tr.state == TradeState::Stop {
                    if let Some(b) = self.rbox.as_mut().filter(|b| Some(b.id) == tr.range_box) {
                        b.stops[border(tr.dir)] += 1;
                    }
                }
                self.rbox = None;
            }
        }
    }
}
