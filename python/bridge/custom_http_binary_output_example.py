from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from hgripe_api_bridge import run_task


PNG_BYTES = b"\x89PNG\r\n\x1a\ncustom-http-output-example"


class ExampleHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self.send_response(200)
        self.send_header("content-type", "image/png")
        self.send_header("content-length", str(len(PNG_BYTES)))
        self.send_header("x-request-id", "local-custom-http-binary-output-example")
        self.end_headers()
        self.wfile.write(PNG_BYTES)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    task = {
        "id": "custom-http-binary-output-example",
        "provider": "custom_http",
        "operation": "request",
        "inputs": {},
        "params": {
            "method": "GET",
            "url": f"http://127.0.0.1:{server.server_port}/image.png",
            "save_response": True,
            "output_extension": "png",
        },
        "credentials_ref": None,
        "output_type": "files",
        "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
        "retry_policy": {"max_attempts": 2, "backoff_ms": 500, "timeout_ms": 30000},
    }

    result = run_task(task)
    print(
        json.dumps(
            {
                "status": result.get("status"),
                "provider_request_id": result.get("provider_request_id"),
                "body_saved": (result.get("output_json") or {}).get("body_saved"),
                "output_files": result.get("output_files", []),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
finally:
    server.shutdown()
