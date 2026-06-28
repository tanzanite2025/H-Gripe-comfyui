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

from custom_nodes.hgripe_api_nodes import HGripeImageGenerate


def example_png_b64() -> str:
    buffer = BytesIO()
    Image.new("RGB", (2, 3), (255, 32, 64)).save(buffer, format="PNG")
    return base64.b64encode(buffer.getvalue()).decode("ascii")


class ExampleHandler(BaseHTTPRequestHandler):
    image_b64 = example_png_b64()

    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = self.rfile.read(request_size)
        # /images/edits is multipart and includes the reference image part.
        is_edit = b'name="image"' in request_body
        payload = {
            "created": 123,
            "data": [
                {
                    "b64_json": self.image_b64,
                    "revised_prompt": "edit" if is_edit else "generate",
                }
            ],
        }
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-image-generate-node-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


def summarize(label: str, image, result_json: str, status: str) -> dict[str, object]:
    result = json.loads(result_json)
    return {
        "label": label,
        "status": status,
        "image_shape": tuple(image.shape),
        "provider_request_id": result.get("provider_request_id"),
        "revised_prompt": result["output_json"]["images"][0].get("revised_prompt"),
        "output_files": result.get("output_files", []),
    }


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    node = HGripeImageGenerate()
    base_url = f"http://127.0.0.1:{server.server_port}"
    common = dict(
        profile_ref="",
        model="local-image-model",
        base_url=base_url,
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        size="2x3",
        n=1,
        response_format="b64_json",
        quality="provider_default",
        style="provider_default",
        output_format="provider_default",
        reference_image_format="png",
        reference_image_index=0,
        save_outputs="enable",
        download_url_outputs="enable",
        advanced_json="{}",
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )

    # text-to-image (operation auto-resolves to image.generate)
    txt_image, txt_json, txt_status = node.run(
        prompt="hello text to image",
        operation="auto",
        **common,
    )
    print(summarize("text2img", txt_image, txt_json, txt_status))

    # image-to-image (a reference image auto-resolves to image.edit)
    reference = torch.zeros((1, 3, 2, 3), dtype=torch.float32)
    reference[:, :, :, 0] = 1.0
    img_image, img_json, img_status = node.run(
        prompt="hello image to image",
        operation="auto",
        reference_image=reference,
        **common,
    )
    print(summarize("img2img", img_image, img_json, img_status))
finally:
    server.shutdown()
