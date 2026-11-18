use crate::{
    feed::{Aggregator, Tick},
    model::Strategy,
    protocols::{self, Analysis, Comment},
    state::{Entity, Kind, Outbound, Store},
    swing::{Kind as SwingKind, Swing, SwingConfig, Trade as SwingTrade, TradeState},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
};
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    #[serde(default)]
    calendar_version: u32,
    #[serde(default)]
    hash_version: u32,
    markets: BTreeMap<String, Market>,
    rules_hash: String,
    #[serde(default)]
    operation: Option<RuleOperation>,
    known: BTreeMap<i64, String>,
    #[serde(default)]
    texts: BTreeMap<i64, String>,
    #[serde(default)]
    reply_context: BTreeMap<i64, String>,
    /// Last published state, retained after Swing drains a completed trade.
    #[serde(default)]
    swing_context: BTreeMap<String, Value>,
    offset: i64,
    queue: VecDeque<Comment>,
    api_down: bool,
    sequence: u64,
    usage: Value,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Market {
    aggregator: Aggregator,
    feed_down: bool,
    #[serde(default)]
    swing: Option<SwingMarket>,
}
/// Swing rules of one instrument (requirements: Правила «свинг» в работе бота).
#[derive(Clone, Serialize, Deserialize)]
struct SwingMarket {
    config: Value,
    rules: Swing,
    #[serde(default = "default_true")]
    enabled: bool,
    /// Last published event per open trade.
    published: BTreeMap<String, String>,
    /// Older configuration instances retained only while their published trades are open.
    #[serde(default)]
    retired: Vec<RetiredSwing>,
}
#[derive(Clone, Serialize, Deserialize)]
struct RetiredSwing {
    config: Value,
    rules: Swing,
    published: BTreeMap<String, String>,
}
fn default_true() -> bool {
    true
}
fn swing_id(symbol: &str, t: &SwingTrade) -> String {
    let kind = match t.kind {
        SwingKind::Trend => "trend",
        SwingKind::Correction => "correction",
        SwingKind::Range => "range",
        SwingKind::TrendPullback => "trend_pullback",
    };
    format!("{symbol}:swing:{kind}:{}:{}", t.dir, t.signal_t)
}
fn swing_event(t: &SwingTrade, previous: Option<&str>) -> &'static str {
    match t.state {
        TradeState::Pending => "Лимитная заявка",
        TradeState::Active
            if t.entry_kind == Some(crate::swing::EntryKind::Limit)
                || previous == Some("Лимитная заявка")
                || (t.entry_kind.is_none() && t.fill_t.is_some_and(|f| f != t.signal_t)) =>
        {
            "Заявка исполнена"
        }
        TradeState::Active => "Сделка открыта",
        TradeState::Stop => "Стоп",
        TradeState::Target => "Тейк",
        TradeState::Cancelled => "Заявка снята",
        TradeState::ReferenceExit => "Закрыта по индексу доллара",
    }
}
pub fn swing_text(symbol: &str, t: &SwingTrade, event: &str, quote_millis: Option<i64>) -> String {
    let pair = format!("{}/{}", &symbol[..3], &symbol[3..]);
    let side = if t.dir > 0 {
        "покупка"
    } else {
        "продажа"
    };
    let kind = match t.kind {
        SwingKind::Trend => "тренд",
        SwingKind::Correction => "коррекция",
        SwingKind::Range => "боковик",
        SwingKind::TrendPullback => "тренд pullback",
    };
    let digits = if symbol.ends_with("JPY") { 3 } else { 5 };
    let p = |v: f64| format!("{v:.digits$}");
    let order = match t.entry_kind {
        Some(crate::swing::EntryKind::Limit) => "лимит",
        Some(crate::swing::EntryKind::Market) => "по рынку",
        None if t.state == TradeState::Pending
            || t.fill_t.is_some_and(|fill| fill != t.signal_t) =>
        {
            "лимит"
        }
        None => "по рынку",
    };
    let when = quote_millis
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|t| format!("\n{}", t.to_rfc3339()))
        .unwrap_or_default();
    let rr = (t.tp - t.entry).abs() / (t.entry - t.stop).abs();
    let mut text = format!(
        "{pair} · {side} · {kind}\n{event}\nВход {} ({order}) · стоп {} · тейк {} · RR {rr:.2}{when}",
        p(t.entry),
        p(t.stop),
        p(t.tp)
    );
    if let Some(r) = t.r {
        text.push_str(&format!("\nРезультат {r:+.2}R"));
    }
    text
}
#[derive(Clone, Serialize, Deserialize)]
struct RuleOperation {
    id: i64,
    before: String,
    after: String,
}
fn hash(v: &Value) -> String {
    format!("{:x}", Sha256::digest(v.to_string().as_bytes()))
}
fn calculation_hash(v: &Value) -> Result<String, String> {
    Ok(hash(&forex_core::document::calculations(v)?))
}
pub struct Runtime {
    store: Store,
    rules: Value,
    rules_path: Option<std::path::PathBuf>,
    s: Snapshot,
    last_status: std::time::Duration,
    startup_announced: bool,
}
fn value<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|_| "snapshot serialization failed".into())
}
fn message(id: String, text: String, signal: Option<&str>, reply: Option<i64>) -> Outbound {
    Outbound {
        id,
        payload: json!({"text":text,"signal_id":signal,"reply_to":reply}),
    }
}
struct SwingAdvanceContext<'a> {
    tick: &'a Tick,
    id: &'a str,
    config: &'a Value,
    bars: &'a [(crate::swing::Tf, crate::swing::Bar)],
    external_open_positions: usize,
    messages: &'a mut Vec<Outbound>,
    signal_context: &'a mut BTreeMap<String, Value>,
}
fn advance_swing(
    rules: &mut Swing,
    published: &mut BTreeMap<String, String>,
    context: SwingAdvanceContext<'_>,
) -> Result<(), String> {
    let SwingAdvanceContext {
        tick,
        id,
        config,
        bars,
        external_open_positions,
        messages,
        signal_context,
    } = context;
    let symbol = tick.symbol.as_str();
    let now = crate::swing::grid_time(tick.millis / 1000);
    let price = tick.price as f64 / 1_000_000.0;
    for (tf, bar) in bars {
        rules.close_with_external_open_positions(*tf, *bar, now, external_open_positions);
    }
    rules.minute(now, price, price);
    let mut trades = rules.drain_finished();
    trades.extend(rules.trades().iter().cloned());
    for trade in &trades {
        let key = swing_id(symbol, trade);
        let previous = published.get(&key).cloned();
        let mut events = vec![];
        if previous.is_none() {
            if trade.entry_kind == Some(crate::swing::EntryKind::Limit) {
                events.push("Лимитная заявка");
                if trade.fill_t.is_some() {
                    events.push("Заявка исполнена");
                }
            } else if trade.state != TradeState::Pending {
                events.push("Сделка открыта");
            }
        }
        if previous.as_deref() == Some("Лимитная заявка")
            && trade.fill_t.is_some()
            && !matches!(trade.state, TradeState::Pending)
        {
            events.push("Заявка исполнена");
        }
        let event = swing_event(trade, previous.as_deref());
        if previous.as_deref() != Some(event) && !events.contains(&event) {
            events.push(event);
        }
        for event in events {
            let mut display_trade = trade.clone();
            if event == "Лимитная заявка"
                && trade.entry_kind == Some(crate::swing::EntryKind::Limit)
                && trade.fill_t.is_some()
            {
                let cost = config["cost"]
                    .as_f64()
                    .ok_or("stored swing cost is invalid")?;
                display_trade.entry -= f64::from(trade.dir) * cost;
            }
            if matches!(
                event,
                "Лимитная заявка" | "Заявка исполнена" | "Сделка открыта"
            ) {
                display_trade.state = TradeState::Active;
                display_trade.r = None;
                display_trade.exit_t = None;
            }
            messages.push(message(
                format!("{id}:{key}:{event}"),
                swing_text(symbol, &display_trade, event, Some(tick.millis)),
                Some(&key),
                None,
            ));
            signal_context.insert(
                key.clone(),
                json!({"id":key,"symbol":symbol,"trade":trade,"config":config}),
            );
        }
        if matches!(trade.state, TradeState::Pending | TradeState::Active) {
            published.insert(key, event.to_owned());
        } else {
            published.remove(&key);
        }
    }
    Ok(())
}
fn open_swing_positions(rules: &Swing) -> usize {
    rules
        .trades()
        .iter()
        .filter(|trade| matches!(trade.state, TradeState::Pending | TradeState::Active))
        .count()
}
impl Runtime {
    pub fn started(&mut self, time: i64) -> Result<(), String> {
        if self.startup_announced {
            return Ok(());
        }
        let time = chrono::DateTime::from_timestamp(time, 0)
            .ok_or("invalid startup time")?
            .to_rfc3339();
        let mut s = self.s.clone();
        s.sequence = s.sequence.checked_add(1).ok_or("event sequence overflow")?;
        let id = format!("startup:{}", s.sequence);
        self.commit(
            &id,
            json!({"started_at":time}),
            s,
            vec![message(
                id.clone(),
                format!("Бот запущен. Время запуска: {time}"),
                None,
                None,
            )],
        )?;
        self.startup_announced = true;
        Ok(())
    }

