from __future__ import annotations

import base64
import json
import sys
import urllib.request
import uuid
from io import BytesIO
from pathlib import Path
from typing import Any


ROOT_DIR = Path(__file__).resolve().parents[1]
BRIDGE_DIR = ROOT_DIR / "python" / "bridge"
if str(BRIDGE_DIR) not in sys.path:
    sys.path.insert(0, str(BRIDGE_DIR))

from hgripe_api_bridge import run_task


def _parse_json_field(raw: str, field_name: str, default: Any) -> Any:
    text = (raw or "").strip()
    if not text:
        return default
    try:
        return json.loads(text)
    except json.JSONDecodeError as err:
        raise ValueError(f"{field_name} must be valid JSON: {err}") from err


def _parse_json_object(raw: str, field_name: str) -> dict[str, Any]:
    value = _parse_json_field(raw, field_name, {})
    if not isinstance(value, dict):
        raise ValueError(f"{field_name} must be a JSON object")
    return value


def _apply_openai_auth(
    params: dict[str, Any],
    auth_mode: str,
    credentials_ref: str,
    api_key_env: str,
    api_key: str,
) -> str | None:
    if auth_mode == "no_auth":
        params["no_auth"] = True
        return None

    if auth_mode == "credentials_ref" and credentials_ref.strip():
        return credentials_ref.strip()

    if api_key.strip():
        params["api_key"] = api_key.strip()
    elif api_key_env.strip():
        params["api_key_env"] = api_key_env.strip()

    return None


def _raise_if_failed(result: dict[str, Any], node_name: str) -> None:
    status = result.get("status")
    if status in {"succeeded", "cached"}:
        return

    error = result.get("error") or {}
    message = error.get("message") or json.dumps(result, ensure_ascii=False)
    raise RuntimeError(f"{node_name} failed: {message}")


def _image_bytes_from_item(item: dict[str, Any], timeout_seconds: float) -> bytes | None:
    b64_json = item.get("b64_json")
    if isinstance(b64_json, str) and b64_json.strip():
        return base64.b64decode(b64_json)

    url = item.get("url")
    if isinstance(url, str) and url.strip():
        request = urllib.request.Request(url, headers={"User-Agent": "H-Gripe-ComfyUI"})
        with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
            return response.read()

    return None


def _images_to_tensor(result: dict[str, Any], timeout_ms: int):
    import numpy as np
    import torch
    from PIL import Image

    output_json = result.get("output_json") or {}
    images = output_json.get("images") or []
    if not isinstance(images, list) or not images:
        raise RuntimeError("OpenAI-compatible image result does not contain images")

    tensors = []
    timeout_seconds = max(1.0, min(float(timeout_ms) / 1000.0, 300.0))
    expected_size: tuple[int, int] | None = None

    for item in images:
        if not isinstance(item, dict):
            continue
        image_bytes = _image_bytes_from_item(item, timeout_seconds)
        if image_bytes is None:
            continue

        image = Image.open(BytesIO(image_bytes)).convert("RGB")
        if expected_size is None:
            expected_size = image.size
        elif image.size != expected_size:
            raise RuntimeError(
                "OpenAI-compatible image batch contains mixed image sizes; set n=1 or use one fixed size"
            )

        array = np.asarray(image).astype(np.float32) / 255.0
        tensors.append(torch.from_numpy(array)[None,])

    if not tensors:
        raise RuntimeError("OpenAI-compatible image result has no decodable b64_json or url image")

    return torch.cat(tensors, dim=0)


def _tensor_image_to_data_url(image: Any, image_index: int, image_format: str) -> str:
    import numpy as np
    from PIL import Image

    if len(image.shape) == 3:
        selected = image
    else:
        batch_size = int(image.shape[0])
        if image_index < 0 or image_index >= batch_size:
            raise ValueError(f"image_index must be between 0 and {batch_size - 1}")
        selected = image[image_index]

    array = selected.detach().cpu().numpy()
    array = np.clip(array * 255.0, 0, 255).astype(np.uint8)
    pil_image = Image.fromarray(array).convert("RGB")

    normalized_format = image_format.lower()
    if normalized_format not in {"png", "jpeg", "webp"}:
        raise ValueError("image_format must be png, jpeg, or webp")

    buffer = BytesIO()
    save_format = "JPEG" if normalized_format == "jpeg" else normalized_format.upper()
    pil_image.save(buffer, format=save_format)
    encoded = base64.b64encode(buffer.getvalue()).decode("ascii")
    mime_type = "image/jpeg" if normalized_format == "jpeg" else f"image/{normalized_format}"
    return f"data:{mime_type};base64,{encoded}"


