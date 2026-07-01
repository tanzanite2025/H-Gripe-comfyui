//! Native in-process FFmpeg decoder backend (feature `native-ffmpeg`).
//!
//! A second implementation of the media-engine decoder seam
//! ([`FrameSource`](super::video_engine::FrameSource)) that decodes with the
//! vendored libav* libraries directly — no Python/PyAV, no system ffmpeg. The
//! libraries live under `third_party/ffmpeg` (LGPL *shared*, cut from upstream
//! and locally maintained); they are linked via `rusty_ffmpeg` using the
//! `FFMPEG_*` env in `.cargo/config.toml`.
//!
//! It opens a fresh container per call (`probe` / `decode_frame`) keyed by the
//! `video` path the seam hands in — the media engine already caches decoded
//! frames, so there is no persistent per-file state to hold here. Every failure
//! returns `Err`, which the callers (`video_probe` / `video_scrub`) turn into a
//! fallback to the PyAV one-shot path, so enabling the feature never regresses
//! behaviour: a clip the native decoder chokes on still resolves via PyAV.
//!
//! # Safety
//! The body is FFI against libav. Raw pointers are confined to method scope and
//! freed by [`Decoder`]'s `Drop`; nothing is shared across threads (the seam is
//! `Send`, not `Sync`, and a source is only touched from the decode thread).

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;

use rusty_ffmpeg::ffi;

use super::video_engine::{FrameSource, PyAvFrameSource, VideoMeta};

/// libav's "no timestamp" sentinel (`AV_NOPTS_VALUE`), defined here so we don't
/// depend on the macro surviving into the generated bindings.
const AV_NOPTS_VALUE: i64 = i64::MIN;

/// Upper bound on frames decoded while walking forward from the seek keyframe to
/// the requested timestamp, so a pathological stream can't spin forever.
const MAX_FRAMES_TO_TARGET: u32 = 600;

/// Native libav-backed [`FrameSource`]. Zero-sized: all state is per-call.
pub(crate) struct NativeFfmpegFrameSource;

impl NativeFfmpegFrameSource {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl FrameSource for NativeFfmpegFrameSource {
    fn probe(&mut self, video: &Path) -> Result<VideoMeta, String> {
        let decoder = Decoder::open(video)?;
        decoder.probe()
    }

    fn decode_frame(
        &mut self,
        video: &Path,
        timestamp_sec: f64,
        poster_out: &Path,
    ) -> Result<PathBuf, String> {
        let mut decoder = Decoder::open(video)?;
        decoder.decode_to_png(timestamp_sec, poster_out)
    }
}

/// The native decoder with a PyAV safety net. `probe`/`decode_frame` try libav
/// first and, on any `Err` (unsupported codec, corrupt clip, a libav quirk),
/// fall back to the long-lived PyAV worker — so turning on `native-ffmpeg`
/// upgrades decoding where libav succeeds without losing any clip PyAV handled.
/// This is what [`super::video_engine::make_frame_source`] hands the engine.
pub(crate) struct FfmpegWithPyAvFallback {
    native: NativeFfmpegFrameSource,
    fallback: PyAvFrameSource,
}

impl FfmpegWithPyAvFallback {
    pub(crate) fn new(python: PathBuf, dir: PathBuf) -> Self {
        Self {
            native: NativeFfmpegFrameSource::new(),
            fallback: PyAvFrameSource::new(python, dir),
        }
    }
}

impl FrameSource for FfmpegWithPyAvFallback {
    fn probe(&mut self, video: &Path) -> Result<VideoMeta, String> {
        match self.native.probe(video) {
            Ok(meta) => Ok(meta),
            Err(_) => self.fallback.probe(video),
        }
    }

