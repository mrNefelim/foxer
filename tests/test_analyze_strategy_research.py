import importlib.util
from pathlib import Path
import unittest
from datetime import datetime, timezone

SPEC = importlib.util.spec_from_file_location("analysis", Path(__file__).parents[1] / "scripts/analyze_strategy_research.py")
analysis = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analysis)


def ms(date):
    return int(datetime.fromisoformat(date).replace(tzinfo=timezone.utc).timestamp() * 1000)


def trade(date, value, symbol="EURUSD", rule="W_point_A", gaps=None):
    return {"id": f"{symbol}:{rule}:1", "symbol": symbol, "side": "Long",
            "state": "TakeProfit" if value > 0 else "Stop", "entry": 1100000,
            "stop": 1099000, "exit_at": ms(date), "result_r": value, "gaps": gaps or []}


class ResearchAnalysisTests(unittest.TestCase):
    def fixture(self):
        return {"from": ms("2023-12-01"), "to": ms("2024-04-01"),
                "symbols": ["EURUSD"],
                "sources": {"EURUSD": {"files": [
                    {"source": "verified MT5 export", "sha256": "a" * 64}
                ]}},
                "execution": {"intrabar": "observed ticks"},
                "trades": [trade("2024-03-01", 3), trade("2023-12-02", 2),
                           trade("2024-01-03", -1, gaps=[[1, 2]]),
                           {"state": "Cancelled", "result_r": None}]}

    def test_arithmetic_costs_chronology_and_empty_months(self):
        result = analysis.summarize(self.fixture())
        gross = result["gross"]
        self.assertEqual(gross["closed"], 3)
        self.assertEqual(gross["net_r"], 4)
        self.assertEqual(gross["max_closed_drawdown_r"], 1)
        self.assertEqual(gross["profit_factor"], 5)
        self.assertEqual(gross["monthly"], {"2023-12": 2, "2024-01": -1, "2024-02": 0, "2024-03": 3})
        self.assertAlmostEqual(result["stress_1pip"]["net_r"], 3.7)
        self.assertAlmostEqual(result["stress_2pip"]["net_r"], 3.4)
        self.assertEqual(result["no_gap"]["gross"]["net_r"], 5)
        self.assertEqual(gross["top5_wins_share_of_gross_profit"], 1)
        self.assertEqual(gross["net_r_without_top5_wins"], -1)

    def test_groups_and_jpy_pip_units(self):
        source = self.fixture()
        t = trade("2024-02-01", 1, "USDJPY", "D_correction_start")
        t.update(entry=150000000, stop=149900000, side="Short")
        source["trades"].append(t)
        result = analysis.summarize(source)
        self.assertEqual(result["gross"]["by_year"]["2024"]["net_r"], 3)
        self.assertEqual(result["gross"]["by_rule"]["W_point_A"]["closed"], 3)
        self.assertEqual(result["gross"]["by_side"]["Short"]["closed"], 1)
        self.assertAlmostEqual(result["stress_1pip"]["by_symbol"]["USDJPY"]["net_r"], .9)

    def test_bootstrap_paired_deterministic_and_constant_difference(self):
        baseline = {f"2024-{i:02}": i for i in range(1, 13)}
        candidate = {key: value + 1 for key, value in baseline.items()}
        a = analysis.paired_bootstrap(baseline, candidate)
        self.assertEqual(a, analysis.paired_bootstrap(baseline, candidate))
        self.assertEqual(a["delta_net_r"], 12)
        self.assertEqual(a["ci95_delta_net_r"], [12, 12])
        self.assertEqual(a["fraction_positive"], 1)
        self.assertEqual(a["replications"], 2000)
        self.assertEqual(analysis.paired_bootstrap(baseline, baseline)["fraction_positive"], 0)
        varied = {key: 2 * value for key, value in baseline.items()}
        first = analysis.paired_bootstrap(baseline, varied)
        self.assertEqual(first, analysis.paired_bootstrap(baseline, varied))
        self.assertEqual(first["delta_net_r"], 78)
        self.assertLess(first["ci95_delta_net_r"][0], 78)
        self.assertGreater(first["ci95_delta_net_r"][1], 78)
        self.assertEqual(first["fraction_positive"], 1)
        with self.assertRaises(ValueError):
            analysis.paired_bootstrap({"2024-01": 0, "2024-03": 0}, {"2024-01": 0, "2024-03": 0})

    def test_comparison_requires_equal_periods_and_rejects_bad_risk(self):
        source = self.fixture()
        other = {**source, "to": ms("2024-05-01")}
        with self.assertRaises(ValueError):
            analysis.compare(source, other)
        source["trades"][0]["stop"] = source["trades"][0]["entry"]
        with self.assertRaises(ValueError):
            analysis.summarize(source)

    def test_comparison_requires_matching_symbols_origin_hashes_and_execution(self):
        base = self.fixture()
        candidate = self.fixture()
        candidate["symbols"] = ["GBPUSD"]
        with self.assertRaisesRegex(ValueError, "symbol"):
            analysis.compare(base, candidate)
        candidate = self.fixture()
        candidate["sources"]["EURUSD"]["files"][0]["source"] = "different feed"
        with self.assertRaisesRegex(ValueError, "provenance"):
            analysis.compare(base, candidate)
        candidate = self.fixture()
        candidate["sources"]["EURUSD"]["files"][0]["sha256"] = "b" * 64
        with self.assertRaisesRegex(ValueError, "provenance"):
            analysis.compare(base, candidate)
        malformed = self.fixture()
        candidate = self.fixture()
        malformed["sources"]["EURUSD"]["files"][0]["sha256"] = "z" * 64
        candidate["sources"]["EURUSD"]["files"][0]["sha256"] = "z" * 64
        with self.assertRaisesRegex(ValueError, "hash"):
            analysis.compare(malformed, candidate)
        candidate = self.fixture()
        candidate["execution"]["intrabar"] = "synthetic OHLC"
        with self.assertRaisesRegex(ValueError, "execution"):
            analysis.compare(base, candidate)

    def test_comparison_normalizes_legacy_clean_execution_to_symbol_map(self):
        baseline = self.fixture()
        candidate = self.fixture()
        candidate["execution"] = {
            "intrabar": "observed ticks",
            "intrabar_by_symbol": {"EURUSD": "observed ticks"},
        }
        self.assertIn("paired_bootstrap", analysis.compare(baseline, candidate))

    def test_comparison_rejects_mixed_or_conflicting_symbol_modes(self):
        malformed = [
            {"intrabar": "mixed"},
            {"intrabar": "mixed", "intrabar_by_symbol": None},
            {"intrabar": "mixed", "intrabar_by_symbol": {}},
            {"intrabar": "mixed", "intrabar_by_symbol": {"EURUSD": "observed ticks"}},
            {"intrabar": "observed ticks", "intrabar_by_symbol": {"EURUSD": "synthetic"}},
        ]
        for execution in malformed:
            baseline = self.fixture()
            candidate = self.fixture()
            baseline["execution"] = execution
            candidate["execution"] = execution
            with self.subTest(execution=execution), self.assertRaises(ValueError):
                analysis.compare(baseline, candidate)

    def test_comparison_rejects_a_real_per_symbol_execution_difference(self):
        baseline = self.fixture()
        candidate = self.fixture()
        candidate["execution"] = {
            "intrabar": "synthetic: 4 index quotes per M1, worst-case extreme order for open signals",
            "intrabar_by_symbol": {"EURUSD": "synthetic"},
        }
        with self.assertRaises(ValueError):
            analysis.compare(baseline, candidate)

    def test_comparison_rejects_missing_provenance_or_execution_metadata(self):
        base = self.fixture()
        candidate = self.fixture()
        del candidate["sources"]
        with self.assertRaisesRegex(ValueError, "provenance"):
            analysis.compare(base, candidate)
        candidate = self.fixture()
        del candidate["execution"]
        with self.assertRaisesRegex(ValueError, "execution"):
            analysis.compare(base, candidate)

    def test_comparison_ignores_paths_and_record_order_when_hashes_match(self):
        base = self.fixture()
        candidate = self.fixture()
        base["sources"]["EURUSD"]["files"].append(
            {"prepared": "one/path.csv", "sha256": "b" * 64}
        )
        candidate["sources"]["EURUSD"]["files"].append(
            {"prepared": "different/path.csv", "sha256": "b" * 64}
        )
        candidate["sources"]["EURUSD"]["files"].reverse()
        self.assertIn("paired_bootstrap", analysis.compare(base, candidate))


if __name__ == "__main__":
    unittest.main()
