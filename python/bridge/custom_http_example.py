from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from hgripe_api_bridge import run_task


class ExampleHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        payload = {"message": "hello from local http", "path": self.path}
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    task = {
        "id": "custom-http-example",
        "provider": "custom_http",
        "operation": "request",
        "inputs": {},
        "params": {
            "method": "GET",
            "url": f"http://127.0.0.1:{server.server_port}/demo",
        },
        "credentials_ref": None,
        "output_type": "json",
        "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
        "retry_policy": {"max_attempts": 2, "backoff_ms": 500, "timeout_ms": 30000},
    }

    print(run_task(task))
finally:
    server.shutdown()
