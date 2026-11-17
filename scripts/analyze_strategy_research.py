"""Offline backtest analysis. Costs are hypothetical round-trip pip deductions.

Usage: python3 scripts/analyze_strategy_research.py --baseline results.json
       --candidate candidate/results.json --output new-summary.json

Bootstrap uncertainty is conditional on the historical sample and execution model;
fraction_positive is a resampling frequency, not a p-value or live-profit forecast.
"""
import argparse
from collections import defaultdict
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import random


def _utc(millis):
    return datetime.fromtimestamp(millis / 1000, timezone.utc)


def _months(start, end):
    if start >= end:
        raise ValueError("from must precede to")
    first, last = _utc(start), _utc(end - 1)
    year, month = first.year, first.month
    result = {}
    while (year, month) <= (last.year, last.month):
        result[f"{year:04}-{month:02}"] = 0.0
        year, month = (year + 1, 1) if month == 12 else (year, month + 1)
    return result


def _number(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError("expected finite number")
    return value


def _rule(trade):
    plan = (trade.get("basis") or {}).get("plan") or {}
    if plan.get("rule"):
        return plan["rule"]
    parts = trade["id"].split(":")
    if parts[0] == trade["symbol"]:
        parts = parts[1:]
    return parts[0]


def _stats(rows):
    ordered = sorted(rows, key=lambda t: (t["exit_at"], t["symbol"], t["id"]))
    net = peak = drawdown = gain = loss = 0.0
    positives = []
    for row in ordered:
        value = row["adjusted_r"]
        net += value
        peak = max(peak, net)
        drawdown = max(drawdown, peak - net)
        if value > 0:
            gain += value
            positives.append(value)
        else:
            loss -= value
    top5 = sum(sorted(positives, reverse=True)[:5])
    count = len(ordered)
    return {"closed": count, "wins": len(positives), "net_r": net,
            "mean_r": net / count if count else None,
            "win_rate": len(positives) / count if count else None,
            "gross_profit_r": gain, "gross_loss_r": loss,
            "profit_factor": gain / loss if loss else None,
            "max_closed_drawdown_r": drawdown,
            "top5_wins_r": top5,
            "top5_wins_share_of_gross_profit": top5 / gain if gain else None,
            "net_r_without_top5_wins": net - top5}


def _scenario(rows, start, end, pips):
    adjusted = [{**t, "adjusted_r": t["result_r"] - pips * t["one_pip_cost_r"]}
                for t in rows]
    result = _stats(adjusted)
    result["monthly"] = _months(start, end)
    for t in adjusted:
        result["monthly"][_utc(t["exit_at"]).strftime("%Y-%m")] += t["adjusted_r"]
    for name, key in [("year", lambda t: str(_utc(t["exit_at"]).year)),
                      ("symbol", lambda t: t["symbol"]),
                      ("rule", _rule), ("side", lambda t: t["side"])]:
        groups = defaultdict(list)
        for t in adjusted:
            groups[key(t)].append(t)
        result["by_" + name] = {k: _stats(v) for k, v in sorted(groups.items())}
    return result


def summarize(result):
    start, end = _number(result["from"]), _number(result["to"])
    _months(start, end)
    rows = []
    for trade in result["trades"]:
        if trade.get("result_r") is None:
            continue
        _number(trade["result_r"])
        exit_at = _number(trade["exit_at"])
        if not start <= exit_at < end:
            raise ValueError("closed trade outside result period")
        risk = abs(_number(trade["entry"]) - _number(trade["stop"]))
        if risk == 0:
            raise ValueError("zero entry-stop risk")
        pip = {"EURUSD": 100, "GBPUSD": 100, "USDJPY": 10000}.get(trade["symbol"])
        if pip is None:
            raise ValueError("unknown pip scale; define explicitly")
        rows.append({**trade, "one_pip_cost_r": pip / risk})
    def scenarios(selected):
        return {name: _scenario(selected, start, end, pips)
                for name, pips in [("gross", 0), ("stress_1pip", 1), ("stress_2pip", 2)]}
    report = scenarios(rows)
    report.update(from_ms=start, to_ms=end, gap_affected_closed=sum(bool(t.get("gaps")) for t in rows))
    report["no_gap"] = scenarios([t for t in rows if not t.get("gaps")])
    return report


def paired_bootstrap(baseline_monthly, candidate_monthly, block_months=3,
                     replications=2000, seed=20261116):
    keys = sorted(baseline_monthly)
    if keys != sorted(candidate_monthly) or not keys:
        raise ValueError("paired bootstrap requires matching nonempty months")
    start = datetime.fromisoformat(keys[0] + "-01").replace(tzinfo=timezone.utc)
    year, month = map(int, keys[-1].split("-"))
    year, month = (year + 1, 1) if month == 12 else (year, month + 1)
    end = datetime(year, month, 1, tzinfo=timezone.utc)
    if keys != list(_months(start.timestamp() * 1000, end.timestamp() * 1000)):
        raise ValueError("monthly series must be contiguous and zero-filled")
    if block_months < 1 or replications < 1:
        raise ValueError("positive block length and replications required")
    differences = [_number(candidate_monthly[k]) - _number(baseline_monthly[k]) for k in keys]
    n = len(differences)
    block = min(block_months, n)
    rng = random.Random(seed)
    draws = []
    for _ in range(replications):
        sample = []
        while len(sample) < n:
            offset = rng.randrange(n - block + 1)
            sample.extend(differences[offset:offset + block])
        draws.append(sum(sample[:n]))
    draws.sort()
    def quantile(p):
        index = (len(draws) - 1) * p
        low = int(index)
        high = min(low + 1, len(draws) - 1)
        return draws[low] + (draws[high] - draws[low]) * (index - low)
    return {"method": "paired overlapping moving-block percentile bootstrap",
            "months": n, "block_months": block, "replications": replications, "seed": seed,
            "delta_net_r": sum(differences), "ci95_delta_net_r": [quantile(.025), quantile(.975)],
            "fraction_positive": sum(v > 0 for v in draws) / replications,
            "interpretation": "conditional historical uncertainty; fraction_positive is not a p-value"}


def compare(baseline, candidate):
    if (baseline["from"], baseline["to"]) != (candidate["from"], candidate["to"]):
        raise ValueError("comparison requires identical from/to periods")
    left, right = summarize(baseline), summarize(candidate)
    names = ("gross", "stress_1pip", "stress_2pip")
    bootstrap = {name: paired_bootstrap(left[name]["monthly"], right[name]["monthly"]) for name in names}
    bootstrap["no_gap"] = {name: paired_bootstrap(left["no_gap"][name]["monthly"], right["no_gap"][name]["monthly"]) for name in names}
    return {"baseline": left, "candidate": right, "paired_bootstrap": bootstrap,
            "notes": ["Closed-trade R and drawdown only; unrealized risk excluded.",
                      "Stress costs subtract a hypothetical total round-trip 1 or 2 pips per closed trade.",
                      "No-gap subsets may differ between strategies.",
                      "Bootstrap does not correct strategy selection or repeated use of historical data."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    baseline = json.loads(args.baseline.read_text())
    candidate = json.loads(args.candidate.read_text())
    result = compare(baseline, candidate)
    result["input_files"] = {"baseline": str(args.baseline), "candidate": str(args.candidate)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:
        json.dump(result, output, ensure_ascii=False, indent=2, allow_nan=False)
        output.write("\n")


if __name__ == "__main__":
    main()
