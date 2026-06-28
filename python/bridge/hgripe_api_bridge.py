from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any, Mapping


def default_broker_path() -> Path:
    root = Path(__file__).resolve().parents[2]
    exe_name = "hgripe-api-broker.exe" if os.name == "nt" else "hgripe-api-broker"
    return root / "target" / "debug" / exe_name


def run_task(task: Mapping[str, Any], broker_path: str | os.PathLike[str] | None = None) -> dict[str, Any]:
    broker = Path(
        broker_path
        or os.environ.get("HGRIPE_API_BROKER", "")
        or default_broker_path()
    )
    if not broker.exists():
        raise FileNotFoundError(
            f"H-Gripe API broker not found: {broker}. "
            "Build it with `cargo build -p hgripe-api --bin hgripe-api-broker`."
        )

    proc = subprocess.run(
        [str(broker)],
        input=json.dumps(task),
        text=True,
        capture_output=True,
        check=False,
    )
    if proc.returncode != 0 and not proc.stdout:
        raise RuntimeError(proc.stderr.strip() or f"broker exited with {proc.returncode}")
    return json.loads(proc.stdout)

