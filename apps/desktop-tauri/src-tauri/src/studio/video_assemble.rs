//! The `videoAssemble` node executor: encodes an ordered frame-image sequence
//! into a video file through the media engine's FFmpeg backend (the long-lived
//! PyAV worker's `assemble` command). This is the runner's video
//! assembly/export card: connect frames (a batch's saved outputs, a directory
//! of rendered stills), pick fps/codec, get an `.mp4` on disk.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use super::graph::{
    number_param, optional, resolve_output_dir, studio_output_map, studio_value_to_string,
    StudioGraphNode,
};
use crate::psd::{project_python, resolve_project_dir};

/// Shape of the worker's `assemble` payload.
#[derive(Debug, Deserialize)]
struct AssemblePayload {
    #[serde(default)]
    video_path: String,
    #[serde(default)]
    frame_count: u64,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    fps: Option<f64>,
    #[serde(default)]
    duration_sec: Option<f64>,
    #[serde(default)]
    codec: Option<String>,
}

/// Collect the ordered frame paths from the `frames` input (a JSON array or a
/// newline-delimited string) or, failing that, the node's `frames` param.
fn collect_frames(node: &StudioGraphNode, inputs: &BTreeMap<String, Value>) -> Vec<String> {
    let value = inputs
        .get("frames")
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| node.params.get("frames").cloned().unwrap_or(Value::Null));
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| studio_value_to_string(Some(item)))
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .collect(),
        other => studio_value_to_string(Some(&other))
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect(),
    }
}

/// The output video path: `output_dir` (param or runtime default) joined with
/// `output_name` (default `assembled-<millis>.mp4`, extension appended when
/// missing).
fn resolve_output_path(node: &StudioGraphNode) -> Result<String, String> {
    let dir = resolve_output_dir(node)?;
    let name = match optional(studio_value_to_string(node.params.get("output_name"))) {
        Some(name) if name.contains('.') => name,
        Some(name) => format!("{name}.mp4"),
        None => {
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|err| format!("system clock error: {err}"))?
                .as_millis();
            format!("assembled-{millis}.mp4")
        }
    };
    Ok(Path::new(&dir).join(name).to_string_lossy().to_string())
}

pub(super) fn execute_studio_video_assemble(
    node: &StudioGraphNode,
    inputs: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let frames = collect_frames(node, inputs);
    if frames.is_empty() {
        return Err(
            "Video Assemble needs at least one frame (connect a frames input or set the frames param)"
                .to_string(),
        );
    }
    for frame in &frames {
        if !Path::new(frame).is_file() {
            return Err(format!("frame image does not exist: {frame}"));
        }
    }

    let fps = number_param(node, "fps", 24.0);
    if !(fps > 0.0) {
        return Err("fps must be positive".to_string());
    }
    let codec = optional(studio_value_to_string(node.params.get("codec")))
        .unwrap_or_else(|| "libx264".to_string());
    let out_path = resolve_output_path(node)?;

    let dir = resolve_project_dir(&None)?;
    let python = project_python(&dir);
    let args = json!({
        "frames": frames,
        "out": out_path,
        "fps": fps,
        "codec": codec,
    });
    let stdout = super::video_worker::run(&python, &dir, "assemble", &args)?;
    let payload: AssemblePayload = serde_json::from_str(stdout.trim()).map_err(|err| {
        format!(
            "could not parse video assemble result: {err} (raw: {})",
            stdout.trim()
        )
    })?;

    let video_path = if payload.video_path.is_empty() {
        out_path
    } else {
        payload.video_path
    };
    Ok(studio_output_map([
        ("video", json!(video_path)),
        ("frame_count", json!(payload.frame_count)),
        ("duration_sec", json!(payload.duration_sec)),
        (
            "assemble_report",
            json!({
                "width": payload.width,
                "height": payload.height,
                "fps": payload.fps,
                "codec": payload.codec,
                "frame_count": payload.frame_count,
                "duration_sec": payload.duration_sec,
            }),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> StudioGraphNode {
        StudioGraphNode {
            id: "n1".to_string(),
            kind: "videoAssemble".to_string(),
            params: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_missing_frames() {
        let err = execute_studio_video_assemble(&node(), &BTreeMap::new()).unwrap_err();
        assert!(err.contains("needs at least one frame"), "{err}");
    }

    #[test]
    fn rejects_nonexistent_frame_before_encoding() {
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "frames".to_string(),
            json!(["Z:/definitely/missing-frame.png"]),
        );
        let err = execute_studio_video_assemble(&node(), &inputs).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn collect_frames_accepts_array_and_multiline_string() {
        let mut inputs = BTreeMap::new();
        inputs.insert("frames".to_string(), json!(["a.png", "  ", "b.png"]));
        assert_eq!(collect_frames(&node(), &inputs), vec!["a.png", "b.png"]);

        inputs.insert("frames".to_string(), json!("a.png\n\n b.png \n"));
        assert_eq!(collect_frames(&node(), &inputs), vec!["a.png", "b.png"]);
    }

    #[test]
    fn collect_frames_falls_back_to_param() {
        let mut n = node();
        n.params.insert("frames".to_string(), json!("x.png\ny.png"));
        assert_eq!(collect_frames(&n, &BTreeMap::new()), vec!["x.png", "y.png"]);
    }

    #[test]
    fn rejects_nonpositive_fps() {
        let dir = std::env::temp_dir();
        let frame = dir.join("hgripe-video-assemble-fps-test.png");
        std::fs::write(&frame, b"not really a png").unwrap();
        let mut n = node();
        n.params.insert("fps".to_string(), json!(0));
        let mut inputs = BTreeMap::new();
        inputs.insert("frames".to_string(), json!([frame.to_string_lossy()]));
        let err = execute_studio_video_assemble(&n, &inputs).unwrap_err();
        assert!(err.contains("fps must be positive"), "{err}");
        let _ = std::fs::remove_file(&frame);
    }

    #[test]
    fn output_name_gets_mp4_extension() {
        let mut n = node();
        n.params.insert("output_dir".to_string(), json!("C:/out"));
        n.params.insert("output_name".to_string(), json!("clip"));
        let path = resolve_output_path(&n).unwrap();
        assert!(path.ends_with("clip.mp4"), "{path}");

        n.params
            .insert("output_name".to_string(), json!("clip.webm"));
        let path = resolve_output_path(&n).unwrap();
        assert!(path.ends_with("clip.webm"), "{path}");
    }
}