class HGripeCustomHttpApi:
    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "url": ("STRING", {"default": "http://127.0.0.1:8199/"}),
                "method": (["GET", "POST", "PUT", "PATCH", "DELETE"], {"default": "GET"}),
                "headers_json": ("STRING", {"multiline": True, "default": "{}"}),
                "query_json": ("STRING", {"multiline": True, "default": "{}"}),
                "body_json": ("STRING", {"multiline": True, "default": ""}),
                "max_attempts": ("INT", {"default": 2, "min": 1, "max": 10, "step": 1}),
                "timeout_ms": (
                    "INT",
                    {"default": 30000, "min": 1000, "max": 600000, "step": 1000},
                ),
                "force_run_nonce": (
                    "INT",
                    {"default": 0, "min": 0, "max": 2147483647, "step": 1},
                ),
            }
        }

    RETURN_TYPES = ("STRING", "STRING")
    RETURN_NAMES = ("result_json", "status")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/API"

    def run(
        self,
        url: str,
        method: str,
        headers_json: str,
        query_json: str,
        body_json: str,
        max_attempts: int,
        timeout_ms: int,
        force_run_nonce: int,
    ):
        headers = _parse_json_object(headers_json, "headers_json")
        query = _parse_json_object(query_json, "query_json")
        body = _parse_json_field(body_json, "body_json", None)

        params: dict[str, Any] = {
            "url": url,
            "method": method,
            "headers": headers,
            "query": query,
        }
        if body is not None:
            params["json"] = body

        task = {
            "id": f"comfy-http-{uuid.uuid4()}",
            "provider": "custom_http",
            "operation": "request",
            "inputs": {"force_run_nonce": force_run_nonce},
            "params": params,
            "credentials_ref": None,
            "output_type": "json",
            "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
            "retry_policy": {
                "max_attempts": max_attempts,
                "backoff_ms": 500,
                "timeout_ms": timeout_ms,
            },
        }

        result = run_task(task)
        status = str(result.get("status", "unknown"))
        return (json.dumps(result, ensure_ascii=False, indent=2), status)


class HGripeOpenAICompatibleText:
    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "base_url": (
                    "STRING",
                    {"default": ""},
                ),
                "model": ("STRING", {"default": "gpt-4.1-mini"}),
                "credentials_ref": ("STRING", {"default": "openai-main"}),
                "auth_mode": (
                    ["credentials_ref", "env_or_key", "no_auth"],
                    {"default": "credentials_ref"},
                ),
                "api_key_env": ("STRING", {"default": "OPENAI_API_KEY"}),
                "api_key": ("STRING", {"default": ""}),
                "system_prompt": ("STRING", {"multiline": True, "default": ""}),
                "prompt": ("STRING", {"multiline": True, "default": "Hello"}),
                "temperature": (
                    "FLOAT",
                    {"default": 0.7, "min": 0.0, "max": 2.0, "step": 0.1},
                ),
                "max_tokens": (
                    "INT",
                    {"default": 1024, "min": 1, "max": 131072, "step": 1},
                ),
                "extra_body_json": ("STRING", {"multiline": True, "default": "{}"}),
                "max_attempts": ("INT", {"default": 2, "min": 1, "max": 10, "step": 1}),
                "timeout_ms": (
                    "INT",
                    {"default": 120000, "min": 1000, "max": 1200000, "step": 1000},
                ),
                "force_run_nonce": (
                    "INT",
                    {"default": 0, "min": 0, "max": 2147483647, "step": 1},
                ),
            }
        }

    RETURN_TYPES = ("STRING", "STRING", "STRING")
    RETURN_NAMES = ("text", "result_json", "status")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/API"

    def run(
        self,
        base_url: str,
        model: str,
        credentials_ref: str,
        auth_mode: str,
        api_key_env: str,
        api_key: str,
        system_prompt: str,
        prompt: str,
        temperature: float,
        max_tokens: int,
        extra_body_json: str,
        max_attempts: int,
        timeout_ms: int,
        force_run_nonce: int,
    ):
        extra_body = _parse_json_object(extra_body_json, "extra_body_json")
        params: dict[str, Any] = {
            "base_url": base_url,
            "model": model,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "extra_body": extra_body,
        }

        task_credentials_ref = _apply_openai_auth(
            params, auth_mode, credentials_ref, api_key_env, api_key
        )

        if system_prompt.strip():
            params["system_prompt"] = system_prompt

        task = {
            "id": f"comfy-openai-text-{uuid.uuid4()}",
            "provider": "openai_compatible",
            "operation": "chat.completions",
            "inputs": {
                "prompt": prompt,
                "force_run_nonce": force_run_nonce,
            },
            "params": params,
            "credentials_ref": task_credentials_ref,
            "output_type": "text",
            "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
            "retry_policy": {
                "max_attempts": max_attempts,
                "backoff_ms": 500,
                "timeout_ms": timeout_ms,
            },
        }

        result = run_task(task)
        status = str(result.get("status", "unknown"))
        output_json = result.get("output_json") or {}
        text = str(output_json.get("text") or "")
        return (text, json.dumps(result, ensure_ascii=False, indent=2), status)


