from __future__ import annotations

import json
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

from custom_nodes.hgripe_api_nodes import HGripeCustomHttpMultipartApi


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = self.rfile.read(request_size)
        uploaded = (
            b'name="image"; filename="input.png"' in request_body
            and b'name="prompt"' in request_body
            and b"local multipart upload" in request_body
        )
        body = json.dumps({"ok": True, "uploaded": uploaded}).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-custom-http-multipart-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

upload_path = Path(tempfile.gettempdir()) / "hgripe-custom-http-multipart-example.png"
upload_path.write_bytes(b"\x89PNG\r\n\x1a\nlocal multipart upload")

try:
    node = HGripeCustomHttpMultipartApi()
    output_path, result_json, status = node.run(
        url=f"http://127.0.0.1:{server.server_port}/upload",
        method="POST",
        profile_ref="",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        headers_json="{}",
        query_json="{}",
        fields_json='{"prompt":"local multipart upload","strength":0.75}',
        file_path=str(upload_path),
        file_field="image",
        file_name="input.png",
        file_mime_type="image/png",
        save_response="disable",
        output_extension="",
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
            "uploaded": (result.get("output_json") or {}).get("body", {}).get("uploaded"),
        }
    )
finally:
    server.shutdown()
    upload_path.unlink(missing_ok=True)
