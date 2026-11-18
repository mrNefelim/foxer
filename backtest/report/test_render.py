import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from pypdf import PdfReader
spec = importlib.util.spec_from_file_location('render', Path(__file__).with_name('render.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ReportTests(unittest.TestCase):
    def test_empty_report_contains_method_and_no_fabricated_trades(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            summary = dict(closed=0, net_r=0, max_closed_drawdown_r=0, win_rate=None,
                           mean_r=None, profit_factor=None, mean_duration_seconds=None,
                           open=0, pending=0, cancelled=0, gap_affected=0, curve=[])
            result = dict(schema_version=1, from_=0, to=86400000, symbols=['EURUSD'],
                          strategy_sha256='a'*64, summary=summary, by_symbol={'EURUSD':summary},
                          monthly={}, gap_affected_summary=summary, trades=[], examples=[],
                          sources={'EURUSD': {'files': [{'source': 'prepared source'}]}},
                          coverage={}, initial_context={}, engine_version='test',
                          execution={'intrabar': 'observed ticks', 'costs': 'not modeled',
                                     'gaps': 'observed ticks only; affected trades flagged'})
            result['from'] = result.pop('from_')
            (p/'results.json').write_text(json.dumps(result))
            module.render(p)
            first = (p/'report.pdf').read_bytes()
            module.render(p)
            self.assertEqual(first, (p/'report.pdf').read_bytes())
            pdf = PdfReader(p/'report.pdf')
            text = '\n'.join(page.extract_text() for page in pdf.pages)
            self.assertIn('Сделки не закрывались', text)
            self.assertIn('Издержки: не моделируются', text)
            self.assertIn('EURUSD', text)
            self.assertIn('наблюдаемые тики', text)
            self.assertNotIn('не распознан', text)
            self.assertGreaterEqual(len(pdf.pages), 2)

    def test_method_and_source_follow_saved_execution_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            summary = dict(closed=0, net_r=0, max_closed_drawdown_r=0, win_rate=None,
                           mean_r=None, profit_factor=None, mean_duration_seconds=None,
                           open=0, pending=0, cancelled=0, gap_affected=0, curve=[])
            result = dict(schema_version=1, **{'from': 0, 'to': 86400000}, symbols=['EURUSD'],
                          strategy_sha256='a'*64, summary=summary, by_symbol={'EURUSD':summary},
                          monthly={}, gap_affected_summary=summary, trades=[], examples=[],
                          sources={'EURUSD': {'files': [
                              {'source': 'prepared feed', 'intrabar': 'synthetic'},
                              {'source': 'prepared feed'}]}}, coverage={}, initial_context={},
                          engine_version='test', execution={'intrabar': 'synthetic: 4 index quotes per M1, worst-case extreme order for open signals',
                                                           'gaps': 'observed ticks only; affected trades flagged'})
            (p/'results.json').write_text(json.dumps(result))
            module.render(p)
            text = '\n'.join(page.extract_text() for page in PdfReader(p/'report.pdf').pages)
            self.assertIn('четыре синтетические индексные котировки на минуту', text)
            self.assertIn('prepared feed', text)
            self.assertNotIn('Результат по наблюдаемым котировкам', text)
            self.assertNotIn('synthetic: 4 index quotes per M1', text)
            self.assertNotIn('только наблюдаемые тики', text)

            del result['execution']
            (p/'results.json').write_text(json.dumps(result))
            module.render(p)
            text = '\n'.join(page.extract_text() for page in PdfReader(p/'report.pdf').pages)
            self.assertIn('Метод исполнения не указан', text)
            self.assertNotIn('наблюдаемым котировкам', text)


    def test_mixed_execution_lists_each_symbol_and_its_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)
            summary = dict(closed=0, net_r=0, max_closed_drawdown_r=0, win_rate=None,
                           mean_r=None, profit_factor=None, mean_duration_seconds=None,
                           open=0, pending=0, cancelled=0, gap_affected=0, curve=[])
            result = dict(schema_version=1, **{'from': 0, 'to': 86400000},
                          symbols=['EURUSD', 'GBPUSD'], strategy_sha256='a'*64,
                          summary=summary, by_symbol={'EURUSD':summary,'GBPUSD':summary},
                          monthly={}, gap_affected_summary=summary, trades=[], examples=[],
                          sources={
                              'EURUSD': {'files':[{'source':'tick fixture'}]},
                              'GBPUSD': {'files':[{'source':'prepared fixture'}]},
                          }, coverage={}, initial_context={}, engine_version='test',
                          execution={
                              'intrabar':'mixed',
                              'intrabar_by_symbol':{
                                  'EURUSD':'observed ticks',
                                  'GBPUSD':'synthetic',
                              },
                              'gaps':'observed ticks only; affected trades flagged',
                          })
            (p/'results.json').write_text(json.dumps(result))
            module.render(p)
            text='\n'.join(page.extract_text() for page in PdfReader(p/'report.pdf').pages)
            self.assertIn('EURUSD', text)
            self.assertIn('только наблюдаемые тики', text)
            self.assertIn('GBPUSD', text)
            self.assertIn('четыре синтетические индексные котировки на минуту', text)
            self.assertNotIn('не распознан', text)

if __name__ == '__main__': unittest.main()
