from __future__ import annotations

from hgripe_api_bridge import run_task


task = {
    "id": "example-task",
    "provider": "mock",
    "operation": "echo",
    "inputs": {"prompt": "hello from python"},
    "params": {},
    "credentials_ref": None,
    "output_type": "json",
    "cache_policy": {"enabled": True, "ttl_seconds": None, "key": None},
    "retry_policy": {"max_attempts": 3, "backoff_ms": 100, "timeout_ms": 120000},
}

print(run_task(task))

