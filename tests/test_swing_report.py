import csv
from pathlib import Path
import tempfile
import unittest

import numpy as np

from scripts import swing_report
from scripts import research_mark_to_market as mtm


class SwingReportTests(unittest.TestCase):
    def test_fractional_millionth_entry_and_stop_preserve_marked_r(self):
        entry = swing_report.index_price_units('1.1000004')
        stop = swing_report.index_price_units('1.0999991')
        self.assertEqual(entry, float('1.1000004') * 1_000_000)
        self.assertEqual(stop, float('1.0999991') * 1_000_000)
        result = {
            'from': 0, 'to': 120000, 'symbols': ['EURUSD'], 'sources': {},
            'trades': [{'id': 'EURUSD:1', 'symbol': 'EURUSD', 'side': 'Long',
                        'state': 'Active', 'entry_at': 59000, 'entry': entry,
                        'stop': stop, 'exit_at': None, 'result_r': None}],
            'summary': {},
        }
        marks = mtm.calculate(result, np.array([59000, 119000]),
                              {'EURUSD': np.array([1100000, 1100001.7])})
        self.assertAlmostEqual(marks['gross']['last_equity_r'], 1.0)

    def test_source_grid_inverse_and_cutoff_censoring(self):
        self.assertEqual(swing_report.grid_to_utc_ms('2020-09-01 00:00:00'),
                         1598907600000)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'EURUSD.csv'
            fields = ['kind', 'dir', 'signal_t', 'fill_t', 'entry', 'stop', 'tp', 'rr',
                      'state', 'r', 'exit_t']
            records = [
                {'kind': 'trend', 'dir': '1', 'signal_t': '2026-11-12 10:00:00',
                 'fill_t': '2026-11-12 10:05:00', 'entry': '1.1', 'stop': '1.09',
                 'tp': '1.12', 'rr': '2', 'state': 'tp', 'r': '1.5',
                 'exit_t': '2026-11-12 10:16:00'},
                {'kind': 'range', 'dir': '-1', 'signal_t': '2026-11-12 10:16:00',
                 'fill_t': '2026-11-12 10:16:00', 'entry': '1.1', 'stop': '1.11',
                 'tp': '1.08', 'rr': '2', 'state': 'sl', 'r': '-1',
                 'exit_t': '2026-11-12 10:20:00'},
                {'kind': 'range', 'dir': '1', 'signal_t': '2026-11-11 10:00:00',
                 'fill_t': '', 'entry': '1.1', 'stop': '1.09', 'tp': '1.12',
                 'rr': '2', 'state': 'cancelled', 'r': '', 'exit_t': ''},
            ]
            with path.open('w', newline='') as stream:
                writer = csv.DictWriter(stream, fieldnames=fields)
                writer.writeheader()
                writer.writerows(records)
            rows, stats = swing_report.read_scoped_trades(
                path, 'EURUSD', 1598907600000, 1794471360000)
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]['state'], 'Active')
            self.assertIsNone(rows[0]['exit_at'])
            self.assertIsNone(rows[0]['result_r'])
            self.assertEqual(stats['censored_at_cutoff'], 1)
            self.assertEqual(stats['fills_at_or_after_cutoff'], 1)
            self.assertEqual(stats['unfilled_or_cancelled'], 1)

    def test_terminal_values_come_from_last_common_mark(self):
        result = {
            'from': 0, 'to': 240000, 'symbols': ['EURUSD'], 'sources': {},
            'trades': [{
                'id': 'EURUSD:1', 'symbol': 'EURUSD', 'side': 'Long', 'state': 'TakeProfit',
                'entry_at': 59000, 'entry': 1100000, 'stop': 1000000,
                'exit_at': 179000, 'result_r': 1.0,
            }], 'summary': {},
        }
        marks = mtm.calculate(result, np.array([59000, 119000]),
                              {'EURUSD': np.array([1100000, 1050000])})
        terminal = swing_report.terminal_mark_summary(marks)
        self.assertEqual(terminal['realized_r'], 0.0)
        self.assertEqual(terminal['unrealized_r'], -0.5)
        self.assertEqual(terminal['marked_equity_r'], -0.5)
        self.assertEqual(terminal['open_positions'], 1)
        self.assertEqual(marks['closed_only']['computed']['net_r'], 1.0)


if __name__ == '__main__':
    unittest.main()
