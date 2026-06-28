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

from custom_nodes.hgripe_api_nodes import HGripeOpenAICompatibleAudioText


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = self.rfile.read(request_size)
        transcript = (
            "hello audio text node"
            if b'name="file"' in request_body and b'name="model"' in request_body
            else "missing multipart file"
        )
        body = json.dumps({"text": transcript}).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-request-id", "local-openai-compatible-audio-text-node-example")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

audio_path = Path(tempfile.gettempdir()) / "hgripe-openai-compatible-audio-text-example.wav"
audio_path.write_bytes(b"RIFF local-openai-compatible-audio-text-example")

try:
    node = HGripeOpenAICompatibleAudioText()
    text, result_json, status = node.run(
        audio_path=str(audio_path),
        operation="audio.transcriptions",
        base_url=f"http://127.0.0.1:{server.server_port}",
        profile_ref="",
        model="local-transcribe-model",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        language="",
        prompt="",
        response_format="json",
        temperature=0.0,
        extra_body_json="{}",
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
    audio_path.unlink(missing_ok=True)
