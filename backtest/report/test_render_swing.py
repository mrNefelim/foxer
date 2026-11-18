"""PDF contract checks for the standalone Swing report renderer."""
import json
from pathlib import Path
import tempfile
import unittest

from pypdf import PdfReader

from render_swing import render


def sample_report():
    symbols = ['EURUSD', 'GBPUSD', 'USDJPY']
    cfg = {'rr_fixed': 2.0, 'take_profit': 'fixed', 'rr_target_cap': 5.0,
           'plan_days': 10, 'stop_lookback_h4_bars': 30, 'stop_buffer_atr': 0.05,
           'cost': 0.0001, 'range': {'enabled': True}}
    metric = {'closed': 2, 'closed_r': 1.5, 'closed_drawdown_r': 0.5,
              'terminal': {'marked_equity_r': 1.4, 'realized_r': 1.5,
                           'unrealized_r': -0.1, 'open_positions': 1},
              'mtm_drawdown_r': 0.6, 'peak_open_positions': 2,
              'peak_open_initial_risk_r': 2,
              'mtm_series': [{'millis': 1598918459000, 'gross_equity_r': 0.0, 'open_trades': 0},
                             {'millis': 1794471359000, 'gross_equity_r': 1.4, 'open_trades': 1}]}
    return {
        'schema_version': 1, 'title': 'Исторический отчёт по стратегии Swing',
        'real_market_data_note': 'Реальные исторические котировки MT5; это не реальные брокерские сделки.',
        'period': {'from_utc_ms': 1598907600000, 'to_utc_exclusive_ms': 1794471360000,
                   'configured_start_source_grid': '2020-09-01 00:00:00',
                   'warmup_from_utc': '2020-01-02T04:00:00Z'},
        'symbols': symbols, 'source': 'MT5 broker export (user terminal, M1 bars)',
        'timezone': 'UTC', 'price_basis': 'exported bid OHLC used as index price',
        'limitations': ['Test limitation: no real execution.'],
        'coverage': {'common_observations': 2, 'common_last_mark_utc_ms': 1794471359000,
                     'by_symbol': {s: {'observations': 3} for s in symbols}},
        'configs': {s: cfg for s in symbols}, 'portfolio': metric,
        'by_symbol': {s: metric for s in symbols},
        'yearly_closed': {'2026': {'closed': 6, 'closed_r': 4.5,
                                   'by_symbol': {s: {'closed_r': 1.5} for s in symbols}}},
        'input_provenance': {s: {'last_minute_utc': '2026-11-12T08:15:00Z',
                                 'input_file': {'rows': 100}, 'source_gap_intervals': 1,
                                 'missing_months': [], 'quality_sha256': 'a' * 64,
                                 'trade_csv': {'sha256': 'b' * 64}, 'config_sha256': 'c' * 64}
                             for s in symbols},
        'completion': {'run_plan_sha256': 'd' * 64, 'binary_sha256': 'e' * 64,
                       'database_sha256': 'f' * 64, 'docker_image': 'sha256:' + '1' * 64},
    }


class SwingPdfTests(unittest.TestCase):
    def test_pdf_reports_real_data_scope_methods_pairs_and_terminal_split(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            report_path, pdf_path = root / 'report.json', root / 'report.pdf'
            report_path.write_text(json.dumps(sample_report(), ensure_ascii=False), encoding='utf-8')
            render(report_path, pdf_path)
            text = '\n'.join(page.extract_text() or '' for page in PdfReader(pdf_path).pages)
            for expected in ('EURUSD', 'GBPUSD', 'USDJPY', 'OHLC M1',
                             'первым считается stop', 'не реальные брокерские сделки',
                             '1.50 R', '1.40 R', '1.50 / -0.10 R',
                             '2020-09-01 00:00:00'):
                self.assertIn(expected, text)
            self.assertGreaterEqual(len(PdfReader(pdf_path).pages), 3)
            with self.assertRaises(FileExistsError):
                render(report_path, pdf_path)


if __name__ == '__main__':
    unittest.main()
