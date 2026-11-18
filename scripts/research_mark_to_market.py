"""Discrete portfolio R on common observed M1 closes, without forward filling.

CLI: --database data/forex.sqlite --results results.json --output NEW.json
For several results sharing period/symbols, load_common_quotes once and reuse its
returned arrays with calculate. No strategy selection or backtest execution here.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import sqlite3

import numpy as np

PIP_TICKS = {'EURUSD': 100, 'GBPUSD': 100, 'USDJPY': 10000}
CONTENT_PATH_KEYS = ('prepared', 'archive')
DECLARATION_HASH_KEYS = ('quality_sha256', 'manifest_sha256')


def _sha256(value):
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in '0123456789abcdefABCDEF' for character in value)
    )


def _declared_result_source(provenance, symbol):
    if not isinstance(provenance, list) or not provenance:
        raise ValueError(f'{symbol}: missing result source provenance')
    declared_sources = []
    content_count = 0
    for record in provenance:
        if not isinstance(record, dict):
            raise ValueError(f'{symbol}: invalid result provenance record')
        if 'source' in record or 'root' in record:
            root, source = record.get('root'), record.get('source')
            if (not isinstance(root, str) or not root.strip() or
                    not isinstance(source, str) or not source.strip() or
                    any(key in record for key in CONTENT_PATH_KEYS)):
                raise ValueError(f'{symbol}: invalid source declaration')
            for key in DECLARATION_HASH_KEYS:
                if key in record and not _sha256(record[key]):
                    raise ValueError(f'{symbol}: invalid source declaration hash ({key})')
            declared_sources.append(source)
            continue
        path_keys = [key for key in CONTENT_PATH_KEYS if key in record]
        if len(path_keys) != 1:
            raise ValueError(f'{symbol}: invalid content provenance record')
        path = record[path_keys[0]]
        if not isinstance(path, str) or not path.strip() or not _sha256(record.get('sha256')):
            raise ValueError(f'{symbol}: invalid content provenance record')
        content_count += 1
    unique_sources = set(declared_sources)
    if len(unique_sources) != 1:
        raise ValueError(f'{symbol}: missing or mixed result source provenance')
    if content_count == 0:
        raise ValueError(f'{symbol}: missing result content provenance')
    return next(iter(unique_sources))


def load_common_quotes(database, result):
    start, end = int(result['from']), int(result['to'])
    symbols = result['symbols']
    if start >= end or not symbols or len(set(symbols)) != len(symbols):
        raise ValueError('invalid period or symbols')
    database = Path(database).resolve()
    db = sqlite3.connect(database.as_uri() + '?mode=ro', uri=True)
    arrays, metadata = {}, {}
    try:
        db.execute('BEGIN')
        if (db.execute('PRAGMA application_id').fetchone()[0] != 1180190513 or
                db.execute('PRAGMA user_version').fetchone()[0] != 4):
            raise ValueError('unsupported database schema')
        for symbol in symbols:
            sources = db.execute('SELECT DISTINCT source FROM minutes WHERE symbol=?', (symbol,)).fetchall()
            if len(sources) != 1:
                raise ValueError(f'{symbol}: missing or mixed price sources')
            source = sources[0][0]
            source_map = result.get('sources')
            symbol_sources = source_map.get(symbol) if isinstance(source_map, dict) else None
            provenance = symbol_sources.get('files') if isinstance(symbol_sources, dict) else None
            result_source = _declared_result_source(provenance, symbol)
            if result_source != source:
                raise ValueError(f'{symbol}: result/database source mismatch')
            # Restrict using indexed second timestamps; observed M1 closes are
            # valued at minute start +59s, without reconstructing intraminute ticks.
            lo, hi = (start - 59000 + 999) // 1000, (end - 59000 + 999) // 1000
            rows = db.execute("SELECT timestamp*1000+59000,json_extract(payload,'$.price[3]') "
                              'FROM minutes WHERE symbol=? AND source=? AND timestamp>=? '
                              'AND timestamp<? ORDER BY timestamp', (symbol, source, lo, hi))
            array = np.fromiter(rows, dtype=[('time', '<i8'), ('price', '<i8')])
            if not len(array) or np.any(array['price'] <= 0):
                raise ValueError(f'{symbol}: absent or nonpositive observed prices')
            if np.any(np.diff(array['time']) <= 0) or np.any(array['time'] % 60000 != 59000):
                raise ValueError(f'{symbol}: invalid minute timestamps')
            arrays[symbol] = array
            metadata[symbol] = {'source': source, 'observations': len(array),
                                'first_ms': int(array['time'][0]), 'last_ms': int(array['time'][-1])}
    finally:
        db.close()
    times = arrays[symbols[0]]['time'].copy()
    for symbol in symbols[1:]:
        times = np.intersect1d(times, arrays[symbol]['time'], assume_unique=True)
    if not len(times):
        raise ValueError('no common observed M1 closes')
    prices = {s: a['price'][np.searchsorted(a['time'], times)] for s, a in arrays.items()}
    first_grid = ((start - 59000 + 59999) // 60000) * 60000 + 59000
    end_grid = ((end - 1 - 59000) // 60000) * 60000 + 59000 + 60000
    expected = max(0, (end_grid - first_grid) // 60000)
    edges = np.concatenate(([first_grid - 60000], times, [end_grid]))
    missing = [[int(edges[i] + 60000), int(edges[i+1])]
               for i in np.flatnonzero(np.diff(edges) > 60000)]
    coverage = {'quote_time': 'observed M1 close at minute_start + 59000 ms',
                'by_symbol': metadata, 'common_observations': len(times),
                'common_first_ms': int(times[0]), 'common_last_ms': int(times[-1]),
                'wall_clock_minute_slots': expected,
                'common_missing_minute_slots': expected - len(times),
                'missing_intervals_ms': missing,
                'missing_interval_semantics': '[first absent close timestamp, next common close timestamp); '
                    'includes weekends, scheduled closures and absent source minutes; no imputation',
                'declared_source_gaps': {s: result.get('sources', {}).get(s, {}).get('excluded_intervals', [])
                                         for s in symbols}}
    return times, prices, coverage


def _finite(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError('expected finite numeric trade value')
    return value


def _curve_summary(equity, times):
    peaks = np.maximum.accumulate(np.maximum(equity, 0.0))
    drawdown = peaks - equity
    trough = int(np.argmax(drawdown))
    peak = None
    if peaks[trough] > 0:
        peak = int(np.argmax(equity[:trough + 1]))
    return {'last_equity_r': float(equity[-1]),
            'max_mark_to_market_drawdown_r': float(drawdown[trough]),
            'drawdown_peak_ms': int(times[peak]) if peak is not None else None,
            'drawdown_peak_equity_r': float(peaks[trough]),
            'drawdown_trough_ms': int(times[trough]),
            'drawdown_trough_equity_r': float(equity[trough]),
            'peak_equity_r': float(max(0, np.max(equity))),
            'minimum_equity_r': float(min(0, np.min(equity)))}, {trough, *([] if peak is None else [peak])}


def calculate(result, times, prices, coverage=None):
    times = np.asarray(times, dtype=np.int64)
    n = len(times)
    if (not n or np.any(np.diff(times) <= 0) or times[0] < result['from'] or
            times[-1] >= result['to']):
        raise ValueError('invalid common observation times')
    for symbol in result['symbols']:
        if symbol not in prices or len(prices[symbol]) != n:
            raise ValueError('prices must share the exact common observation grid')
        if np.any(np.asarray(prices[symbol]) <= 0):
            raise ValueError('nonpositive observed price')
    unrealized = np.zeros(n, dtype=np.float64)
    realized_delta = np.zeros(n + 1, dtype=np.float64)
    cost_delta = np.zeros(n + 1, dtype=np.float64)
    count_delta = np.zeros(n + 1, dtype=np.int64)
    entered = 0
    closed = []
    total_cost = 0.0
    for trade in result['trades']:
        state = trade['state']
        if state not in ('Stop', 'TakeProfit', 'Active'):
            continue
        symbol, side = trade['symbol'], trade['side']
        if symbol not in prices or symbol not in PIP_TICKS or side not in ('Long', 'Short'):
            raise ValueError('unknown trade symbol, pip scale, or side')
        entry_at = _finite(trade['entry_at'])
        entry, stop = _finite(trade['entry']), _finite(trade['stop'])
        risk = abs(entry - stop)
        if risk <= 0 or not result['from'] <= entry_at < result['to']:
            raise ValueError('zero initial risk or entry outside result period')
        exit_at = result['to']
        if state != 'Active':
            exit_at = _finite(trade['exit_at'])
            realized = _finite(trade['result_r'])
            if exit_at < entry_at or exit_at >= result['to']:
                raise ValueError('invalid trade exit time')
            closed.append(trade)
        elif trade.get('exit_at') is not None:
            raise ValueError('active trade has exit timestamp')
        lo, hi = int(np.searchsorted(times, entry_at)), int(np.searchsorted(times, exit_at))
        if lo < hi:
            sign = 1 if side == 'Long' else -1
            unrealized[lo:hi] += sign * (np.asarray(prices[symbol][lo:hi], dtype=np.float64) - entry) / risk
        count_delta[lo] += 1
        count_delta[hi] -= 1
        cost = PIP_TICKS[symbol] / risk
        cost_delta[lo] += cost
        total_cost += cost
        if state != 'Active':
            realized_delta[hi] += realized
        entered += 1
    realized = np.cumsum(realized_delta[:-1])
    costs = np.cumsum(cost_delta[:-1])
    counts = np.cumsum(count_delta[:-1])
    equity = realized + unrealized
    stressed = equity - costs
    gross, gross_extremes = _curve_summary(equity, times)
    stress, stress_extremes = _curve_summary(stressed, times)
    net = peak = dd = 0.0
    for trade in sorted(closed, key=lambda t: (t['exit_at'], t['symbol'], t['id'])):
        net += trade['result_r']; peak = max(peak, net); dd = max(dd, peak-net)
    computed = {'closed': len(closed), 'net_r': net, 'max_closed_drawdown_r': dd}
    supplied = {key: result.get('summary', {}).get(key) for key in computed}
    comparison = {key: math.isclose(computed[key], value, rel_tol=1e-10, abs_tol=1e-10)
                  for key, value in supplied.items() if value is not None}
    days = times // 86400000
    daily_last = np.flatnonzero(np.r_[days[1:] != days[:-1], True])
    keep = sorted(set(map(int, daily_last)) | gross_extremes | stress_extremes |
                  {0, n-1, int(np.argmax(counts)), int(np.argmax(equity)), int(np.argmin(equity)),
                   int(np.argmax(stressed)), int(np.argmin(stressed))})
    series = [{'millis': int(times[i]), 'gross_equity_r': float(equity[i]),
               'stress_1pip_equity_r': float(stressed[i]), 'realized_gross_r': float(realized[i]),
               'unrealized_gross_r': float(unrealized[i]), 'cumulative_cost_r': float(costs[i]),
               'open_trades': int(counts[i])} for i in keep]
    return {'from_ms': result['from'], 'to_ms': result['to'], 'symbols': result['symbols'],
            'numpy_version': np.__version__, 'coverage': coverage or {'common_observations': n},
            'gross': gross, 'stress_1pip': stress,
            'risk': {'entered_trades': entered, 'peak_concurrent_trades_on_marks': int(np.max(counts)),
                     'max_open_initial_risk_r_on_marks': int(np.max(counts)),
                     'cost_r_at_last_common_mark': float(costs[-1]),
                     'cost_r_for_all_entered_trades': total_cost},
            'closed_only': {'computed': computed, 'backtest_summary': supplied, 'matches': comparison},
            'series': series,
            'notes': ['Each trade starts with independent 1R; this is additive R, not money or compounded equity.',
                      'Only common actually observed M1 closes are marked; missing prices are never carried forward.',
                      'Discrete observation drawdown is not intraminute maximum drawdown or worst-case risk.',
                      'Stress scenario charges a hypothetical total round-trip 1 pip at entry, including still-open trades.',
                      'Initial equity is zero. A null drawdown_peak_ms means the pre-trade zero baseline.',
                      'Series retains daily last observations, both drawdown peak/trough pairs and equity/concurrency extremes.',
                      'The last common mark can precede final backtest events; closed-only summary covers all recorded exits.']}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--database', type=Path, required=True)
    p.add_argument('--results', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    if args.output.exists():
        raise ValueError('output already exists')
    raw = args.results.read_bytes()
    result = json.loads(raw)
    times, prices, coverage = load_common_quotes(args.database, result)
    report = calculate(result, times, prices, coverage)
    report['input'] = {'database': str(args.database), 'results': str(args.results),
                       'results_sha256': hashlib.sha256(raw).hexdigest()}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as f:
        json.dump(report, f, indent=2, ensure_ascii=False, allow_nan=False)
        f.write('\n')
    print(json.dumps({k: report[k] for k in ['gross', 'stress_1pip', 'risk', 'closed_only']}, indent=2))


if __name__ == '__main__':
    main()
