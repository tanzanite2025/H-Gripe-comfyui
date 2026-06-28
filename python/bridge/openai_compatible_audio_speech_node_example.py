from __future__ import annotations

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[2]
if str(ROOT_DIR) not in sys.path:
    sys.path.insert(0, str(ROOT_DIR))

from custom_nodes.hgripe_api_nodes import HGripeOpenAICompatibleAudioSpeech


AUDIO_BYTES = b"ID3 local-openai-compatible-audio-speech-example"


class ExampleHandler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        request_size = int(self.headers.get("content-length", "0"))
        request_body = json.loads(self.rfile.read(request_size).decode("utf-8"))
        if request_body.get("input") != "hello audio node":
            self.send_response(400)
            self.end_headers()
            return

        self.send_response(200)
        self.send_header("content-type", "audio/mpeg")
        self.send_header("content-length", str(len(AUDIO_BYTES)))
        self.send_header("x-request-id", "local-openai-compatible-audio-speech-node-example")
        self.end_headers()
        self.wfile.write(AUDIO_BYTES)

    def log_message(self, format: str, *args: object) -> None:
        return


server = ThreadingHTTPServer(("127.0.0.1", 0), ExampleHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()

try:
    node = HGripeOpenAICompatibleAudioSpeech()
    audio_path, result_json, status = node.run(
        base_url=f"http://127.0.0.1:{server.server_port}",
        profile_ref="",
        model="local-tts-model",
        credentials_ref="",
        auth_mode="no_auth",
        api_key_env="",
        api_key="",
        voice="alloy",
        text="hello audio node",
        response_format="mp3",
        speed=1.0,
        instructions="",
        save_outputs="enable",
        extra_body_json="{}",
        max_attempts=2,
        timeout_ms=30000,
        force_run_nonce=0,
    )
    result = json.loads(result_json)
    print(
        {
            "status": status,
            "audio_path": audio_path,
            "provider_request_id": result.get("provider_request_id"),
            "output_files": result.get("output_files", []),
        }
    )
finally:
    server.shutdown()