    fn decode_frame(
        &mut self,
        video: &Path,
        timestamp_sec: f64,
        poster_out: &Path,
    ) -> Result<PathBuf, String> {
        match self.native.decode_frame(video, timestamp_sec, poster_out) {
            Ok(path) => Ok(path),
            Err(_) => self.fallback.decode_frame(video, timestamp_sec, poster_out),
        }
    }
}

/// An open input container + its selected video decode context. Owns the raw
/// libav pointers and frees them on drop.
struct Decoder {
    fmt: *mut ffi::AVFormatContext,
    codec_ctx: *mut ffi::AVCodecContext,
    stream_index: i32,
    time_base: ffi::AVRational,
    avg_frame_rate: ffi::AVRational,
}

impl Decoder {
    fn open(video: &Path) -> Result<Self, String> {
        let path = CString::new(video.to_string_lossy().as_bytes())
            .map_err(|_| "video path contains a NUL byte".to_string())?;
        unsafe {
            let mut fmt: *mut ffi::AVFormatContext = ptr::null_mut();
            let ret =
                ffi::avformat_open_input(&mut fmt, path.as_ptr(), ptr::null_mut(), ptr::null_mut());
            if ret < 0 {
                return Err(format!("avformat_open_input failed ({ret})"));
            }
            if ffi::avformat_find_stream_info(fmt, ptr::null_mut()) < 0 {
                ffi::avformat_close_input(&mut fmt);
                return Err("avformat_find_stream_info failed".to_string());
            }

            let mut decoder: *const ffi::AVCodec = ptr::null();
            let stream_index =
                ffi::av_find_best_stream(fmt, ffi::AVMEDIA_TYPE_VIDEO, -1, -1, &mut decoder, 0);
            if stream_index < 0 || decoder.is_null() {
                ffi::avformat_close_input(&mut fmt);
                return Err("no decodable video stream found".to_string());
            }

            let stream = *(*fmt).streams.add(stream_index as usize);
            let codecpar = (*stream).codecpar;
            let codec_ctx = ffi::avcodec_alloc_context3(decoder);
            if codec_ctx.is_null() {
                ffi::avformat_close_input(&mut fmt);
                return Err("avcodec_alloc_context3 failed".to_string());
            }
            let mut this = Decoder {
                fmt,
                codec_ctx,
                stream_index,
                time_base: (*stream).time_base,
                avg_frame_rate: (*stream).avg_frame_rate,
            };
            if ffi::avcodec_parameters_to_context(codec_ctx, codecpar) < 0 {
                return Err("avcodec_parameters_to_context failed".to_string());
            }
            if ffi::avcodec_open2(codec_ctx, decoder, ptr::null_mut()) < 0 {
                return Err("avcodec_open2 failed".to_string());
            }
            // Take ownership out of the temporary so its Drop doesn't run early.
            this.fmt = fmt;
            Ok(this)
        }
    }

    fn probe(&self) -> Result<VideoMeta, String> {
        unsafe {
            let width = (*self.codec_ctx).width.max(0) as u32;
            let height = (*self.codec_ctx).height.max(0) as u32;

            let raw_duration = (*self.fmt).duration;
            let duration_sec = if raw_duration != AV_NOPTS_VALUE && raw_duration > 0 {
                Some(raw_duration as f64 / ffi::AV_TIME_BASE as f64)
            } else {
                None
            };

            let fps = {
                let r = self.avg_frame_rate;
                if r.num > 0 && r.den > 0 {
                    Some(r.num as f64 / r.den as f64)
                } else {
                    None
                }
            };

            let codec = {
                let name = ffi::avcodec_get_name((*self.codec_ctx).codec_id);
                if name.is_null() {
                    None
                } else {
                    Some(CStr::from_ptr(name).to_string_lossy().into_owned())
                }
            };

            Ok(VideoMeta {
                width,
                height,
                duration_sec,
                fps,
                codec,
            })
        }
    }

