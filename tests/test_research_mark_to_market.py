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


def result(trades, symbols=None):
    symbols = symbols or ['EURUSD']
    return {'from': 0, 'to': 180000, 'symbols': symbols,
            'sources': {symbol: {'files': [{'source': 'fixture'}],
                        'excluded_intervals': []} for symbol in symbols},
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

    def test_sqlite_requires_matching_unambiguous_result_provenance(self):
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

            cases = []
            missing = result([], ['EURUSD'])
            del missing['sources']
            cases.append(('missing source map', missing))
            empty = result([], ['EURUSD'])
            empty['sources']['EURUSD']['files'] = []
            cases.append(('empty source list', empty))
            mismatch = result([], ['EURUSD'])
            mismatch['sources']['EURUSD']['files'][0]['source'] = 'fixture-b'
            cases.append(('database mismatch', mismatch))
            mixed = result([], ['EURUSD'])
            mixed['sources']['EURUSD']['files'].append({'source': 'fixture-b'})
            cases.append(('mixed result provenance', mixed))
            for label, invalid in [
                    ('empty object', {}),
                    ('missing source', {'filename': 'other.csv'}),
                    ('blank source', {'source': ''}),
                    ('whitespace source', {'source': '  '}),
                    ('non-object provenance', 'fixture-a')]:
                malformed = result([], ['EURUSD'])
                malformed['sources']['EURUSD']['files'][0]['source'] = 'fixture-a'
                malformed['sources']['EURUSD']['files'].append(invalid)
                cases.append((label, malformed))

            for label, item in cases:
                with self.subTest(label=label):
                    with self.assertRaises(ValueError):
                        mtm.load_common_quotes(path, item)

            matching = result([], ['EURUSD'])
            matching['sources']['EURUSD']['files'][0]['source'] = 'fixture-a'
            matching['sources']['EURUSD']['files'].append({'source': 'fixture-a'})
            times, prices, coverage = mtm.load_common_quotes(path, matching)
            self.assertEqual(times.tolist(), [59000, 119000, 179000])
            self.assertEqual(coverage['by_symbol']['EURUSD']['source'], 'fixture-a')

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