class HGripeOpenAICompatibleImage:
    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "base_url": (
                    "STRING",
                    {"default": ""},
                ),
                "model": ("STRING", {"default": "gpt-image-1"}),
                "credentials_ref": ("STRING", {"default": "openai-main"}),
                "auth_mode": (
                    ["credentials_ref", "env_or_key", "no_auth"],
                    {"default": "credentials_ref"},
                ),
                "api_key_env": ("STRING", {"default": "OPENAI_API_KEY"}),
                "api_key": ("STRING", {"default": ""}),
                "prompt": ("STRING", {"multiline": True, "default": "A clean product photo"}),
                "size": ("STRING", {"default": "1024x1024"}),
                "n": ("INT", {"default": 1, "min": 1, "max": 8, "step": 1}),
                "response_format": (
                    ["provider_default", "b64_json", "url"],
                    {"default": "provider_default"},
                ),
                "quality": (
                    ["provider_default", "auto", "standard", "hd", "low", "medium", "high"],
                    {"default": "provider_default"},
                ),
                "style": (
                    ["provider_default", "vivid", "natural"],
                    {"default": "provider_default"},
                ),
                "output_format": (
                    ["provider_default", "png", "jpeg", "webp"],
                    {"default": "provider_default"},
                ),
                "extra_body_json": ("STRING", {"multiline": True, "default": "{}"}),
                "max_attempts": ("INT", {"default": 2, "min": 1, "max": 10, "step": 1}),
                "timeout_ms": (
                    "INT",
                    {"default": 180000, "min": 1000, "max": 1200000, "step": 1000},
                ),
                "force_run_nonce": (
                    "INT",
                    {"default": 0, "min": 0, "max": 2147483647, "step": 1},
                ),
            }
        }

    RETURN_TYPES = ("IMAGE", "STRING", "STRING")
    RETURN_NAMES = ("image", "result_json", "status")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/API"

    def run(
        self,
        base_url: str,
        model: str,
        credentials_ref: str,
        auth_mode: str,
        api_key_env: str,
        api_key: str,
        prompt: str,
        size: str,
        n: int,
        response_format: str,
        quality: str,
        style: str,
        output_format: str,
        extra_body_json: str,
        max_attempts: int,
        timeout_ms: int,
        force_run_nonce: int,
    ):
        extra_body = _parse_json_object(extra_body_json, "extra_body_json")
        params: dict[str, Any] = {
            "base_url": base_url,
            "model": model,
            "n": n,
            "extra_body": extra_body,
        }

        task_credentials_ref = _apply_openai_auth(
            params, auth_mode, credentials_ref, api_key_env, api_key
        )

        if size.strip() and size.strip() != "provider_default":
            params["size"] = size.strip()
        if response_format != "provider_default":
            params["response_format"] = response_format
        if quality != "provider_default":
            params["quality"] = quality
        if style != "provider_default":
            params["style"] = style
        if output_format != "provider_default":
            params["output_format"] = output_format

        task = {
            "id": f"comfy-openai-image-{uuid.uuid4()}",
            "provider": "openai_compatible",
            "operation": "image.generate",
            "inputs": {
                "prompt": prompt,
                "force_run_nonce": force_run_nonce,
            },
            "params": params,
            "credentials_ref": task_credentials_ref,
            "output_type": "image",
            "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
            "retry_policy": {
                "max_attempts": max_attempts,
                "backoff_ms": 500,
                "timeout_ms": timeout_ms,
            },
        }

        result = run_task(task)
        _raise_if_failed(result, "H-Gripe OpenAI Compatible Image")
        status = str(result.get("status", "unknown"))
        image = _images_to_tensor(result, timeout_ms)
        return (image, json.dumps(result, ensure_ascii=False, indent=2), status)


