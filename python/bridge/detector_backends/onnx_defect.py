"""ONNX semantic-defect detector (opt-in, via ``onnxruntime``).

This is the first learned ``engine`` for the Detail Watchdog node. It graduates
the semantic watch targets the Phase 1 rule layer records as ``skipped`` —
``hands`` / ``text`` / ``logo`` — into real findings by running a single-stage
object detector exported to ONNX. It stays **opt-in**: the backend is only used
when the node's ``engine`` param is ``onnx_defect`` *and* both the optional dep
(``onnxruntime``) and the model weight are present; otherwise the caller keeps
the always-available rule-only report.

Nothing heavy is imported at module load — ``onnxruntime`` is only touched
inside :meth:`available` (via :func:`importlib.util.find_spec`) and
:meth:`detect`.

Weight resolution order:
1. ``HGRIPE_WATCHDOG_MODEL`` (explicit path, for dev / CI), else
2. ``<model cache>/watchdog_defect.onnx`` where the cache dir is
   ``HGRIPE_MODEL_CACHE`` or the bundled ``resources/models`` dir.

The weight is **not** bundled in the installer; ``scripts`` can fetch it into
the cache dir, exactly like the SAM 2 / ViTMatte / Real-ESRGAN weights.

Model contract (a standard single-image detector):
* input: one tensor ``[1, 3, H, W]`` float32, RGB, values in ``0..1``. The
  network's fixed spatial size is read from the model; a dynamic axis falls back
  to ``_DEFAULT_SIZE``. The image is letterbox-resized (aspect preserved) into
  that square so detections map back to the original pixels.
* outputs: ``boxes`` ``[N, 4]`` ``xyxy`` in input-pixel coords, ``scores``
  ``[N]``, and ``labels`` ``[N]`` int class ids. Outputs are matched by name
  (``boxes`` / ``scores`` / ``labels``), falling back to positional order.
* a sidecar ``<weight>.labels.json`` maps each class id to a watch-target name,
  e.g. ``{"0": "hands", "1": "text", "2": "logo"}``. Without it the backend uses
  the natural order of :attr:`targets`.
"""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
from typing import Any

from . import DetectorUnavailable, model_cache_dir

_DEFAULT_WEIGHT_NAME = "watchdog_defect.onnx"
# Fallback square size when the model declares a dynamic spatial axis.
_DEFAULT_SIZE = 640
# Per-target score floor: a detection below this is ignored (tunable later).
_SCORE_FLOOR = 0.35
# How each covered target maps into the QualityReport. The action drives Detail
# Repaint selection (default repaint action is ``detail_redraw``).
_ISSUE_TYPE = {
    "hands": "malformed_hands",
    "text": "garbled_text",
    "logo": "deformed_logo",
}
_SUGGESTED_ACTION = {
    "hands": "detail_redraw",
    "text": "detail_redraw",
    "logo": "detail_redraw",
}


