from __future__ import annotations

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

from custom_nodes.hgripe_api_nodes import HGripeCustomHttpAsyncJob


VIDEO_BYTES = b"fake mp4 bytes from custom http async job example"


class ExampleHandler(BaseHTTPRequestHandler):
    poll_count = 0

    def do_POST(self) -> None:
        if self.path != "/submit":
            self.send_error(404)
            return

        request_size = int(self.headers.get("content-length", "0"))
        self.rfile.read(request_size)
        self._write_json({"id": "job-123"}, request_id="local-custom-http-async-submit")

    def do_GET(self) -> None:
        if self.path == "/jobs/job-123":
            type(self).poll_count += 1
            if type(self).poll_count == 1:
                self._write_json(
                    {"status": "running"},
                    request_id="local-custom-http-async-poll-running",
                )
            else:
                self._write_json(
                    {
                        "status": "succeeded",
                        "result": {
                            "video_url": f"http://127.0.0.1:{self.server.server_port}/video.mp4"
                        },
                    },
                    request_id="local-custom-http-async-poll-complete",
                )
            return

        if self.path == "/video.mp4":
            self.send_response(200)
            self.send_header("content-type", "video/mp4")
            self.send_header("content-length", str(len(VIDEO_BYTES)))
            self.send_header("x-request-id", "local-custom-http-async-download")
            self.end_headers()
            self.wfile.write(VIDEO_BYTES)
            return

        self.send_error(404)

    def _write_json(self, payload: dict[str, object], request_id: str) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
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
    node = HGripeCustomHttpAsyncJob()
    output_path, result_json, status = node.run(
        url=f"{base_url}/submit",
        method="POST",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        headers_json="{}",
        query_json="{}",
        body_json='{"prompt":"make a short video"}',
        multipart_fields_json="{}",
        multipart_file_path="",
        multipart_file_field="file",
        multipart_file_name="",
        multipart_file_mime_type="",
        poll_url=f"{base_url}/jobs/{{job_id}}",
        poll_method="GET",
        poll_headers_json="{}",
        poll_query_json="{}",
        poll_body_json="",
        job_id_path="id",
        status_path="status",
        success_values_json='["succeeded"]',
        failure_values_json='["failed"]',
        result_path="result",
        download_result="enable",
        download_url_path="result.video_url",
        save_response="disable",
        output_extension="mp4",
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
            "provider_request_id": result.get("provider_request_id"),
            "poll_count": (result.get("output_json") or {})
            .get("polling", {})
            .get("poll_count"),
            "download_saved": (result.get("output_json") or {}).get("download_saved"),
        }
    )
finally:
    server.shutdown()