class HGripeOpenAICompatibleVision:
    @classmethod
    def INPUT_TYPES(cls):
        return {
            "required": {
                "image": ("IMAGE",),
                "base_url": (
                    "STRING",
                    {"default": ""},
                ),
                "model": ("STRING", {"default": "gpt-4.1-mini"}),
                "credentials_ref": ("STRING", {"default": "openai-main"}),
                "auth_mode": (
                    ["credentials_ref", "env_or_key", "no_auth"],
                    {"default": "credentials_ref"},
                ),
                "api_key_env": ("STRING", {"default": "OPENAI_API_KEY"}),
                "api_key": ("STRING", {"default": ""}),
                "system_prompt": ("STRING", {"multiline": True, "default": ""}),
                "prompt": ("STRING", {"multiline": True, "default": "Describe this image."}),
                "image_index": ("INT", {"default": 0, "min": 0, "max": 4095, "step": 1}),
                "image_format": (["png", "jpeg", "webp"], {"default": "png"}),
                "detail": (["auto", "low", "high"], {"default": "auto"}),
                "temperature": (
                    "FLOAT",
                    {"default": 0.2, "min": 0.0, "max": 2.0, "step": 0.1},
                ),
                "max_tokens": (
                    "INT",
                    {"default": 1024, "min": 1, "max": 131072, "step": 1},
                ),
                "extra_body_json": ("STRING", {"multiline": True, "default": "{}"}),
                "max_attempts": ("INT", {"default": 2, "min": 1, "max": 10, "step": 1}),
                "timeout_ms": (
                    "INT",
                    {"default": 120000, "min": 1000, "max": 1200000, "step": 1000},
                ),
                "force_run_nonce": (
                    "INT",
                    {"default": 0, "min": 0, "max": 2147483647, "step": 1},
                ),
            }
        }

    RETURN_TYPES = ("STRING", "STRING", "STRING")
    RETURN_NAMES = ("text", "result_json", "status")
    FUNCTION = "run"
    CATEGORY = "H-Gripe/API"

    def run(
        self,
        image,
        base_url: str,
        model: str,
        credentials_ref: str,
        auth_mode: str,
        api_key_env: str,
        api_key: str,
        system_prompt: str,
        prompt: str,
        image_index: int,
        image_format: str,
        detail: str,
        temperature: float,
        max_tokens: int,
        extra_body_json: str,
        max_attempts: int,
        timeout_ms: int,
        force_run_nonce: int,
    ):
        extra_body = _parse_json_object(extra_body_json, "extra_body_json")
        image_url = _tensor_image_to_data_url(image, image_index, image_format)
        messages: list[dict[str, Any]] = []

        if system_prompt.strip():
            messages.append({"role": "system", "content": system_prompt})

        messages.append(
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": image_url,
                            "detail": detail,
                        },
                    },
                ],
            }
        )

        params: dict[str, Any] = {
            "base_url": base_url,
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "extra_body": extra_body,
        }

        task_credentials_ref = _apply_openai_auth(
            params, auth_mode, credentials_ref, api_key_env, api_key
        )

        task = {
            "id": f"comfy-openai-vision-{uuid.uuid4()}",
            "provider": "openai_compatible",
            "operation": "vision.analyze",
            "inputs": {"force_run_nonce": force_run_nonce},
            "params": params,
            "credentials_ref": task_credentials_ref,
            "output_type": "text",
            "cache_policy": {"enabled": False, "ttl_seconds": None, "key": None},
            "retry_policy": {
                "max_attempts": max_attempts,
                "backoff_ms": 500,
                "timeout_ms": timeout_ms,
            },
        }

        result = run_task(task)
        _raise_if_failed(result, "H-Gripe OpenAI Compatible Vision")
        status = str(result.get("status", "unknown"))
        output_json = result.get("output_json") or {}
        text = str(output_json.get("text") or "")
        return (text, json.dumps(result, ensure_ascii=False, indent=2), status)


NODE_CLASS_MAPPINGS = {
    "HGripeCustomHttpApi": HGripeCustomHttpApi,
    "HGripeOpenAICompatibleText": HGripeOpenAICompatibleText,
    "HGripeOpenAICompatibleImage": HGripeOpenAICompatibleImage,
    "HGripeOpenAICompatibleVision": HGripeOpenAICompatibleVision,
}

NODE_DISPLAY_NAME_MAPPINGS = {
    "HGripeCustomHttpApi": "H-Gripe Custom HTTP API",
    "HGripeOpenAICompatibleText": "H-Gripe OpenAI Compatible Text",
    "HGripeOpenAICompatibleImage": "H-Gripe OpenAI Compatible Image",
    "HGripeOpenAICompatibleVision": "H-Gripe OpenAI Compatible Vision",
}
