from __future__ import annotations

import argparse
import json
import os
import sqlite3
import uuid
from pathlib import Path
from typing import Any

from hgripe_api_bridge import run_task


def default_history_file() -> Path:
    root = Path(__file__).resolve().parents[2]
    return root / "user" / "hgripe" / "history" / "tasks.jsonl"


def default_history_db() -> Path:
    root = Path(__file__).resolve().parents[2]
    return root / "user" / "hgripe" / "history" / "tasks.sqlite3"


def load_record_from_sqlite(path: Path, task_id: str) -> dict[str, Any] | None:
    if not path.exists():
        return None

    with sqlite3.connect(path) as connection:
        row = connection.execute(
            "SELECT record_json FROM task_history WHERE task_id = ?",
            (task_id,),
        ).fetchone()

    if row is None:
        return None
    return json.loads(row[0])


def load_record_from_jsonl(path: Path, task_id: str) -> dict[str, Any] | None:
    if not path.exists():
        return None

    lines = [line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    for line in reversed(lines):
        record = json.loads(line)
        if record.get("task_id") == task_id:
            return record
    return None


def load_record(task_id: str, history_db: Path, history_file: Path) -> dict[str, Any] | None:
    return load_record_from_sqlite(history_db, task_id) or load_record_from_jsonl(
        history_file,
        task_id,
    )


def prepare_rerun_task(
    record: dict[str, Any],
    new_id: str | None,
    keep_cache: bool,
) -> dict[str, Any]:
    snapshot = record.get("task_snapshot")
    if not isinstance(snapshot, dict):
        raise ValueError(
            "This history record has no task_snapshot. "
            "Run a new task after rebuilding the broker, then rerun that newer task id."
        )

    task = json.loads(json.dumps(snapshot))
    old_id = str(task.get("id") or record.get("task_id") or "task")
    task["id"] = new_id or f"{old_id}-rerun-{uuid.uuid4().hex[:8]}"

    if not keep_cache:
        cache_policy = task.setdefault("cache_policy", {})
        cache_policy["enabled"] = False

    return task


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("task_id")
    parser.add_argument("--history-db", default=os.environ.get("HGRIPE_HISTORY_DB", ""))
    parser.add_argument("--history-file", default=os.environ.get("HGRIPE_HISTORY_FILE", ""))
    parser.add_argument("--broker", default="")
    parser.add_argument("--new-id", default="")
    parser.add_argument("--keep-cache", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    history_db = Path(args.history_db) if args.history_db else default_history_db()
    history_file = Path(args.history_file) if args.history_file else default_history_file()
    record = load_record(args.task_id, history_db, history_file)
    if record is None:
        raise SystemExit(f"History record not found for task_id={args.task_id!r}.")

    task = prepare_rerun_task(record, args.new_id.strip() or None, args.keep_cache)

    if args.dry_run:
        print(json.dumps({"rerun_task": task}, ensure_ascii=False, indent=2))
        return

    result = run_task(task, broker_path=args.broker or None)
    print(
        json.dumps(
            {
                "source_task_id": args.task_id,
                "rerun_task_id": task["id"],
                "result": result,
            },
            ensure_ascii=False,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
