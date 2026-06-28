from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

VIDEO_BYTES = b"FAKE-MP4-BYTES"


class ExampleHandler(BaseHTTPRequestHandler):
    def _json(self, payload: dict[str, object]) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-video-job-node-example")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        self.rfile.read(request_size)
        # Submit returns a job id; status starts queued.
        self._json({"id": "vid-1", "status": "queued"})

    def do_GET(self) -> None:
        port = self.server.server_address[1]
        if self.path.startswith("/jobs/"):
            self._json(
                {
                    "status": "succeeded",
                    "result": {"url": f"http://127.0.0.1:{port}/files/out.mp4"},
                }
            )
            return
        # Final video download.
        self.send_response(200)
        self.send_header("content-type", "video/mp4")
        self.send_header("content-length", str(len(VIDEO_BYTES)))
        self.end_headers()
        self.wfile.write(VIDEO_BYTES)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
port = server.server_address[1]
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

work_dir = Path(tempfile.mkdtemp(prefix="hgripe-video-job-"))
profiles_file = work_dir / "provider_profiles.json"
profiles_file.write_text(
    json.dumps(
        {
            "stub-video": {
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

# Import after env is set so the node/broker pick up the stub profile and paths.
from custom_nodes.hgripe_api_nodes import HGripeVideoJob

try:
    node = HGripeVideoJob()
    output_path, result_json, status = node.run(
        prompt="a calm ocean at sunset",
        profile_ref="stub-video",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        prompt_key="prompt",
        body_json='{"duration": 4}',
        image_key="image",
        image_format="png",
        image_index=0,
        output_extension="mp4",
        download_result="enable",
        max_polls=5,
        poll_interval_ms=0,
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )
    result = json.loads(result_json)
    saved = Path(output_path).read_bytes() if output_path else b""
    print(
        {
            "status": status,
            "output_path": output_path,
            "downloaded_bytes": len(saved),
            "matches_stub": saved == VIDEO_BYTES,
            "provider_request_id": result.get("provider_request_id"),
        }
    )
finally:
    server.shutdown()
