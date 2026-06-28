from __future__ import annotations

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

from custom_nodes.hgripe_api_nodes import HGripeReplicateRun


IMAGE_BYTES = b"\x89PNG\r\n\x1a\nfake replicate output png"


class ExampleHandler(BaseHTTPRequestHandler):
    poll_count = 0

    def do_POST(self) -> None:
        if self.path != "/v1/models/stability-ai/sdxl/predictions":
            self.send_error(404)
            return

        request_size = int(self.headers.get("content-length", "0"))
        self.rfile.read(request_size)
        self._write_json(
            {
                "id": "pred-123",
                "status": "starting",
                "urls": {
                    "get": f"http://127.0.0.1:{self.server.server_port}/v1/predictions/pred-123"
                },
            },
            status_code=201,
            request_id="local-replicate-create",
        )

    def do_GET(self) -> None:
        if self.path == "/v1/predictions/pred-123":
            type(self).poll_count += 1
            if type(self).poll_count == 1:
                self._write_json(
                    {"id": "pred-123", "status": "processing"},
                    request_id="local-replicate-poll-running",
                )
            else:
                self._write_json(
                    {
                        "id": "pred-123",
                        "status": "succeeded",
                        "output": [
                            f"http://127.0.0.1:{self.server.server_port}/files/out-0.png"
                        ],
                    },
                    request_id="local-replicate-poll-complete",
                )
            return

        if self.path == "/files/out-0.png":
            self.send_response(200)
            self.send_header("content-type", "image/png")
            self.send_header("content-length", str(len(IMAGE_BYTES)))
            self.end_headers()
            self.wfile.write(IMAGE_BYTES)
            return

        self.send_error(404)

    def _write_json(
        self,
        payload: dict[str, object],
        request_id: str,
        status_code: int = 200,
    ) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status_code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", request_id)
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    base_url = f"http://127.0.0.1:{server.server_port}"
    node = HGripeReplicateRun()
    output_path, output_json, result_json, status = node.run(
        model="stability-ai/sdxl",
        version="",
        input_json='{"prompt":"a small friendly robot"}',
        base_url=base_url,
        profile_ref="",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        extra_body_json="{}",
        download_outputs="enable",
        output_extension="",
        max_polls=3,
        poll_interval_ms=100,
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )
    result = json.loads(result_json)
    print(
        {
            "status": status,
            "output_path": output_path,
            "output": json.loads(output_json),
            "provider_request_id": result.get("provider_request_id"),
            "poll_count": (result.get("output_json") or {})
            .get("polling", {})
            .get("poll_count"),
            "output_files": len(result.get("output_files") or []),
        }
    )
finally:
    server.shutdown()
