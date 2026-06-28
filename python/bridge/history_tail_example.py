from __future__ import annotations

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
        }
        for record in records
    ]


history_file = Path(os.environ.get("HGRIPE_HISTORY_FILE", "") or default_history_file())
history_db = Path(os.environ.get("HGRIPE_HISTORY_DB", "") or default_history_db())

if history_db.exists():
    with sqlite3.connect(history_db) as connection:
        rows = connection.execute(
            """
            SELECT record_json
            FROM task_history
            ORDER BY timestamp_ms DESC, rowid DESC
            LIMIT 5
            """
        ).fetchall()
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
    records = [json.loads(line) for line in lines[-5:]]
    print(
        {
            "history_file": str(history_file),
            "records": summarize_records(records),
        }
    )
