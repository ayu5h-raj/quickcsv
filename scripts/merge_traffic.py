#!/usr/bin/env python3
"""Merge a fresh 14-day GitHub traffic snapshot into the long-term history CSV.

GitHub only retains traffic data for a rolling 14-day window, so this script is
run daily by .github/workflows/traffic.yml and upserts each day's record into
traffic/traffic.csv, building a permanent history.

Usage: merge_traffic.py <views.json> <clones.json> <history.csv>
"""

import csv
import json
import sys
from pathlib import Path

FIELDS = ["date", "views", "unique_views", "clones", "unique_clones"]


def load_history(path: Path) -> dict[str, dict]:
    """Read the existing history CSV into a dict keyed by date."""
    rows: dict[str, dict] = {}
    if not path.exists():
        return rows
    with path.open(newline="") as f:
        for row in csv.DictReader(f):
            rows[row["date"]] = row
    return rows


def date_key(timestamp: str) -> str:
    """GitHub timestamps look like 2026-05-12T00:00:00Z; keep the date part."""
    return timestamp.split("T", 1)[0]


def apply_snapshot(rows: dict[str, dict], data: dict, kind: str) -> None:
    """Upsert daily counts from a views or clones API response.

    The latest snapshot is authoritative for any date it covers, so overwriting
    overlapping dates keeps the history accurate rather than double-counting.
    """
    for entry in data.get(kind, []):
        day = date_key(entry["timestamp"])
        record = rows.setdefault(day, {f: "" for f in FIELDS})
        record["date"] = day
        if kind == "views":
            record["views"] = entry["count"]
            record["unique_views"] = entry["uniques"]
        else:
            record["clones"] = entry["count"]
            record["unique_clones"] = entry["uniques"]


def main() -> None:
    views_path, clones_path, history_path = (Path(p) for p in sys.argv[1:4])

    rows = load_history(history_path)
    apply_snapshot(rows, json.loads(views_path.read_text()), "views")
    apply_snapshot(rows, json.loads(clones_path.read_text()), "clones")

    history_path.parent.mkdir(parents=True, exist_ok=True)
    with history_path.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        for day in sorted(rows):
            record = rows[day]
            writer.writerow({f: record.get(f, "") for f in FIELDS})

    print(f"Wrote {len(rows)} days of traffic history to {history_path}")


if __name__ == "__main__":
    main()