    pub fn status_log(&mut self, elapsed: std::time::Duration) -> Option<String> {
        if elapsed.saturating_sub(self.last_status) < std::time::Duration::from_secs(300) {
            return None;
        }
        self.last_status = elapsed;
        Some(
            self.s
                .markets
                .iter()
                .map(|(symbol, m)| {
                    let last = m
                        .aggregator
                        .last_quote_millis()
                        .and_then(chrono::DateTime::from_timestamp_millis)
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_else(|| "нет".into());
                    format!(
                        "{symbol}: Работаю; последняя котировка: {last}; открытых Swing-позиций: {}",
                        m.swing.as_ref().map_or(0, |sw| open_swing_positions(&sw.rules) + sw.retired.iter().map(|old| open_swing_positions(&old.rules)).sum::<usize>())
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
    pub fn comment_context(&self, c: &Comment) -> Value {
        json!({"signal":self.signal_context(&c.signal_id),"replied_message":self.s.reply_context.get(&c.update_id)})
    }

    pub fn open(path: &Path, rules: Value) -> Result<Self, String> {
        let c: Strategy =
            serde_json::from_value(rules.clone()).map_err(|_| "invalid rules JSON")?;
        crate::validate(&c)?;
        let rules_hash = calculation_hash(&rules)?;
        let mut store = Store::open(path)?;
        let stored = store
            .entities(Kind::Runtime)?
            .into_iter()
            .find(|e| e.key == "coordinator");
        let mut s = if let Some(e) = stored {
            serde_json::from_value::<Snapshot>(e.value).map_err(|_| "invalid runtime snapshot")?
        } else {
            Snapshot {
                calendar_version: 4,
                hash_version: 3,
                markets: BTreeMap::new(),
                rules_hash: rules_hash.clone(),
                operation: None,
                known: BTreeMap::new(),
                texts: BTreeMap::new(),
                reply_context: BTreeMap::new(),
                swing_context: BTreeMap::new(),
                offset: 0,
                queue: VecDeque::new(),
                api_down: false,
                sequence: 0,
                usage: Value::Null,
            }
        };
        for market in s.markets.values_mut() {
            if let Some(swing) = market.swing.as_mut() {
                swing.rules.normalize_legacy_entry_kinds();
                for retired in &mut swing.retired {
                    retired.rules.normalize_legacy_entry_kinds();
                }
            }
        }
        if s.hash_version > 3 {
            return Err("unsupported runtime hash version".into());
        }
        if s.hash_version != 3 && s.operation.is_some() {
            return Err("unfinished legacy rule operation; restore with the previous application before upgrading".into());
        }
        if s.calendar_version != 4 {
            return Err("unsupported runtime calendar version".into());
        }
        let mut recovery_messages = vec![];
        for symbol in c.instruments.keys() {
            s.markets.entry(symbol.clone()).or_default();
        }
        for (symbol, market) in &mut s.markets {
            market.aggregator.disconnected();
            if !c.instruments.contains_key(symbol) {
                if let Some(swing) = &mut market.swing {
                    swing.enabled = false;
                    swing.rules.stop_new_plans();
                }
            }
        }
        s.markets
            .retain(|symbol, market| c.instruments.contains_key(symbol) || market.swing.is_some());
        // A staged operation contains hashes only. Never apply its old patch on recovery.
        if let Some(op) = s.operation.take() {
            if hash(&rules) != op.before && s.queue.front().is_some_and(|c| c.update_id == op.id) {
                let comment = s.queue.pop_front().unwrap();
                let explanation = if hash(&rules) == op.after {
                    "Изменение сохранено в JSON; обработка восстановлена после перезапуска."
                } else {
                    "JSON изменён отдельно; прерванная правка не применяется повторно."
                };
                recovery_messages.push(message(
                    format!("analysis:{}", op.id),
                    explanation.into(),
                    Some(&comment.signal_id),
                    Some(comment.message_id),
                ));
            }
        }
        s.calendar_version = 4;
        s.rules_hash = rules_hash;
        s.hash_version = 3;
        if recovery_messages.is_empty() {
            store.save_runtime(&value(&s)?)?;
        } else {
            s.sequence += 1;
            store.apply(
                &format!("restore:{}:{}", s.rules_hash, s.sequence),
                &json!({"rules_hash":s.rules_hash}),
                &[Entity {
                    kind: Kind::Runtime,
                    key: "coordinator".into(),
                    value: value(&s)?,
                }],
                &recovery_messages,
            )?;
        }
        store.retire_level_revisions(&value(&s)?)?;
        Ok(Self {
            store,
            s,
            rules,
            rules_path: None,
            last_status: std::time::Duration::ZERO,
            startup_announced: false,
        })
    }
    pub fn open_file(path: &Path, file: &Path) -> Result<Self, String> {
        let rules = forex_core::load_document(file)?;
        let mut r = Self::open(path, rules)?;
        r.rules_path = Some(file.to_owned());
        Ok(r)
    }
    /// Includes removed instruments while their saved Swing state is monitored.
    pub fn symbols(&self) -> Vec<String> {
        self.s.markets.keys().cloned().collect()
    }
    fn commit(
        &mut self,
        id: &str,
        event: Value,
        s: Snapshot,
        messages: Vec<Outbound>,
    ) -> Result<(), String> {
        let mut parts = Vec::new();
        for m in messages {
            let text = m.payload["text"].as_str().ok_or("missing outgoing text")?;
            let mut chunks = Vec::new();
            let mut chunk = String::new();
            let mut units = 0;
            for ch in text.chars() {
                if units + ch.len_utf16() > 3500 {
                    chunks.push(chunk);
                    chunk = String::new();
                    units = 0;
                }
                chunk.push(ch);
                units += ch.len_utf16();
            }
            chunks.push(chunk);
            let count = chunks.len();
            for (i, text) in chunks.into_iter().enumerate() {
                let mut payload = m.payload.clone();
                payload["text"] = json!(text);
                if i + 1 < count {
                    payload
                        .as_object_mut()
                        .ok_or("invalid outbox payload")?
                        .remove("revision");
                }
                parts.push(Outbound {
                    id: if count == 1 {
                        m.id.clone()
                    } else {
                        format!("{}:part:{i}", m.id)
                    },
                    payload,
                });
            }
        }
        if self.store.apply(
            id,
            &event,
            &[Entity {
                kind: Kind::Runtime,
                key: "coordinator".into(),
                value: value(&s)?,
            }],
            &parts,
        )? {
            self.s = s;
        }
        Ok(())
    }
    pub fn link(&mut self, id: i64, signal: &str) -> Result<(), String> {
        let mut s = self.s.clone();
        s.known.insert(id, signal.into());
        self.commit(&format!("link:{id}"), json!(signal), s, vec![])
    }
    pub fn update(&mut self, u: &Value, chat: i64) -> Result<(), String> {
        let id = u["update_id"].as_i64().ok_or("missing update ID")?;
        if id < self.s.offset {
            return Ok(());
        }
        let mut s = self.s.clone();
        if let Some(c) = protocols::parse_comment(u, chat, &s.known)? {
            if let Some(reply) = u["message"]["reply_to_message"]["message_id"].as_i64() {
                if let Some(text) = s.texts.get(&reply) {
                    s.reply_context.insert(id, text.clone());
                }
            }
            s.texts.insert(c.message_id, c.text.clone());
            s.queue.push_back(c);
        }
        s.offset = id.checked_add(1).ok_or("update ID overflow")?;
        self.commit(&format!("telegram:{id}"), u.clone(), s, vec![])
    }
    pub fn offset(&self) -> i64 {
        self.s.offset
    }
    pub fn next_comment(&self) -> Option<Comment> {
        // Never race a later rule revision against delivery of earlier levels.
        if self
            .store
            .pending()
            .ok()?
            .iter()
            .any(|m| m.payload.get("revision").is_some())
        {
            return None;
        }
        self.s.queue.front().cloned()
    }
    pub fn rules(&self) -> Value {
        self.rules.clone()
    }
    pub fn signal_context(&self, id: &str) -> Value {
        for (symbol, market) in &self.s.markets {
            if let Some(swing) = &market.swing {
                for (config, rules) in std::iter::once((&swing.config, &swing.rules))
                    .chain(swing.retired.iter().map(|old| (&old.config, &old.rules)))
                {
                    if let Some(trade) = rules
                        .trades()
                        .iter()
                        .find(|trade| swing_id(symbol, trade) == id)
                    {
                        return json!({"id":id,"symbol":symbol,"trade":trade,"config":config});
                    }
                }
            }
        }
        self.s.swing_context.get(id).cloned().unwrap_or(Value::Null)
    }
    pub fn analysis(&mut self, id: i64, a: &Analysis, usage: &Value) -> Result<(), String> {
        let comment = self
            .s
            .queue
            .front()
            .filter(|c| c.update_id == id)
            .cloned()
            .ok_or("analysis is not FIFO head")?;
        let mut s = self.s.clone();
        let mut messages = vec![];
        let mut explanation = a.explanation.clone();
        let mut next_rules = self.rules.clone();
        let candidate = if !comment.signal_id.contains(":swing:") {
            explanation =
                "Этот сигнал относится к удалённой стратегии FVG. Правила Swing не изменены."
                    .into();
            Ok(self.rules.clone())
        } else {
            protocols::apply_changes(&self.rules, a)
        };
        match candidate {
            Ok(rules) if rules!=self.rules => {
                let next_hash=calculation_hash(&rules)?;
                let file=self.rules_path.as_ref().ok_or("rules file is required for changes")?;
                let op=RuleOperation{id,before:hash(&self.rules),after:hash(&rules)};
                let mut staged=self.s.clone();staged.operation=Some(op);
                self.store.save_runtime(&value(&staged)?)?;
                if let Err(e)=write_rules(file,&rules) {
                    self.store.save_runtime(&value(&self.s)?)?;
                    return Err(e);
                }
                s.rules_hash=next_hash;s.operation=None;next_rules=rules;
                explanation.push_str("\nИзменения сохранены в JSON.");
            },
            Ok(_)=>{},
            Err(_)=>explanation="Изменение не применено: ответ не соответствует поддерживаемым правилам. Уточните параметр и значение.".into(),
        }
        messages.insert(
            0,
            message(
                format!("analysis:{id}"),
                explanation,
                Some(&comment.signal_id),
                Some(comment.message_id),
            ),
        );
        s.queue.pop_front();
        s.api_down = false;
        s.usage = usage.clone();
        self.commit(
            &format!("analysis:{id}"),
            json!({"action":a.action,"usage":usage}),
            s,
            messages,
        )?;
        self.rules = next_rules;
        Ok(())
    }
    pub fn pending(&self) -> Result<Vec<Outbound>, String> {
        Ok(self
            .store
            .pending()?
            .into_iter()
            .filter(|o| {
                !o.payload
                    .as_object()
                    .is_some_and(|p| p.contains_key("revision"))
                    && !o.id.starts_with("revision:")
                    && !o.id.starts_with("reconfigure:")
            })
            .collect())
    }
    pub fn api_outage(&mut self) -> Result<(), String> {
        if self.s.api_down {
            return Ok(());
        }
        let mut s = self.s.clone();
        s.api_down = true;
        s.sequence += 1;
        let id = format!("api-down:{}", s.sequence);
        self.commit(&id,json!("api-down"),s,vec![message(id.clone(),"OpenAI недоступен. Комментарии сохранены; сигналы продолжаются по действующим правилам.".into(),None,None)])
    }
    pub fn disconnected(&mut self) -> Result<(), String> {
        for symbol in self.s.markets.keys().cloned().collect::<Vec<_>>() {
            self.disconnected_symbol(&symbol)?;
        }
        Ok(())
    }
    pub fn disconnected_symbol(&mut self, symbol: &str) -> Result<(), String> {
        let mut s = self.s.clone();
        let m = s.markets.get_mut(symbol).ok_or("unknown instrument")?;
        m.aggregator.disconnected();
        if m.feed_down {
            return Ok(());
        }
        m.feed_down = true;
        s.sequence += 1;
        let id = format!("feed-down:{symbol}:{}", s.sequence);
        self.commit(
            &id,
            json!({"symbol":symbol,"feed_down":true}),
            s,
            vec![message(
                id.clone(),
                format!("{symbol}: поток котировок недоступен. Новые сигналы приостановлены."),
                None,
                None,
            )],
        )
    }
    pub fn tick(&mut self, t: Tick, allow_new: bool) -> Result<(), String> {
        self.tick_logged(t, allow_new).map(|_| ())
    }
    pub fn tick_logged(&mut self, t: Tick, allow_new: bool) -> Result<Vec<String>, String> {
        if t.source != "upscale:index" {
            return Err("live runtime requires Upscale index ticks".into());
        }
        let event = value(&t)?;
        let id = format!("tick:{:x}", Sha256::digest(event.to_string().as_bytes()));
        let mut s = self.s.clone();
        let symbol = t.symbol.clone();
        let m = s.markets.get_mut(&symbol).ok_or("unknown instrument")?;
        let mut messages = vec![];
        let all_bars = m.aggregator.push_all(t.clone())?;
        let h4_end = all_bars
            .iter()
            .find(|(tf, _, complete)| *complete && *tf == crate::market::Timeframe::H4)
            .map(|(_, bar, _)| bar.end);
        if m.feed_down {
            m.feed_down = false;
            messages.push(message(
                format!("{id}:restored"),
                format!("{symbol}: Котировки восстановлены. Пропущенное движение цены не восстанавливается."),
                None,
                None,
            ));
        }
        if let Some(sw) = m.swing.as_mut() {
            let bars = crate::swing::rules_bars(&all_bars);
            // Snapshot occupancy before processing any exits in this tick. This
            // makes admission conservative when a trade exits on the same minute.
            let current_at_start = open_swing_positions(&sw.rules);
            let retired_at_start: Vec<_> = sw
                .retired
                .iter()
                .map(|retired| open_swing_positions(&retired.rules))
                .collect();
            let retired_total: usize = retired_at_start.iter().sum();
            advance_swing(
                &mut sw.rules,
                &mut sw.published,
                SwingAdvanceContext {
                    tick: &t,
                    id: &id,
                    config: &sw.config,
                    bars: &bars,
                    external_open_positions: retired_total,
                    messages: &mut messages,
                    signal_context: &mut s.swing_context,
                },
            )?;
            let current_reserved = current_at_start.max(open_swing_positions(&sw.rules));
            for (index, retired) in sw.retired.iter_mut().enumerate() {
                let other_retired: usize = retired_at_start
                    .iter()
                    .enumerate()
                    .filter(|(other_index, _)| *other_index != index)
                    .map(|(_, count)| *count)
                    .sum();
                advance_swing(
                    &mut retired.rules,
                    &mut retired.published,
                    SwingAdvanceContext {
                        tick: &t,
                        id: &id,
                        config: &retired.config,
                        bars: &bars,
                        external_open_positions: current_reserved + other_retired,
                        messages: &mut messages,
                        signal_context: &mut s.swing_context,
                    },
                )?;
            }
            sw.retired.retain(|retired| {
                retired
                    .rules
                    .trades()
                    .iter()
                    .any(|trade| matches!(trade.state, TradeState::Pending | TradeState::Active))
            });
        }
        let mut logs = Vec::new();
        if let Some(end) = h4_end.filter(|_| allow_new) {
            let found = messages
                .iter()
                .filter(|out| {
                    out.payload["text"].as_str().is_some_and(|text| {
                        text.contains("Сделка открыта") || text.contains("Лимитная заявка")
                    })
                })
                .count();
            let result = if found > 0 {
                format!("новых сигналов: {found}")
            } else {
                "новых сигналов нет".into()
            };
            let time = chrono::DateTime::from_timestamp(end, 0)
                .ok_or("invalid H4 close time")?
                .to_rfc3339();
            logs.push(format!("{symbol}: H4 {time}; условия проверены; {result}"));
        }
        self.commit(&id, event, s, messages)?;
        Ok(logs)
    }
    /// Connects swing rules to an instrument. `warm` builds the rules from history and is
    /// called only when the instrument has no rules yet or its parameters changed.
    pub fn swing(
        &mut self,
        symbol: &str,
        config: Value,
        warm: impl FnOnce() -> Result<Swing, String>,
    ) -> Result<(), String> {
        let parsed: SwingConfig = serde_json::from_value(config.clone())
            .map_err(|e| format!("invalid swing rules for {symbol}: {e}"))?;
        parsed
            .validate()
            .map_err(|e| format!("invalid swing rules for {symbol}: {e}"))?;
        if parsed.reference_exit.is_some() {
            return Err("swing reference_exit is not available in the live bot".into());
        }
        let current = self
            .s
            .markets
            .get(symbol)
            .ok_or("swing rules for an unknown instrument")?;
        let same_config = current.swing.as_ref().is_some_and(|sw| sw.config == config);
        let enabled = current.swing.as_ref().is_some_and(|sw| sw.enabled);
        if same_config && enabled {
            return Ok(());
        }
        if same_config {
            let mut s = self.s.clone();
            s.sequence = s.sequence.checked_add(1).ok_or("event sequence overflow")?;
            let tag = format!("swing:resume:{symbol}:{}", s.sequence);
            let sw = s
                .markets
                .get_mut(symbol)
                .and_then(|market| market.swing.as_mut())
                .ok_or("swing rules disappeared")?;
            sw.enabled = true;
            sw.rules.resume_new_plans();
            return self.commit(&tag, json!({"symbol":symbol,"enabled":true}), s, vec![]);
        }
        let mut rules = warm()?;
        rules.drain_finished();
        // Delivered and still-pending outbox messages both prove that a stable
        // swing signal ID was already published. Read pending rows without
        // acknowledging or otherwise changing the delivery queue.
        let mut registered_ids: std::collections::BTreeSet<String> =
            self.s.known.values().cloned().collect();
        registered_ids.extend(
            self.store
                .pending()?
                .into_iter()
                .filter_map(|out| out.payload["signal_id"].as_str().map(str::to_owned)),
        );
        let mut s = self.s.clone();
        s.sequence += 1;
        let tag = format!("swing:{symbol}:{}:{}", hash(&config), s.sequence);
        let market = s
            .markets
            .get_mut(symbol)
            .ok_or("swing rules for an unknown instrument")?;
        let mut retired = if let Some(mut old) = market.swing.take() {
            let mut retired = old.retired;
            if old
                .rules
                .trades()
                .iter()
                .any(|trade| matches!(trade.state, TradeState::Pending | TradeState::Active))
            {
                old.rules.stop_new_plans();
                retired.push(RetiredSwing {
                    config: old.config,
                    rules: old.rules,
                    published: old.published,
                });
            }
            retired
        } else {
            Vec::new()
        };
        // A full-history warm-up can rediscover the same still-open signal. Keep its
        // original instance and do not publish/manage an identical second trade.
        let mut already_managed: Vec<_> = retired
            .iter()
            .flat_map(|old| old.rules.trades())
            .filter(|trade| matches!(trade.state, TradeState::Pending | TradeState::Active))
            .cloned()
            .collect();
        already_managed.extend(
            rules
                .trades()
                .iter()
                .filter(|trade| {
                    matches!(trade.state, TradeState::Pending | TradeState::Active)
                        && registered_ids.contains(&swing_id(symbol, trade))
                })
                .cloned(),
        );
        rules.remove_duplicate_open_trades(&already_managed);
        let retired_open_positions: usize = retired
            .iter()
            .map(|old| open_swing_positions(&old.rules))
            .sum();
        rules.limit_warm_open_positions(retired_open_positions);
        let mut published = BTreeMap::new();
        let mut messages = vec![];
        for tr in rules.trades() {
            let key = swing_id(symbol, tr);
            let event = if tr.state == TradeState::Pending {
                "Заявка до запуска"
            } else {
                "Открыта до запуска"
            };
            messages.push(message(
                format!("{tag}:{key}"),
                swing_text(symbol, tr, event, None),
                Some(&key),
                None,
            ));
            published.insert(key, swing_event(tr, None).to_owned());
        }
        market.swing = Some(SwingMarket {
            config: config.clone(),
            rules,
            enabled: true,
            published,
            retired: std::mem::take(&mut retired),
        });
        self.commit(&tag, json!({"symbol":symbol,"swing":config}), s, messages)
    }
    /// Reconcile attached swing rules with the current main rules document.
    /// Existing Pending/Active trades remain attached for monitoring.
    pub fn reconcile_swing(
        &mut self,
        configured: &std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        let disabled: Vec<String> = self
            .s
            .markets
            .iter()
            .filter_map(|(symbol, market)| {
                market
                    .swing
                    .as_ref()
                    .filter(|swing| swing.enabled && !configured.contains(symbol))
                    .map(|_| symbol.clone())
            })
            .collect();
        if disabled.is_empty() {
            return Ok(());
        }
        let mut s = self.s.clone();
        s.sequence = s.sequence.checked_add(1).ok_or("event sequence overflow")?;
        let tag = format!("swing:disable:{}", s.sequence);
        for symbol in &disabled {
            let swing = s
                .markets
                .get_mut(symbol)
                .and_then(|market| market.swing.as_mut())
                .ok_or("swing rules disappeared")?;
            swing.enabled = false;
            swing.rules.stop_new_plans();
        }
        self.commit(&tag, json!({"disabled_swing":disabled}), s, vec![])
    }
    pub fn swing_rules(&self, symbol: &str) -> Option<&Swing> {
        self.s
            .markets
            .get(symbol)?
            .swing
            .as_ref()
            .map(|sw| &sw.rules)
    }
    pub fn delivered(&mut self, id: &str, message_id: i64) -> Result<(), String> {
        let Some(out) = self.store.pending()?.into_iter().find(|m| m.id == id) else {
            return Ok(());
        };
        let mut s = self.s.clone();
        if let Some(text) = out.payload["text"].as_str() {
            s.texts.insert(message_id, text.into());
        }
        if let Some(signal) = out.payload["signal_id"].as_str() {
            s.known.insert(message_id, signal.into());
        }
        if self.store.acknowledge(
            &format!("delivered:{id}"),
            &json!({"message_id":message_id}),
            &[Entity {
                kind: Kind::Runtime,
                key: "coordinator".into(),
                value: value(&s)?,
            }],
            id,
        )? {
            self.s = s;
        }
        Ok(())
    }
}
fn write_rules(path: &Path, rules: &Value) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("json.pending");
    let bytes = serde_json::to_vec_pretty(rules).map_err(|_| "rules serialization failed")?;
    let mut file = std::fs::File::create(&tmp).map_err(|_| "cannot stage rules file")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "cannot persist rules file")?;
    std::fs::rename(&tmp, path).map_err(|_| "cannot publish rules file")?;
    std::fs::File::open(path.parent().unwrap_or(Path::new(".")))
        .and_then(|d| d.sync_all())
        .map_err(|_| "cannot sync rules directory")?;
    Ok(())
}
