from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import BytesIO
from pathlib import Path

import torch
from PIL import Image

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))


def enhanced_png() -> bytes:
    # Pretend the backend upscaled the input to 8x6.
    buffer = BytesIO()
    Image.new("RGB", (8, 6), (16, 200, 64)).save(buffer, format="PNG")
    return buffer.getvalue()


PNG_BYTES = enhanced_png()


class ExampleHandler(BaseHTTPRequestHandler):
    def _json(self, payload: dict[str, object]) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-image-enhance-node-example")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        self.rfile.read(request_size)
        self._json({"id": "enh-1", "status": "queued"})

    def do_GET(self) -> None:
        port = self.server.server_address[1]
        if self.path.startswith("/jobs/"):
            self._json(
                {
                    "status": "succeeded",
                    "result": {"url": f"http://127.0.0.1:{port}/files/out.png"},
                }
            )
            return
        self.send_response(200)
        self.send_header("content-type", "image/png")
        self.send_header("content-length", str(len(PNG_BYTES)))
        self.end_headers()
        self.wfile.write(PNG_BYTES)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
port = server.server_address[1]
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

work_dir = Path(tempfile.mkdtemp(prefix="hgripe-image-enhance-"))
profiles_file = work_dir / "provider_profiles.json"
profiles_file.write_text(
    json.dumps(
        {
            "stub-enhance": {
                "provider": "custom_http",
                "path": f"http://127.0.0.1:{port}/submit",
                "no_auth": True,
                "params": {
                    "method": "POST",
                    "poll_url": f"http://127.0.0.1:{port}/jobs/{{job_id}}",
                    "poll_method": "GET",
                    "job_id_path": "id",
                    "status_path": "status",
                    "success_values": ["succeeded"],
                    "result_path": "result",
                    "download_url_path": "result.url",
                },
            }
        }
    ),
    encoding="utf-8",
)

os.environ["HGRIPE_PROVIDER_PROFILES_FILE"] = str(profiles_file)
os.environ["HGRIPE_OUTPUT_DIR"] = str(work_dir / "outputs")
os.environ["HGRIPE_HISTORY_DISABLED"] = "1"

from custom_nodes.hgripe_api_nodes import HGripeImageEnhance

try:
    source = torch.zeros((1, 3, 4, 3), dtype=torch.float32)
    source[:, :, :, 0] = 1.0

    node = HGripeImageEnhance()
    enhanced, output_path, result_json, status = node.run(
        image=source,
        profile_ref="stub-enhance",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        image_key="image",
        image_format="png",
        image_index=0,
        body_json='{"scale": 2}',
        output_extension="png",
        max_polls=5,
        poll_interval_ms=0,
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )
    result = json.loads(result_json)
    print(
        {
            "status": status,
            "input_shape": tuple(source.shape),
            "enhanced_shape": tuple(enhanced.shape),
            "output_path": output_path,
            "provider_request_id": result.get("provider_request_id"),
        }
    )
finally:
    server.shutdown()
