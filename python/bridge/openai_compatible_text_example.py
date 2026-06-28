from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from hgripe_api_bridge import run_task


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = json.loads(self.rfile.read(request_size).decode("utf-8"))
        prompt = request_body["messages"][-1]["content"]
        payload = {
            "id": "local-chat-example",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": f"local reply for: {prompt}",
                    },
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 4, "completion_tokens": 4, "total_tokens": 8},
        }
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-openai-compatible-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    task = {
        "id": "openai-compatible-text-example",
        "provider": "openai_compatible",
        "operation": "chat.completions",
        "inputs": {"prompt": "hello from python"},
        "params": {
            "base_url": f"http://127.0.0.1:{server.server_port}",
            "no_auth": True,
            "model": "local-test-model",
            "temperature": 0.2,
        },
        "credentials_ref": None,
        "output_type": "text",
        "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
        "retry_policy": {"max_attempts": 2, "backoff_ms": 500, "timeout_ms": 30000},
    }

    print(run_task(task))
finally:
    server.shutdown()
