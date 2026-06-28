from __future__ import annotations

import json
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from hgripe_api_bridge import run_task


class ExampleHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        authorized = (
            self.headers.get("authorization") == "Bearer local-credential-token"
            and self.headers.get("x-credential-test") == "yes"
            and self.path == "/secure"
        )
        body = json.dumps({"ok": True, "authorized": authorized}).encode("utf-8")
        self.send_response(200 if authorized else 401)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-custom-http-credentials-ref-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

credentials_path = Path(tempfile.gettempdir()) / "hgripe-custom-http-credentials-example.json"
credentials_path.write_text(
    json.dumps(
        {
            "local-custom-http": {
                "provider": "custom_http",
                "base_url": f"http://127.0.0.1:{server.server_port}",
                "api_key": "local-credential-token",
                "headers": {
                    "X-Credential-Test": "yes",
                },
            }
        },
        indent=2,
    ),
    encoding="utf-8",
)

try:
    task = {
        "id": "custom-http-credentials-ref-example",
        "provider": "custom_http",
        "operation": "request",
        "inputs": {},
        "params": {
            "method": "GET",
            "url": "/secure",
            "credentials_file": str(credentials_path),
        },
        "credentials_ref": "local-custom-http",
        "output_type": "json",
        "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
        "retry_policy": {"max_attempts": 2, "backoff_ms": 500, "timeout_ms": 30000},
    }

    result = run_task(task)
    print(
        json.dumps(
            {
                "status": result.get("status"),
                "provider_request_id": result.get("provider_request_id"),
                "authorized": (result.get("output_json") or {})
                .get("body", {})
                .get("authorized"),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
finally:
    server.shutdown()
    credentials_path.unlink(missing_ok=True)