    fn decode_to_png(&mut self, timestamp_sec: f64, poster_out: &Path) -> Result<PathBuf, String> {
        unsafe {
            // Target timestamp in the stream's time base: ts / (num/den).
            let q = self.time_base;
            let target_ts = if q.num > 0 && q.den > 0 {
                (timestamp_sec * q.den as f64 / q.num as f64).round() as i64
            } else {
                0
            };

            if timestamp_sec > 0.0 {
                // Seek to the keyframe at or before the target, then decode
                // forward. Best-effort: a seek failure just decodes from where
                // we are.
                let _ = ffi::av_seek_frame(
                    self.fmt,
                    self.stream_index,
                    target_ts,
                    ffi::AVSEEK_FLAG_BACKWARD as i32,
                );
                ffi::avcodec_flush_buffers(self.codec_ctx);
            }

            let packet = ffi::av_packet_alloc();
            let frame = ffi::av_frame_alloc();
            if packet.is_null() || frame.is_null() {
                if !packet.is_null() {
                    let mut p = packet;
                    ffi::av_packet_free(&mut p);
                }
                if !frame.is_null() {
                    let mut f = frame;
                    ffi::av_frame_free(&mut f);
                }
                return Err("failed to allocate libav packet/frame".to_string());
            }

            let mut got = false;
            let mut walked: u32 = 0;
            while ffi::av_read_frame(self.fmt, packet) >= 0 {
                if (*packet).stream_index == self.stream_index
                    && ffi::avcodec_send_packet(self.codec_ctx, packet) >= 0
                {
                    loop {
                        let r = ffi::avcodec_receive_frame(self.codec_ctx, frame);
                        if r < 0 {
                            break; // EAGAIN (need more packets) or EOF
                        }
                        let pts = {
                            let best = (*frame).best_effort_timestamp;
                            if best != AV_NOPTS_VALUE {
                                best
                            } else {
                                (*frame).pts
                            }
                        };
                        walked += 1;
                        if pts >= target_ts || walked >= MAX_FRAMES_TO_TARGET {
                            got = true;
                            break;
                        }
                    }
                }
                ffi::av_packet_unref(packet);
                if got {
                    break;
                }
            }

            // Flush the decoder if EOF arrived before we produced a frame.
            if !got {
                ffi::avcodec_send_packet(self.codec_ctx, ptr::null());
                if ffi::avcodec_receive_frame(self.codec_ctx, frame) >= 0 {
                    got = true;
                }
            }

            let result = if got {
                self.frame_to_png(frame, poster_out)
            } else {
                Err("no frame decoded at the requested timestamp".to_string())
            };

            let mut p = packet;
            ffi::av_packet_free(&mut p);
            let mut f = frame;
            ffi::av_frame_free(&mut f);
            result
        }
    }

    /// Scale a decoded frame to RGBA and write it to `poster_out` as PNG.
    unsafe fn frame_to_png(
        &self,
        frame: *mut ffi::AVFrame,
        poster_out: &Path,
    ) -> Result<PathBuf, String> {
        let width = (*frame).width;
        let height = (*frame).height;
        if width <= 0 || height <= 0 {
            return Err("decoded frame has non-positive dimensions".to_string());
        }

        let sws = ffi::sws_getContext(
            width,
            height,
            (*frame).format,
            width,
            height,
            ffi::AV_PIX_FMT_RGBA,
            ffi::SWS_BILINEAR as i32,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        );
        if sws.is_null() {
            return Err("sws_getContext failed".to_string());
        }

        let stride = width * 4;
        let mut buffer = vec![0u8; (stride * height) as usize];
        let dst_data: [*mut u8; 4] = [
            buffer.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ];
        let dst_stride: [i32; 4] = [stride, 0, 0, 0];

        let scaled = ffi::sws_scale(
            sws,
            (*frame).data.as_ptr() as *const *const u8,
            (*frame).linesize.as_ptr(),
            0,
            height,
            dst_data.as_ptr(),
            dst_stride.as_ptr(),
        );
        ffi::sws_freeContext(sws);
        if scaled <= 0 {
            return Err("sws_scale produced no output".to_string());
        }

        let image = image::RgbaImage::from_raw(width as u32, height as u32, buffer)
            .ok_or_else(|| "RGBA buffer did not match frame dimensions".to_string())?;
        image
            .save(poster_out)
            .map_err(|err| format!("failed to write poster {}: {err}", poster_out.display()))?;
        Ok(poster_out.to_path_buf())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            if !self.codec_ctx.is_null() {
                ffi::avcodec_free_context(&mut self.codec_ctx);
            }
            if !self.fmt.is_null() {
                ffi::avformat_close_input(&mut self.fmt);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the full link + runtime-DLL-load chain: constructing the
    /// source and probing a missing file must return `Err` (from
    /// `avformat_open_input`) rather than panic or fail to link. This is the
    /// smoke test that the vendored libraries load on CI.
    #[test]
    fn probe_missing_file_errors_cleanly() {
        let mut source = NativeFfmpegFrameSource::new();
        let result = source.probe(Path::new("definitely_not_a_real_clip_zzx.mp4"));
        assert!(result.is_err());
    }

    #[test]
    fn decode_missing_file_errors_cleanly() {
        let mut source = NativeFfmpegFrameSource::new();
        let out = std::env::temp_dir().join("hgripe_native_ffmpeg_probe_test.png");
        let result =
            source.decode_frame(Path::new("definitely_not_a_real_clip_zzx.mp4"), 0.0, &out);
        assert!(result.is_err());
    }
}
