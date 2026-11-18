import copy
import importlib.util
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest

import numpy as np

MODULE = Path(__file__).resolve().parents[1] / 'scripts/research_mark_to_market.py'
spec = importlib.util.spec_from_file_location('research_mark_to_market', MODULE)
mtm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mtm)


def trade(symbol='EURUSD', side='Long', state='Active', entry=1000, stop=900,
          entry_at=0, exit_at=None, result_r=None):
    return dict(id='test', symbol=symbol, side=side, state=state, entry=entry,
                stop=stop, entry_at=entry_at, exit_at=exit_at, result_r=result_r)


def source_declaration(source='fixture', root='/fixture/root'):
    return {'root': root, 'source': source, 'quality_sha256': 'a' * 64}


def content_record(path='prepared/EURUSD-M1.csv'):
    return {'prepared': path, 'sha256': 'b' * 64}


def result(trades, symbols=None):
    symbols = symbols or ['EURUSD']
    return {'from': 0, 'to': 180000, 'symbols': symbols,
            'sources': {symbol: {'files': [
                            source_declaration(), content_record(path=f'prepared/{symbol}-M1.csv')
                        ], 'excluded_intervals': []} for symbol in symbols},
            'trades': trades, 'summary': {}}


class MarkToMarketTests(unittest.TestCase):
    def test_open_drawdown_cost_at_entry_and_risk_count(self):
        r = mtm.calculate(result([trade()]), np.array([59000, 119000, 179000]),
                          {'EURUSD': np.array([1000, 800, 1100])})
        self.assertEqual(r['gross']['max_mark_to_market_drawdown_r'], 2)
        self.assertEqual(r['gross']['last_equity_r'], 1)
        self.assertEqual(r['stress_1pip']['max_mark_to_market_drawdown_r'], 3)
        self.assertEqual(r['stress_1pip']['last_equity_r'], 0)
        self.assertEqual(r['risk']['peak_concurrent_trades_on_marks'], 1)
        self.assertEqual(r['risk']['max_open_initial_risk_r_on_marks'], 1)
        self.assertEqual(r['closed_only']['computed']['closed'], 0)

    def test_exit_is_realized_at_equality_and_pending_is_ignored(self):
        tr = trade(state='TakeProfit', exit_at=119000, result_r=1)
        pending = trade(state='Pending', entry_at=None)
        r = mtm.calculate(result([tr, pending]), np.array([59000,119000,179000]),
                          {'EURUSD': np.array([950, 10000, 5000])})
        self.assertEqual(r['gross']['last_equity_r'], 1)
        self.assertEqual(r['closed_only']['computed']['net_r'], 1)
        self.assertEqual(r['series'][-1]['open_trades'], 0)
        self.assertEqual(r['stress_1pip']['last_equity_r'], 0)

    def test_sqlite_common_grid_never_fills_missing_pair_quote(self):
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'history.sqlite'
            db=sqlite3.connect(path)
            db.executescript((MODULE.parents[1]/'data/example.sql').read_text())
            for symbol, stamps in [('EURUSD',[0,60,120]),('GBPUSD',[0,120])]:
                for ts in stamps:
                    db.execute('INSERT INTO minutes VALUES(?,?,?,?)',(symbol,'fixture',ts,json.dumps(
                        {'timestamp':ts,'price':[1000,1001,999,1000+ts], 'quotes':1,'gap_before_minutes':0})))
            db.commit();db.close()
            before=path.read_bytes()
            times,prices,coverage=mtm.load_common_quotes(path,result([],['EURUSD','GBPUSD']))
            self.assertEqual(times.tolist(),[59000,179000])
            self.assertEqual(prices['GBPUSD'].tolist(),[1000,1120])
            self.assertEqual(coverage['common_missing_minute_slots'],1)
            self.assertEqual(coverage['missing_intervals_ms'],[[119000,179000]])
            self.assertEqual(path.read_bytes(),before)

    def test_sqlite_accepts_source_declaration_and_source_free_content_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'history.sqlite'
            db = sqlite3.connect(path)
            db.executescript((MODULE.parents[1] / 'data/example.sql').read_text())
            for ts in (0, 60, 120):
                db.execute('INSERT INTO minutes VALUES(?,?,?,?)', ('EURUSD', 'fixture-a', ts,
                    json.dumps({'timestamp': ts, 'price': [1000, 1001, 999, 1000],
                                'quotes': 1, 'gap_before_minutes': 0})))
            db.commit()
            db.close()
            payload = result([], ['EURUSD'])
            payload['sources']['EURUSD']['files'] = [
                source_declaration('fixture-a', '/prepared/EURUSD'),
                content_record('/prepared/EURUSD/prepared/EURUSD-M1-index-2026-01.csv'),
            ]
            before = path.read_bytes()
            times, prices, coverage = mtm.load_common_quotes(path, payload)
            self.assertEqual(times.tolist(), [59000, 119000, 179000])
            self.assertEqual(coverage['by_symbol']['EURUSD']['source'], 'fixture-a')
            self.assertEqual(path.read_bytes(), before)

    def test_sqlite_rejects_invalid_provenance_mutations_of_a_matching_control(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'history.sqlite'
            db = sqlite3.connect(path)
            db.executescript((MODULE.parents[1] / 'data/example.sql').read_text())
            for ts in (0, 60, 120):
                db.execute('INSERT INTO minutes VALUES(?,?,?,?)', ('EURUSD', 'fixture', ts,
                    json.dumps({'timestamp': ts, 'price': [1000, 1001, 999, 1000],
                                'quotes': 1, 'gap_before_minutes': 0})))
            db.commit()
            db.close()
            valid = result([], ['EURUSD'])
            before = path.read_bytes()
            times, _prices, coverage = mtm.load_common_quotes(path, valid)
            self.assertEqual(times.tolist(), [59000, 119000, 179000])
            self.assertEqual(coverage['by_symbol']['EURUSD']['source'], 'fixture')
            self.assertEqual(path.read_bytes(), before)

            cases = []
            missing_map = copy.deepcopy(valid)
            del missing_map['sources']
            cases.append(('missing source map', missing_map, 'missing result source provenance'))
            empty = copy.deepcopy(valid)
            empty['sources']['EURUSD']['files'] = []
            cases.append(('empty source list', empty, 'missing result source provenance'))
            mismatch = copy.deepcopy(valid)
            mismatch['sources']['EURUSD']['files'][0]['source'] = 'fixture-b'
            cases.append(('database mismatch', mismatch, 'result/database source mismatch'))
            whitespace_collision = copy.deepcopy(valid)
            whitespace_collision['sources']['EURUSD']['files'].append(
                source_declaration(' fixture ', '/fixture/other-root'))
            cases.append(('distinct whitespace-bearing declarations', whitespace_collision,
                          'mixed result source provenance'))
            malformed_records = [
                ({}, 'invalid content provenance record'),
                ({'root': '/fixture/root'}, 'invalid source declaration'),
                ({'root': '/fixture/root', 'source': ''}, 'invalid source declaration'),
                ({'root': '/fixture/root', 'source': '  '}, 'invalid source declaration'),
                ({'prepared': '/fixture/file.csv'}, 'invalid content provenance record'),
                ({'prepared': '', 'sha256': 'b' * 64}, 'invalid content provenance record'),
                ({'prepared': '/fixture/file.csv', 'sha256': 'not-a-digest'},
                 'invalid content provenance record'),
                ({'prepared': '/fixture/file.csv', 'sha256': 'a' * 31 + '_' + 'a' * 32},
                 'invalid content provenance record'),
                ('fixture', 'invalid result provenance record'),
            ]
            for record, message in malformed_records:
                mutated = copy.deepcopy(valid)
                mutated['sources']['EURUSD']['files'].append(record)
                cases.append((f'malformed record {record!r}', mutated, message))

            for label, invalid, message in cases:
                with self.subTest(label=label):
                    with self.assertRaisesRegex(ValueError, message):
                        mtm.load_common_quotes(path, invalid)
            self.assertEqual(path.read_bytes(), before)

    def test_sqlite_preserves_whitespace_in_a_matching_source_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'history.sqlite'
            source = ' fixture '
            db = sqlite3.connect(path)
            db.executescript((MODULE.parents[1] / 'data/example.sql').read_text())
            for ts in (0, 60, 120):
                db.execute('INSERT INTO minutes VALUES(?,?,?,?)', ("EURUSD", source, ts,
                    json.dumps({'timestamp': ts, 'price': [1000, 1001, 999, 1000],
                                'quotes': 1, 'gap_before_minutes': 0})))
            db.commit()
            db.close()
            payload = result([], ['EURUSD'])
            payload['sources']['EURUSD']['files'][0] = source_declaration(source)
            before = path.read_bytes()
            times, _prices, coverage = mtm.load_common_quotes(path, payload)
            self.assertEqual(times.tolist(), [59000, 119000, 179000])
            self.assertEqual(coverage['by_symbol']['EURUSD']['source'], source)
            self.assertEqual(path.read_bytes(), before)


    def test_jpy_short_pip_scale_and_unfilled_time(self):
        tr=trade(symbol='USDJPY',side='Short',entry=150000000,stop=151000000,entry_at=60000)
        r=mtm.calculate(result([tr],['USDJPY']),np.array([59000,119000,179000]),
                        {'USDJPY':np.array([155000000,150000000,151000000])})
        self.assertEqual(r['series'][0]['gross_equity_r'],0)
        self.assertAlmostEqual(r['stress_1pip']['last_equity_r'],-1.01)
        self.assertEqual(r['gross']['max_mark_to_market_drawdown_r'],1)

    def test_two_independent_trades_sum_initial_risk(self):
        a=trade(); b={**trade(),'id':'second'}
        r=mtm.calculate(result([a,b]),np.array([59000,119000,179000]),
                        {'EURUSD':np.array([1000,900,1000])})
        self.assertEqual(r['risk']['max_open_initial_risk_r_on_marks'],2)
        self.assertEqual(r['gross']['max_mark_to_market_drawdown_r'],2)


if __name__=='__main__':
    unittest.main()
