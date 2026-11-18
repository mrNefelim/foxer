"""Build a reproducible Swing metrics JSON from saved replay CSVs."""
import argparse
import csv
from datetime import datetime, timezone, timedelta
import hashlib
import importlib.util
import json
import math
from pathlib import Path
from zoneinfo import ZoneInfo

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
MTM_PATH = ROOT / 'scripts/research_mark_to_market.py'
_spec = importlib.util.spec_from_file_location('swing_report_mtm', MTM_PATH)
MTM = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(MTM)
SYMBOLS = ('EURUSD', 'GBPUSD', 'USDJPY')
CSV_FIELDS = ('kind', 'dir', 'signal_t', 'fill_t', 'entry', 'stop', 'tp', 'rr',
              'state', 'r', 'exit_t')
NY = ZoneInfo('America/New_York')


def grid_to_utc_ms(value):
    """Invert Swing's source-grid clock; reject DST times without a unique inverse."""
    try:
        grid = datetime.strptime(value, '%Y-%m-%d %H:%M:%S')
    except (TypeError, ValueError) as exc:
        raise ValueError(f'invalid source-grid timestamp: {value!r}') from exc
    wall = grid - timedelta(hours=7)
    candidates = set()
    for fold in (0, 1):
        local = wall.replace(tzinfo=NY, fold=fold)
        utc = local.astimezone(timezone.utc)
        if utc.astimezone(NY).replace(tzinfo=None) == wall:
            candidates.add(int(utc.timestamp()) * 1000)
    if len(candidates) != 1:
        reason = 'nonexistent' if not candidates else 'ambiguous'
        raise ValueError(f'{reason} DST source-grid timestamp: {value}')
    return next(iter(candidates))


def index_price_units(value):
    """Convert decimal price to the engine's millionth-price scale."""
    return float(value) * 1_000_000


def read_scoped_trades(path, symbol, start_ms, cutoff_ms):
    """Convert Swing CSV rows to the shared MTM contract and censor at cutoff."""
    rows, stats = [], {'csv_rows': 0, 'included_fills': 0, 'fills_at_or_after_cutoff': 0,
                       'before_start': 0, 'unfilled_or_cancelled': 0,
                       'censored_at_cutoff': 0}
    with Path(path).open(newline='', encoding='utf-8') as stream:
        reader = csv.DictReader(stream)
        if tuple(reader.fieldnames or ()) != CSV_FIELDS:
            raise ValueError(f'{symbol}: unexpected Swing CSV columns')
        for index, raw in enumerate(reader):
            stats['csv_rows'] += 1
            fill_text = raw['fill_t'] or None
            if fill_text is None:
                stats['unfilled_or_cancelled'] += 1
                continue
            signal_at = grid_to_utc_ms(raw['signal_t'])
            fill_at = grid_to_utc_ms(fill_text)
            if signal_at > fill_at:
                raise ValueError(f'{symbol}: signal follows fill at CSV row {index + 2}')
            if fill_at < start_ms:
                stats['before_start'] += 1
                continue
            if fill_at >= cutoff_ms:
                stats['fills_at_or_after_cutoff'] += 1
                continue
            state_text = raw['state']
            exit_text = raw['exit_t'] or None
            exit_at = grid_to_utc_ms(exit_text) if exit_text else None
            side = 'Long' if int(raw['dir']) > 0 else 'Short'
            state_map = {'sl': 'Stop', 'tp': 'TakeProfit', 'dxy_close': 'TakeProfit',
                         'active': 'Active'}
            if state_text not in state_map:
                raise ValueError(f'{symbol}: unexpected filled trade state {state_text!r}')
            if exit_at is not None and exit_at < fill_at:
                raise ValueError(f'{symbol}: exit precedes fill at CSV row {index + 2}')
            censored = exit_at is None or exit_at >= cutoff_ms or state_text == 'active'
            if censored:
                state, exit_at, result_r = 'Active', None, None
                stats['censored_at_cutoff'] += 1
            else:
                state, result_r = state_map[state_text], float(raw['r'])
                if not math.isfinite(result_r):
                    raise ValueError(f'{symbol}: non-finite R at CSV row {index + 2}')
            entry = index_price_units(raw['entry'])
            stop = index_price_units(raw['stop'])
            if entry <= 0 or stop <= 0 or entry == stop:
                raise ValueError(f'{symbol}: invalid entry/stop at CSV row {index + 2}')
            rows.append({'id': f'{symbol}:{index}', 'symbol': symbol, 'side': side,
                         'state': state, 'entry_at': fill_at, 'entry': entry, 'stop': stop,
                         'exit_at': exit_at, 'result_r': result_r, 'kind': raw['kind'],
                         'signal_at': signal_at})
            stats['included_fills'] += 1
    return rows, stats


