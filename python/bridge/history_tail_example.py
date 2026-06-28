from __future__ import annotations

import argparse
import json
import os
import sqlite3
from pathlib import Path


def default_history_file() -> Path:
    root = Path(__file__).resolve().parents[2]
    return root / "user" / "hgripe" / "history" / "tasks.jsonl"


def default_history_db() -> Path:
    root = Path(__file__).resolve().parents[2]
    return root / "user" / "hgripe" / "history" / "tasks.sqlite3"


def summarize_records(records: list[dict]) -> list[dict]:
    return [
        {
            "task_id": record["task_id"],
            "provider": record["provider"],
            "operation": record["operation"],
            "status": record["status"],
            "duration_ms": record["duration_ms"],
            "provider_request_id": record.get("provider_request_id"),
            "output_file_count": record.get("output_file_count", 0),
            "rerunnable": bool(record.get("task_snapshot")),
        }
        for record in records
    ]


history_file = Path(os.environ.get("HGRIPE_HISTORY_FILE", "") or default_history_file())
history_db = Path(os.environ.get("HGRIPE_HISTORY_DB", "") or default_history_db())

parser = argparse.ArgumentParser()
parser.add_argument("--limit", type=int, default=5)
parser.add_argument("--provider", default="")
parser.add_argument("--operation", default="")
parser.add_argument("--status", default="")
parser.add_argument("--has-output-files", choices=["yes", "no"], default="")
args = parser.parse_args()
limit = max(1, min(args.limit, 500))

filters = []
params: list[object] = []
if args.provider.strip():
    filters.append("provider = ?")
    params.append(args.provider.strip())
if args.operation.strip():
    filters.append("operation = ?")
    params.append(args.operation.strip())
if args.status.strip():
    filters.append("status = ?")
    params.append(args.status.strip())
if args.has_output_files == "yes":
    filters.append("output_file_count > 0")
elif args.has_output_files == "no":
    filters.append("output_file_count = 0")

if history_db.exists():
    sql = "SELECT record_json FROM task_history"
    if filters:
        sql += " WHERE " + " AND ".join(filters)
    sql += " ORDER BY timestamp_ms DESC, rowid DESC LIMIT ?"
    params.append(limit)

    with sqlite3.connect(history_db) as connection:
        rows = connection.execute(sql, params).fetchall()
    records = [json.loads(row[0]) for row in rows]
    print(
        {
            "history_db": str(history_db),
            "records": summarize_records(records),
        }
    )
elif not history_file.exists():
    print({"history_file": str(history_file), "history_db": str(history_db), "records": []})
else:
    lines = [line for line in history_file.read_text(encoding="utf-8").splitlines() if line.strip()]
    records = [json.loads(line) for line in reversed(lines)]
    if args.provider.strip():
        records = [record for record in records if record["provider"] == args.provider.strip()]
    if args.operation.strip():
        records = [record for record in records if record["operation"] == args.operation.strip()]
    if args.status.strip():
        records = [record for record in records if record["status"] == args.status.strip()]
    if args.has_output_files == "yes":
        records = [record for record in records if record.get("output_file_count", 0) > 0]
    elif args.has_output_files == "no":
        records = [record for record in records if record.get("output_file_count", 0) == 0]
    records = records[:limit]
    print(
        {
            "history_file": str(history_file),
            "records": summarize_records(records),
        }
    )
