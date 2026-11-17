"""Lossless, read-only SQLite history export for the existing backtest CLI."""
import csv
from contextlib import closing
import hashlib
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'scripts/export_sqlite_history.py'
spec = importlib.util.spec_from_file_location('export_sqlite_history', MODULE)
exporter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exporter)


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.db = self.root / 'history.sqlite'
        self.out = self.root / 'export'
        self.report = {'symbol': 'EURUSD', 'source': 'fixture bid', 'origin': 'mt5_m1_bars',
                       'index_model': 'exported bid OHLC used as the index price',
                       'price_basis': 'index', 'timezone': 'UTC', 'missing_months': [],
                       'gaps': [{'from_ms': 1577923260000, 'to_ms': 1577923380000}],
                       'outputs': {'old.csv': {'sha256': 'original'}}}
        with closing(sqlite3.connect(self.db)) as db, db:
            db.executescript((MODULE.parents[1] / 'data/example.sql').read_text())
            db.execute('INSERT INTO imports VALUES(?,?,?)',
                       ('EURUSD', 'original-digest', json.dumps(self.report)))
            for ts, price, count, gap in [
                (1577923200, [1123456, 1123556, 1123356, 1123500], 17, 0),
                (1577923380, [1123500, 1123600, 1123400, 1123550], 31, 2),
            ]:
                db.execute('INSERT INTO minutes VALUES(?,?,?,?)',
                           ('EURUSD', 'fixture bid', ts, json.dumps({
                               'timestamp': ts, 'price': price, 'quotes': count,
                               'gap_before_minutes': gap})))

    def test_exact_prices_times_counts_gaps_and_provenance(self):
        before = self.db.read_bytes()
        exporter.export(self.db, self.out, '/workspace/data/export')
        self.assertEqual(self.db.read_bytes(), before)
        self.assertFalse(Path(str(self.db) + '-wal').exists())
        q = json.loads((self.out / 'EURUSD/quality-report.json').read_text())
        self.assertEqual(q['source'], self.report['source'])
        self.assertEqual(q['gaps'], self.report['gaps'])
        self.assertEqual(q['index_model'], self.report['index_model'])
        original = json.loads((self.out / 'EURUSD/original-imports.json').read_text())
        self.assertEqual(original[0], {'digest': 'original-digest', 'report': self.report})
        name = 'EURUSD-M1-index-2020-01.csv'
        path = self.out / 'EURUSD/prepared' / name
        self.assertEqual(q['outputs'][name]['sha256'], hashlib.sha256(path.read_bytes()).hexdigest())
        self.assertEqual(q['outputs'][name]['bytes'], path.stat().st_size)
        with path.open() as f:
            rows = list(csv.DictReader(f))
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0], dict(zip(exporter.HEADER, [
            '2020-01-02T00:00:00Z', '1577923200000', '2020-01-02T00:01:00Z',
            '1.123456', '1.123556', '1.123356', '1.123500', '17', '0'])))
        self.assertEqual(rows[1]['gap_before_minutes'], '2')
        self.assertEqual(rows[1]['timestamp_ms'], '1577923380000')
        self.assertEqual(json.loads((self.out / 'data-config.json').read_text()),
                         {'EURUSD': ['/workspace/data/export/EURUSD']})

    def test_nonempty_destination_is_never_overwritten(self):
        self.out.mkdir()
        (self.out / 'sentinel').write_text('keep')
        with self.assertRaisesRegex(ValueError, 'not empty'):
            exporter.export(self.db, self.out, '/workspace/data/export')
        self.assertEqual((self.out / 'sentinel').read_text(), 'keep')

    def test_foreign_database_is_rejected(self):
        with closing(sqlite3.connect(self.db)) as db, db:
            db.execute('PRAGMA user_version=3')
        with self.assertRaisesRegex(ValueError, 'schema'):
            exporter.export(self.db, self.out, '/workspace/data/export')

    def test_mixed_source_is_rejected(self):
        with closing(sqlite3.connect(self.db)) as db, db:
            db.execute("UPDATE minutes SET source='another' WHERE timestamp=1577923380")
        with self.assertRaisesRegex(ValueError, 'source'):
            exporter.export(self.db, self.out, '/workspace/data/export')


if __name__ == '__main__':
    unittest.main()