def terminal_mark_summary(mtm_report):
    """Use the last observed mark's realized/open split, not full-cutoff exits."""
    series = mtm_report['series']
    if not series:
        raise ValueError('MTM report has no observed marks')
    last = series[-1]
    realized = float(last['realized_gross_r'])
    unrealized = float(last['unrealized_gross_r'])
    return {'mark_utc_ms': int(last['millis']), 'realized_r': realized,
            'unrealized_r': unrealized, 'marked_equity_r': realized + unrealized,
            'open_positions': int(last['open_trades'])}


def _sha(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def _load_plan(run_dir):
    run_dir = Path(run_dir)
    plan_path, completion_path = run_dir / 'run-plan.json', run_dir / 'completion.json'
    plan_raw = plan_path.read_bytes()
    plan = json.loads(plan_raw)
    completion = json.loads(completion_path.read_text())
    if completion.get('status') != 'complete' or completion.get('fresh_replay_count') != 3:
        raise ValueError('three successful Swing replays are required')
    if completion.get('run_plan_sha256') != hashlib.sha256(plan_raw).hexdigest():
        raise ValueError('completion manifest does not match run plan')
    return plan, completion


def _load_csv_and_config(run_dir, plan, completion, symbol):
    run_dir = Path(run_dir)
    cfg_path = run_dir / 'configs' / f'{symbol}.json'
    trade_path = run_dir / 'trades' / f'{symbol}.csv'
    config = json.loads(cfg_path.read_text())
    strategy = json.loads((run_dir / 'strategy.json').read_text())
    if strategy['instruments'][symbol] != config:
        raise ValueError(f'{symbol}: config differs from strategy snapshot')
    if _sha(cfg_path) != plan['config_sha256'][symbol]:
        raise ValueError(f'{symbol}: config hash differs from run plan')
    expected = [o for o in completion['outputs'] if o.get('symbol') == symbol]
    if len(expected) != 1 or expected[0].get('exit_code') != 0:
        raise ValueError(f'{symbol}: missing successful replay output')
    output = expected[0]
    if (trade_path.resolve() != (ROOT / output['path']).resolve() or
            _sha(trade_path) != output['sha256']):
        raise ValueError(f'{symbol}: trade CSV differs from completion manifest')
    return config, trade_path, output


def _mtm_source(plan, symbol):
    source = plan['sources'][symbol]
    quality_path = ROOT / source['root'] / 'quality-report.json'
    quality_raw = quality_path.read_bytes()
    if hashlib.sha256(quality_raw).hexdigest() != source['quality_sha256']:
        raise ValueError(f'{symbol}: quality report changed since replay')
    quality = json.loads(quality_raw)
    files = [{'root': source['root'], 'source': source['source'],
              'quality_sha256': source['quality_sha256']}]
    for name, metadata in source['outputs'].items():
        if '-M1-' in name and name.endswith('.csv'):
            files.append({'prepared': f"{source['root']}/prepared/{name}",
                          'sha256': metadata['sha256']})
    return quality, files


def _load_prices(database, plan, symbols, start_ms, cutoff_ms):
    native, coverage, source_map = {}, {}, {}
    for symbol in symbols:
        quality, records = _mtm_source(plan, symbol)
        result = {'from': start_ms, 'to': cutoff_ms, 'symbols': [symbol],
                  'sources': {symbol: {'files': records, 'excluded_intervals': quality.get('gaps', [])}},
                  'trades': [], 'summary': {}}
        times, prices, cov = MTM.load_common_quotes(database, result)
        native[symbol] = (times, prices[symbol])
        coverage[symbol] = cov['by_symbol'][symbol]
        source_map[symbol] = quality
    common_times = native[symbols[0]][0].copy()
    for symbol in symbols[1:]:
        common_times = np.intersect1d(common_times, native[symbol][0], assume_unique=True)
    if not len(common_times):
        raise ValueError('no common observed M1 closes')
    common_prices = {s: native[s][1][np.searchsorted(native[s][0], common_times)] for s in symbols}
    portfolio_coverage = {'quote_time': 'M1 close at UTC minute start + 59 seconds',
                          'by_symbol': coverage, 'common_observations': int(len(common_times)),
                          'common_first_ms': int(common_times[0]),
                          'common_last_ms': int(common_times[-1]),
                          'gaps_are_not_filled': True}
    return native, (common_times, common_prices, portfolio_coverage), source_map


def _metric_record(report):
    terminal = terminal_mark_summary(report)
    return {'closed': report['closed_only']['computed']['closed'],
            'closed_r': report['closed_only']['computed']['net_r'],
            'closed_drawdown_r': report['closed_only']['computed']['max_closed_drawdown_r'],
            'terminal': terminal,
            'mtm_drawdown_r': report['gross']['max_mark_to_market_drawdown_r'],
            'peak_open_positions': report['risk']['peak_concurrent_trades_on_marks'],
            'peak_open_initial_risk_r': report['risk']['max_open_initial_risk_r_on_marks'],
            'mtm_series': report['series']}


def _yearly(rows_by_symbol):
    periods = {}
    for symbol, rows in rows_by_symbol.items():
        for trade in rows:
            if trade['state'] not in ('Stop', 'TakeProfit') or trade['exit_at'] is None:
                continue
            year = str(datetime.fromtimestamp(trade['exit_at'] / 1000, timezone.utc).year)
            item = periods.setdefault(year, {'closed': 0, 'closed_r': 0.0, 'by_symbol': {}})
            by_symbol = item['by_symbol'].setdefault(symbol, {'closed': 0, 'closed_r': 0.0})
            item['closed'] += 1; item['closed_r'] += trade['result_r']
            by_symbol['closed'] += 1; by_symbol['closed_r'] += trade['result_r']
    return dict(sorted(periods.items()))


def build_report(run_dir, database):
    run_dir = Path(run_dir)
    plan, completion = _load_plan(run_dir)
    start_ms = int(datetime.fromisoformat(plan['from_utc'].replace('Z', '+00:00')).timestamp() * 1000)
    cutoff_ms = int(datetime.fromisoformat(plan['to_utc_exclusive'].replace('Z', '+00:00')).timestamp() * 1000)
    if start_ms >= cutoff_ms:
        raise ValueError('invalid report period')
    configs, csv_paths, output_records, scoped, stats = {}, {}, {}, {}, {}
    for symbol in SYMBOLS:
        configs[symbol], csv_paths[symbol], output_records[symbol] = _load_csv_and_config(
            run_dir, plan, completion, symbol)
        scoped[symbol], stats[symbol] = read_scoped_trades(
            csv_paths[symbol], symbol, start_ms, cutoff_ms)
    native, common, qualities = _load_prices(database, plan, SYMBOLS, start_ms, cutoff_ms)
    common_times, common_prices, common_coverage = common
    def calc(symbols, rows, times, prices, coverage):
        result = {'from': start_ms, 'to': cutoff_ms, 'symbols': list(symbols),
                  'sources': {}, 'trades': rows, 'summary': {}}
        return MTM.calculate(result, times, prices, coverage)
    pair_reports = {}
    for symbol in SYMBOLS:
        pair_reports[symbol] = _metric_record(calc(
            [symbol], scoped[symbol], native[symbol][0], {symbol: native[symbol][1]},
            {'by_symbol': {symbol: common_coverage['by_symbol'][symbol]},
             'common_observations': len(native[symbol][0])}))
    all_rows = [row for symbol in SYMBOLS for row in scoped[symbol]]
    portfolio = _metric_record(calc(SYMBOLS, all_rows, common_times, common_prices, common_coverage))
    terminal_ms = int(portfolio['terminal']['mark_utc_ms'])
    latest = {s: int(native[s][0][-1]) for s in SYMBOLS}
    result = {
        'schema_version': 1,
        'title': 'Исторический отчёт по стратегии Swing',
        'real_market_data_note': 'Реальные исторические котировки MT5; это не реальные брокерские сделки.',
        'period': {'from_utc_ms': start_ms, 'to_utc_exclusive_ms': cutoff_ms,
                   'configured_start_source_grid': plan['configured_start_source_grid'],
                   'warmup_from_utc': plan['warmup_from_utc'],
                   'last_common_mark_utc_ms': terminal_ms},
        'symbols': list(SYMBOLS), 'source': 'MT5 broker export (user terminal, M1 bars)',
        'origin': 'mt5_m1_bars', 'timezone': 'UTC', 'price_basis': 'exported bid OHLC used as index price',
        'execution_model': {'bars': 'M1 OHLC assembled by the shared aggregator',
                            'intrabar': 'full M1 OHLC range; no tick path reconstructed',
                            'stop_target_priority': 'stop first when one M1 bar touches both stop and target',
                            'costs': 'configured cost applied once against each trade on entry by Swing',
                            'risk_unit': 'R is each trade divided by its initial price risk; not percent or money'},
        'limitations': ['Market gaps are preserved; absent prices are not filled forward.',
                        'M1 OHLC does not identify the actual within-minute price path.',
                        'The model is not live broker execution and does not include margin or cash sizing.',
                        'Per-pair files extend to their own last observation; all reported metrics are censored at the common cutoff.'],
        'coverage': {'common_observations': int(len(common_times)),
                     'common_first_mark_utc_ms': int(common_times[0]),
                     'common_last_mark_utc_ms': int(common_times[-1]),
                     'per_symbol_last_mark_utc_ms': latest,
                     'by_symbol': common_coverage['by_symbol'],
                     'no_gap_filling': True},
        'configs': configs, 'input_provenance': {}, 'completion': {},
        'portfolio': portfolio, 'by_symbol': pair_reports,
        'trade_scope': stats, 'yearly_closed': _yearly(scoped),
    }
    for symbol in SYMBOLS:
        source = plan['sources'][symbol]
        result['input_provenance'][symbol] = {
            'source': source['source'], 'origin': source['origin'],
            'quality_sha256': source['quality_sha256'],
            'input_file': source['input'], 'first_minute_utc': source['first_minute_utc'],
            'last_minute_utc': source['last_minute_utc'],
            'source_gap_intervals': len(qualities[symbol].get('gaps', [])),
            'missing_months': qualities[symbol].get('missing_months', []),
            'config_sha256': plan['config_sha256'][symbol],
            'trade_csv': output_records[symbol],
            'config': configs[symbol],
        }
    result['completion'] = {'run_plan_sha256': hashlib.sha256((run_dir / 'run-plan.json').read_bytes()).hexdigest(),
                            'binary_sha256': plan['binary_sha256'], 'docker_image': plan['docker_image'],
                            'git_head': plan['git_head'], 'database_sha256': _sha(database)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run_dir', type=Path)
    parser.add_argument('--database', type=Path, default=ROOT / 'data/forex.sqlite')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    output = args.output or args.run_dir / 'report.json'
    if output.exists():
        raise ValueError(f'output already exists: {output}')
    report = build_report(args.run_dir, args.database)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x', encoding='utf-8') as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write('\n')
    print(json.dumps({'output': str(output), 'symbols': report['symbols'],
                      'closed': report['portfolio']['closed'],
                      'closed_r': report['portfolio']['closed_r'],
                      'terminal_marked_r': report['portfolio']['terminal']['marked_equity_r'],
                      'mtm_drawdown_r': report['portfolio']['mtm_drawdown_r']}, ensure_ascii=False))


if __name__ == '__main__':
    main()
