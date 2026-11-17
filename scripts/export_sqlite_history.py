"""Read-only lossless SQLite v4 -> prepared MT5 history for forex-backtest.

Preserves stored OHLC and gap decisions. Does not infer missing quotes or change
source/timezone. Original import reports are retained beside derived reports.
"""
import argparse
import csv
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sqlite3
import time

HEADER = ['timestamp_utc', 'timestamp_ms', 'bar_end_utc', 'open', 'high', 'low',
          'close', 'quote_count', 'gap_before_minutes']


def iso(ts):
    return datetime.fromtimestamp(ts, timezone.utc).isoformat(timespec='seconds').replace('+00:00', 'Z')


def file_hash(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def price(n):
    if type(n) is not int or n <= 0:
        raise ValueError('invalid stored price')
    return f'{n // 1000000}.{n % 1000000:06}'


def export(database, output, runtime_root):
    database, output = Path(database).resolve(), Path(output)
    if output.exists() and any(output.iterdir()):
        raise ValueError('output directory is not empty')
    if not database.is_file():
        raise ValueError('database does not exist')
    before = file_hash(database)
    started = time.monotonic()
    db = sqlite3.connect(database.as_uri() + '?mode=ro', uri=True)
    try:
        db.execute('BEGIN')
        if (db.execute('PRAGMA application_id').fetchone()[0] != 1180190513 or
                db.execute('PRAGMA user_version').fetchone()[0] != 4):
            raise ValueError('unsupported database schema')
        groups = db.execute('SELECT symbol,source,count(*),min(timestamp),max(timestamp) '
                            'FROM minutes GROUP BY symbol,source ORDER BY symbol').fetchall()
        if not groups:
            raise ValueError('no minutes')
        seen, reports = set(), {}
        for symbol, source, count, first, last in groups:
            if symbol in seen:
                raise ValueError('mixed history source for symbol')
            if len(symbol) != 6 or not symbol.isascii() or not symbol.isalpha() or not symbol.isupper():
                raise ValueError('invalid symbol')
            seen.add(symbol)
            originals = [{'digest': digest, 'report': json.loads(report)} for digest, report in
                         db.execute('SELECT digest,report FROM imports WHERE symbol=? ORDER BY digest',
                                    (symbol,))]
            if len(originals) != 1:
                raise ValueError('expected exactly one original import report per symbol')
            q = originals[0]['report']
            if (q.get('source') != source or q.get('symbol') != symbol or
                    q.get('origin') != 'mt5_m1_bars' or q.get('price_basis') != 'index' or
                    q.get('timezone') != 'UTC' or not isinstance(q.get('gaps'), list) or
                    not isinstance(q.get('missing_months'), list)):
                raise ValueError('unsupported source metadata')
            reports[symbol] = originals
        output.mkdir(parents=True, exist_ok=True)
        config, summaries = {}, {}
        for symbol, source, expected_count, first, last in groups:
            root = output / symbol
            prepared = root / 'prepared'
            prepared.mkdir(parents=True)
            originals = reports[symbol]
            q = dict(originals[0]['report'])
            write_json(root / 'original-imports.json', originals)
            outputs = {}
            handle, current_month, path, writer = None, None, None, None
            count, actual_first, actual_last = 0, None, None
            row_hash = hashlib.sha256()
            try:
                for ts, payload in db.execute('SELECT timestamp,payload FROM minutes '
                                               'WHERE symbol=? AND source=? ORDER BY timestamp',
                                               (symbol, source)):
                    m = json.loads(payload)
                    ohlc = m['price']
                    if (m['timestamp'] != ts or ts % 60 or len(ohlc) != 4 or
                            type(m['quotes']) is not int or m['quotes'] <= 0 or
                            type(m['gap_before_minutes']) is not int or m['gap_before_minutes'] < 0 or
                            (actual_last is not None and ts <= actual_last)):
                        raise ValueError('invalid stored minute')
                    p = [price(n) for n in ohlc]
                    o, h, low, close = ohlc
                    if not low <= o <= h or not low <= close <= h:
                        raise ValueError('invalid stored OHLC')
                    stamp = iso(ts)
                    month = stamp[:7]
                    if month != current_month:
                        if handle:
                            handle.close()
                            outputs[path.name] = {'bytes': path.stat().st_size, 'sha256': file_hash(path)}
                        path = prepared / f'{symbol}-M1-index-{month}.csv'
                        handle = path.open('w', newline='')
                        writer = csv.writer(handle, lineterminator='\n')
                        writer.writerow(HEADER)
                        current_month = month
                    writer.writerow([stamp, ts * 1000, iso(ts + 60), *p,
                                     m['quotes'], m['gap_before_minutes']])
                    row_hash.update((json.dumps([ts, ohlc, m['quotes'], m['gap_before_minutes']],
                                                 separators=(',', ':')) + '\n').encode())
                    count += 1
                    actual_first = ts if actual_first is None else actual_first
                    actual_last = ts
            finally:
                if handle:
                    handle.close()
            if path:
                outputs[path.name] = {'bytes': path.stat().st_size, 'sha256': file_hash(path)}
            if (count, actual_first, actual_last) != (expected_count, first, last):
                raise ValueError('export row count or boundary mismatch')
            gap_path = prepared / 'gaps.csv'
            with gap_path.open('w', newline='') as f:
                w = csv.writer(f, lineterminator='\n')
                w.writerow(['gap_start_utc', 'gap_end_utc', 'missing_minutes'])
                for gap in q['gaps']:
                    a, b = gap['from_ms'], gap['to_ms']
                    if a % 60000 or b % 60000 or b <= a:
                        raise ValueError('unsupported gap timestamps')
                    w.writerow([iso(a // 1000), iso(b // 1000), (b - a) // 60000])
            outputs['gaps.csv'] = {'bytes': gap_path.stat().st_size, 'sha256': file_hash(gap_path)}
            q['outputs'] = outputs
            q['sqlite_export'] = {'database_name': database.name, 'database_sha256': before,
                                  'rows': count, 'canonical_rows_sha256': row_hash.hexdigest(),
                                  'first_utc': iso(first), 'last_utc': iso(last),
                                  'original_reports': 'original-imports.json',
                                  'conversion': 'stored integer price / 1000000; no changes or gap filling'}
            write_json(root / 'quality-report.json', q)
            config[symbol] = [str(Path(runtime_root) / symbol)]
            summaries[symbol] = q['sqlite_export']
            print(f'{symbol}: {count} rows, {iso(first)} .. {iso(last)}', flush=True)
    finally:
        db.close()
    after = file_hash(database)
    if before != after:
        raise ValueError('source database changed during export')
    write_json(output / 'data-config.json', config)
    summary = {'database_sha256_before': before, 'database_sha256_after': after,
               'symbols': summaries, 'elapsed_seconds': round(time.monotonic() - started, 3)}
    write_json(output / 'export-summary.json', summary)
    return summary


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('database', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--runtime-root', required=True,
                   help='Dataset root as visible to forex-backtest, e.g. /workspace/data/...')
    args = p.parse_args()
    result = export(args.database, args.output, args.runtime_root)
    print(json.dumps(result, ensure_ascii=False, indent=2))
