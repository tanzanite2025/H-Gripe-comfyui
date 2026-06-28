from __future__ import annotations

import json
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from hgripe_api_bridge import run_task


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = json.loads(self.rfile.read(request_size).decode("utf-8"))
        prompt = request_body["messages"][-1]["content"]
        payload = {
            "id": "local-profile-chat-example",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": (
                            f"profile model={request_body['model']} "
                            f"temperature={request_body.get('temperature')} "
                            f"prompt={prompt}"
                        ),
                    },
                    "finish_reason": "stop",
                }
            ],
        }
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-openai-compatible-profile-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

profiles_path = Path(tempfile.gettempdir()) / "hgripe-provider-profile-example.json"
profiles_path.write_text(
    json.dumps(
        {
            "local-chat-profile": {
                "provider": "openai_compatible",
                "base_url": f"http://127.0.0.1:{server.server_port}",
                "model": "profile-test-model",
                "no_auth": True,
                "params": {
                    "temperature": 0.25,
                },
                "headers": {
                    "X-H-Gripe-Profile": "local-chat-profile",
                },
            }
        },
        indent=2,
    ),
    encoding="utf-8",
)

try:
    task = {
        "id": "openai-compatible-profile-example",
        "provider": "openai_compatible",
        "operation": "chat.completions",
        "inputs": {"prompt": "hello through provider profile"},
        "params": {
            "profile_ref": "local-chat-profile",
            "profiles_file": str(profiles_path),
        },
        "credentials_ref": None,
        "output_type": "text",
        "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
        "retry_policy": {"max_attempts": 2, "backoff_ms": 500, "timeout_ms": 30000},
    }

    print(run_task(task))
finally:
    server.shutdown()
    profiles_path.unlink(missing_ok=True)
