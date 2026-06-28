from __future__ import annotations

import json
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from hgripe_api_bridge import run_task


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        auth_header = self.headers.get("authorization", "")
        payload = {
            "id": "local-credential-ref-example",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": f"auth={auth_header}",
                    },
                    "finish_reason": "stop",
                }
            ],
        }
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-openai-compatible-credential-ref-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

credentials_path = Path(tempfile.gettempdir()) / "hgripe-credentials-ref-example.json"
credentials_path.write_text(
    json.dumps(
        {
            "local-openai": {
                "provider": "openai_compatible",
                "base_url": f"http://127.0.0.1:{server.server_port}",
                "api_key": "credential-ref-key",
            }
        },
        indent=2,
    ),
    encoding="utf-8",
)

try:
    task = {
        "id": "openai-compatible-credential-ref-example",
        "provider": "openai_compatible",
        "operation": "chat.completions",
        "inputs": {"prompt": "hello credential ref"},
        "params": {
            "credentials_file": str(credentials_path),
            "model": "local-test-model",
        },
        "credentials_ref": "local-openai",
        "output_type": "text",
        "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
        "retry_policy": {"max_attempts": 2, "backoff_ms": 500, "timeout_ms": 30000},
    }

    print(run_task(task))
finally:
    server.shutdown()
    credentials_path.unlink(missing_ok=True)
