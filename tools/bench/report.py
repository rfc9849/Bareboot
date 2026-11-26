#!/usr/bin/env python3
"""Compare bareboot-bench JSON results and print a Markdown table."""

import argparse
import json
import sys
from pathlib import Path


def fmt_val(v: float) -> str:
    if v >= 1000:
        return f"{v:,.0f}"
    if v >= 100:
        return f"{v:.1f}"
    return f"{v:.2f}"


def ratio(a: float, b: float, better: str) -> float:
    if a == 0 or b == 0:
        return 0.0
    return (a / b) if better == "higher" else (b / a)


def main() -> int:
    ap = argparse.ArgumentParser(description="Format bareboot-bench JSON files as Markdown.")
    ap.add_argument("files", nargs="+", type=Path, help="JSON files produced by bareboot-bench --json/--out")
    ap.add_argument("--ref", help="Label of the primary column when --vs is set (default: first file)")
    ap.add_argument("--vs", help="Optional comparison label to add a ratio column")
    ap.add_argument("--ranges", action="store_true", help="Include min-max ranges next to medians")
    args = ap.parse_args()

    runs = [json.loads(p.read_text(encoding="utf-8")) for p in args.files]
    labels = [r["label"] for r in runs]
    ref_label = args.ref or labels[0]
    vs_label = args.vs

    by_label = {r["label"]: r for r in runs}
    if ref_label not in by_label:
        sys.exit(f"unknown --ref label: {ref_label}")
    if vs_label and vs_label not in by_label:
        sys.exit(f"unknown --vs label: {vs_label}")

    cpus = {r.get("cpus_allowed", "unknown") for r in runs}
    if len(cpus) > 1:
        sys.stderr.write(f"warning: mismatched cpus_allowed across runs: {sorted(cpus)}\n")

    results_map = {
        r["label"]: {item["id"]: item for item in r["results"]}
        for r in runs
    }
    order = [item["id"] for item in runs[0]["results"]]

    headers = ["Benchmark", "Unit"] + labels
    align = ["---", "---"] + ["---:"] * len(labels)
    if vs_label and vs_label != ref_label:
        headers.append(f"{ref_label} / {vs_label}")
        align.append("---:")

    print("| " + " | ".join(headers) + " |")
    print("| " + " | ".join(align) + " |")

    for bid in order:
        meta = results_map[labels[0]][bid]
        better_tag = "higher" if meta["better"] == "higher" else "lower"
        row = [meta["name"], f"{meta['unit']} ({better_tag})"]
        for lbl in labels:
            item = results_map[lbl].get(bid)
            if not item:
                row.append("-")
                continue
            cell = fmt_val(item["median"])
            if args.ranges:
                cell += f" ({fmt_val(item['min'])}-{fmt_val(item['max'])})"
            row.append(cell)
        if vs_label and vs_label != ref_label:
            a = results_map[ref_label].get(bid)
            b = results_map[vs_label].get(bid)
            if a and b:
                r = ratio(a["median"], b["median"], meta["better"])
                row.append(f"{r:.2f}x")
            else:
                row.append("-")
        print("| " + " | ".join(row) + " |")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
