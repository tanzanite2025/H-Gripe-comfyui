from __future__ import annotations

import base64
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import BytesIO
from pathlib import Path

import torch
from PIL import Image

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

from custom_nodes.hgripe_api_nodes import HGripeOpenAICompatibleImageEdit


def example_png_b64() -> str:
    buffer = BytesIO()
    Image.new("RGB", (2, 3), (64, 128, 255)).save(buffer, format="PNG")
    return base64.b64encode(buffer.getvalue()).decode("ascii")


class ExampleHandler(BaseHTTPRequestHandler):
    image_b64 = example_png_b64()

    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = self.rfile.read(request_size)
        payload = {
            "created": 123,
            "data": [
                {
                    "b64_json": self.image_b64,
                    "revised_prompt": (
                        "multipart edit received"
                        if b'name="image"' in request_body
                        else "missing image"
                    ),
                }
            ],
        }
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-openai-compatible-image-edit-node-example")
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

    node = HGripeOpenAICompatibleImageEdit()
    edited_image, result_json, status = node.run(
        image=image,
        base_url=f"http://127.0.0.1:{server.server_port}",
        profile_ref="",
        model="local-image-edit-model",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        prompt="hello image edit node",
        image_index=0,
        image_format="png",
        size="2x3",
        n=1,
        response_format="b64_json",
        quality="provider_default",
        output_format="provider_default",
        save_outputs="enable",
        download_url_outputs="enable",
        extra_body_json="{}",
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )
    result = json.loads(result_json)
    print(
        {
            "status": status,
            "image_shape": tuple(edited_image.shape),
            "provider_request_id": result.get("provider_request_id"),
            "revised_prompt": result["output_json"]["images"][0].get("revised_prompt"),
            "output_files": result.get("output_files", []),
        }
    )
finally:
    server.shutdown()
