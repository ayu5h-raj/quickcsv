#!/usr/bin/env python3
"""Snapshot cumulative release-download counts into a daily history CSV.

Unlike the traffic API (which exposes daily counts), release download totals are
cumulative and have no per-day breakdown. So we record one row per UTC day with
the running totals, building a growth curve in traffic/downloads.csv.

Usage: snapshot_downloads.py <releases.json> <history.csv>
"""

import csv
import datetime
import json
import sys
from pathlib import Path

FIELDS = ["date", "total_downloads", "latest_tag", "latest_tag_downloads"]


def load_history(path: Path) -> dict[str, dict]:
    rows: dict[str, dict] = {}
    if not path.exists():
        return rows
    with path.open(newline="") as f:
        for row in csv.DictReader(f):
            rows[row["date"]] = row
    return rows


def asset_downloads(release: dict) -> int:
    return sum(asset["download_count"] for asset in release.get("assets", []))


def main() -> None:
    releases_path, history_path = (Path(p) for p in sys.argv[1:3])
    releases = json.loads(releases_path.read_text())

    total = sum(asset_downloads(r) for r in releases)
    # The API returns releases newest-first; the first non-draft is "latest".
    latest = next((r for r in releases if not r.get("draft")), None)

    today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
    rows = load_history(history_path)
    rows[today] = {
        "date": today,
        "total_downloads": total,
        "latest_tag": latest["tag_name"] if latest else "",
        "latest_tag_downloads": asset_downloads(latest) if latest else 0,
    }

    history_path.parent.mkdir(parents=True, exist_ok=True)
    with history_path.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS)
        writer.writeheader()
        for day in sorted(rows):
            writer.writerow({field: rows[day].get(field, "") for field in FIELDS})

    print(f"Recorded {total} total downloads for {today} in {history_path}")


if __name__ == "__main__":
    main()
