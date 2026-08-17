"""Benchmark substring, lexical, semantic, and hybrid retrieval against a running daemon.

Usage:
  python evaluation/benchmark.py --seed
  python evaluation/benchmark.py --api http://127.0.0.1:7777 --top-k 5

The first semantic run downloads the configured FastEmbed model, so run once to warm the cache
before recording latency numbers.
"""

from __future__ import annotations

import argparse
import json
import statistics
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

DATASET = Path(__file__).with_name("retrieval-v1.json")


def request(api: str, method: str, path: str, payload: dict | None = None) -> object:
    body = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(
        f"{api.rstrip('/')}{path}", body, method=method,
        headers={"Content-Type": "application/json"} if body else {},
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return json.load(response)
    except urllib.error.URLError as exc:
        raise SystemExit(f"daemon request failed: {exc}") from exc


def seed(api: str, learnings: list[dict]) -> None:
    for learning in learnings:
        request(api, "POST", "/learnings", {**learning, "author": "retrieval-eval-v1"})


def run_query(api: str, query: str, mode: str, top_k: int) -> tuple[list[str], float]:
    started = time.perf_counter()
    if mode == "substring":
        path = "/learnings?" + urllib.parse.urlencode({"query": query})
        rows = request(api, "GET", path)
        titles = [row["title"] for row in rows[:top_k]]
    else:
        path = "/retrieve?" + urllib.parse.urlencode(
            {"query": query, "mode": mode, "top_k": top_k, "min_score": 0.3}
        )
        response = request(api, "GET", path)
        titles = [row["title"] for row in response["results"]]
    return titles, (time.perf_counter() - started) * 1000


def evaluate(api: str, cases: list[dict], mode: str, top_k: int) -> dict:
    recalls, reciprocal_ranks, latencies, no_result = [], [], [], []
    for case in cases:
        actual, latency = run_query(api, case["query"], mode, top_k)
        expected = set(case["relevant_titles"])
        latencies.append(latency)
        if not expected:
            no_result.append(not actual)
            continue
        recalls.append(len(expected.intersection(actual)) / len(expected))
        ranks = [i + 1 for i, title in enumerate(actual) if title in expected]
        reciprocal_ranks.append(1 / min(ranks) if ranks else 0.0)
    return {
        "mode": mode,
        f"recall@{top_k}": round(statistics.mean(recalls), 4),
        "mrr": round(statistics.mean(reciprocal_ranks), 4),
        "no_result_accuracy": round(statistics.mean(no_result), 4),
        "latency_ms_median": round(statistics.median(latencies), 2),
        "latency_ms_p95": round(sorted(latencies)[max(0, int(len(latencies) * .95) - 1)], 2),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api", default="http://127.0.0.1:7777")
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument("--seed", action="store_true", help="POST the versioned fixture learnings first")
    args = parser.parse_args()
    dataset = json.loads(DATASET.read_text(encoding="utf-8"))
    if args.seed:
        seed(args.api, dataset["learnings"])
    print(json.dumps({
        "dataset_version": dataset["version"],
        "queries": len(dataset["queries"]),
        "results": [evaluate(args.api, dataset["queries"], mode, args.top_k)
                    for mode in ("substring", "lexical", "semantic", "hybrid")],
    }, indent=2))


if __name__ == "__main__":
    main()