class OnnxDefectBackend:
    id = "onnx_defect"
    targets = ("hands", "text", "logo")

    def weight_path(self) -> Path:
        override = (os.environ.get("HGRIPE_WATCHDOG_MODEL") or "").strip()
        if override:
            return Path(override)
        return model_cache_dir() / _DEFAULT_WEIGHT_NAME

    def available(self) -> tuple[bool, str]:
        """Cheap probe: optional dep importable + weight present on disk.

        Uses ``find_spec`` so we never actually import ``onnxruntime`` (which can
        pull in heavy native providers) just to report availability.
        """
        if importlib.util.find_spec("onnxruntime") is None:
            return False, "missing optional dependency: onnxruntime"
        weight = self.weight_path()
        if not weight.is_file():
            return (
                False,
                f"weight not found: {weight} "
                "(set HGRIPE_WATCHDOG_MODEL or fetch into HGRIPE_MODEL_CACHE)",
            )
        return True, "ready"

    def _label_map(self, weight: Path) -> dict[int, str]:
        """Class-id -> target name, from the sidecar JSON or the target order."""
        sidecar = weight.with_suffix(weight.suffix + ".labels.json")
        if sidecar.is_file():
            try:
                raw = json.loads(sidecar.read_text(encoding="utf-8"))
            except (ValueError, OSError):
                raw = None
            if isinstance(raw, dict):
                mapped: dict[int, str] = {}
                for key, value in raw.items():
                    try:
                        mapped[int(key)] = str(value)
                    except (TypeError, ValueError):
                        continue
                if mapped:
                    return mapped
        return {idx: name for idx, name in enumerate(self.targets)}

    def detect(self, rgb: Any, alpha: Any, watch: set[str]) -> list[dict[str, Any]]:
        """Run the detector and map detections of watched targets to issues.

        Raises :class:`DetectorUnavailable` if deps / weights vanished since the
        probe. Detections of targets not in ``watch`` are dropped.
        """
        ok, reason = self.available()
        if not ok:
            raise DetectorUnavailable(reason)

        import numpy as np
        import onnxruntime as ort

        weight = self.weight_path()
        label_map = self._label_map(weight)

        session = ort.InferenceSession(
            str(weight), providers=["CPUExecutionProvider"]
        )
        spec = session.get_inputs()[0]
        # Spatial dims are the trailing two axes of an NCHW input; a non-int
        # (dynamic) axis falls back to the default square size.
        net_h = spec.shape[2] if isinstance(spec.shape[2], int) else _DEFAULT_SIZE
        net_w = spec.shape[3] if isinstance(spec.shape[3], int) else _DEFAULT_SIZE

        src_h, src_w = rgb.shape[:2]
        tensor, scale, pad = _letterbox(rgb, net_w, net_h, np)

        raw = session.run(None, {spec.name: tensor})
        boxes, scores, labels = _named_outputs(session, raw)
        if boxes is None or boxes.size == 0:
            return []

        issues: list[dict[str, Any]] = []
        for box, score, label in zip(boxes, scores, labels):
            target = label_map.get(int(label))
            if target is None or target not in watch:
                continue
            confidence = float(score)
            if confidence < _SCORE_FLOOR:
                continue
            # Undo letterbox: subtract pad, divide by scale, clamp to the image.
            x1 = (float(box[0]) - pad[0]) / scale
            y1 = (float(box[1]) - pad[1]) / scale
            x2 = (float(box[2]) - pad[0]) / scale
            y2 = (float(box[3]) - pad[1]) / scale
            bbox = [
                int(max(0, min(src_w, round(x1)))),
                int(max(0, min(src_h, round(y1)))),
                int(max(0, min(src_w, round(x2)))),
                int(max(0, min(src_h, round(y2)))),
            ]
            if bbox[2] <= bbox[0] or bbox[3] <= bbox[1]:
                continue
            issues.append(
                {
                    "type": _ISSUE_TYPE.get(target, target),
                    "confidence": round(min(0.99, confidence), 2),
                    "bbox": bbox,
                    "suggested_action": _SUGGESTED_ACTION.get(target, "detail_redraw"),
                }
            )
        return issues


def _letterbox(rgb: Any, net_w: int, net_h: int, np: Any) -> tuple[Any, float, tuple[float, float]]:
    """Aspect-preserving resize into a ``net_w x net_h`` square, returning the
    NCHW float32 tensor, the applied ``scale`` and the ``(pad_x, pad_y)`` offset.
    """
    from PIL import Image

    src_h, src_w = rgb.shape[:2]
    scale = min(net_w / max(src_w, 1), net_h / max(src_h, 1))
    new_w = max(1, int(round(src_w * scale)))
    new_h = max(1, int(round(src_h * scale)))
    resized = Image.fromarray(rgb, "RGB").resize((new_w, new_h), Image.BILINEAR)

    canvas = np.zeros((net_h, net_w, 3), dtype=np.float32)
    pad_x = (net_w - new_w) / 2.0
    pad_y = (net_h - new_h) / 2.0
    off_x, off_y = int(pad_x), int(pad_y)
    canvas[off_y : off_y + new_h, off_x : off_x + new_w, :] = (
        np.asarray(resized, dtype=np.float32) / 255.0
    )
    tensor = np.transpose(canvas, (2, 0, 1))[None, ...].astype(np.float32)
    return tensor, scale, (float(off_x), float(off_y))


def _named_outputs(session: Any, raw: list[Any]) -> tuple[Any, Any, Any]:
    """Pick ``(boxes, scores, labels)`` from the session outputs by name.

    Falls back to positional order (boxes, scores, labels) when the model does
    not name them.
    """
    import numpy as np

    names = [o.name.lower() for o in session.get_outputs()]
    by_name: dict[str, Any] = {}
    for name, value in zip(names, raw):
        by_name[name] = value

    def pick(*keys: str) -> Any:
        for key in keys:
            for name, value in by_name.items():
                if key in name:
                    return value
        return None

    boxes = pick("box")
    scores = pick("score", "conf")
    labels = pick("label", "class")
    if boxes is None and len(raw) >= 1:
        boxes = raw[0]
    if scores is None and len(raw) >= 2:
        scores = raw[1]
    if labels is None and len(raw) >= 3:
        labels = raw[2]

    boxes = np.asarray(boxes).reshape(-1, 4) if boxes is not None else None
    scores = np.asarray(scores).reshape(-1) if scores is not None else None
    labels = (
        np.asarray(labels).reshape(-1).astype(int) if labels is not None else None
    )
    if boxes is not None:
        n = boxes.shape[0]
        if scores is None:
            scores = np.ones(n, dtype=np.float32)
        if labels is None:
            labels = np.zeros(n, dtype=int)
    return boxes, scores, labels
