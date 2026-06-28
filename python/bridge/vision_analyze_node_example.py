from __future__ import annotations

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import torch

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

from custom_nodes.hgripe_api_nodes import HGripeVisionAnalyze


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = json.loads(self.rfile.read(request_size).decode("utf-8"))
        content = request_body["messages"][-1]["content"]
        has_image = any(part.get("type") == "image_url" for part in content)
        detail = next(
            (
                part["image_url"].get("detail")
                for part in content
                if part.get("type") == "image_url"
            ),
            "missing",
        )
        payload = {
            "id": "local-vision-analyze-example",
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": f"vision saw image={has_image}, detail={detail}",
                    },
                    "finish_reason": "stop",
                }
            ],
        }
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-vision-analyze-node-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    image = torch.zeros((1, 3, 2, 3), dtype=torch.float32)
    image[:, :, :, 0] = 1.0

    node = HGripeVisionAnalyze()
    text, result_json, status = node.run(
        image=image,
        prompt="describe the image",
        profile_ref="",
        model="local-vision-model",
        base_url=f"http://127.0.0.1:{server.server_port}",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        system_prompt="",
        image_index=0,
        image_format="png",
        detail="low",
        temperature=0.2,
        max_tokens=256,
        advanced_json="{}",
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )
    result = json.loads(result_json)
    print(
        {
            "status": status,
            "text": text,
            "provider_request_id": result.get("provider_request_id"),
        }
    )
finally:
    server.shutdown()
