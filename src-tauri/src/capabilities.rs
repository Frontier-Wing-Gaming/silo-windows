//! What the mpv player declares to the playback planner (protocol v3).
//!
//! mpv decodes through FFmpeg, so it plays nearly every codec and container
//! in software, uses hardware decode where it can, and tone-maps HDR itself.
//! Everything is `declared` evidence: the desktop shell cannot attest the GPU
//! decoder or the audio sink, so no passthrough is claimed.

use serde_json::{json, Value};

const VIDEO: &[&str] = &["h264", "hevc", "av1", "vp9", "vp8", "mpeg2video", "mpeg4", "vc1"];
const AUDIO: &[&str] = &[
    "aac", "mp3", "ac3", "eac3", "truehd", "dts", "flac", "opus", "vorbis", "alac", "pcm_s16le", "pcm_s24le", "mp2",
];
const CONTAINERS: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "ts", "m2ts", "mpegts"];

pub const CLIENT_FEATURES: &[&str] = &["seek_reanchor_v1", "direct_stream_resume_v1", "subrip_sidecar_v1"];

fn sdr_output() -> Value {
    json!({ "hdr10": false, "hdr10_plus": false, "hlg": false, "dolby_vision_profiles": [] })
}

fn subtitles(original: bool) -> Value {
    json!({
        "embedded_text": original,
        "sidecar_text": true,
        "ass_styling": true,
        "embedded_bitmap": original,
        "sidecar_bitmap": true,
        "font_attachments": original
    })
}

fn delivery(containers: &[&str], claims: &[&str], original: bool) -> Value {
    json!({
        "enabled": true,
        "supported_on_device": true,
        "containers": containers,
        "video_codecs": VIDEO,
        "audio_decode_codecs": AUDIO,
        "audio_passthrough_codecs": [],
        "hdr_details": sdr_output(),
        "subtitles": subtitles(original),
        "features": [],
        // mpv keeps the headers it opened the stream with; the plan's URL
        // carries its own signed reference.
        "auth_header_refresh": false,
        "validated_claims": claims,
        "transformations": []
    })
}

pub fn client_capabilities() -> Value {
    json!({
        "video_evidence": "declared",
        "audio_evidence": "declared",
        "codecs_video": VIDEO,
        "codecs_video_hardware": VIDEO,
        "codecs_audio": AUDIO,
        "containers": CONTAINERS,
        "max_resolution": "2160p",
        "hdr": false,
        "hdr_details": sdr_output()
    })
}

pub fn playback_context(os_version: &str) -> Value {
    // mpv picks any audio track in the file and tone-maps HDR and Dolby
    // Vision to the SDR output on its own.
    let claims = ["client_selected_audio_track_v1", "client_managed_dynamic_range_v1"];
    json!({
        "protocol_version": 3,
        "form_factor": "desktop",
        "app_version": crate::api::APP_VERSION,
        "app_channel": "community",
        "device": {
            "platform": "windows",
            "os_version": os_version,
            "platform_details": { "player": "mpv" }
        },
        "output": { "hdr_details": sdr_output() },
        "deliveries": {
            "original_http": delivery(CONTAINERS, &claims, true),
            "progressive": delivery(&["mp4", "mkv", "webm"], &claims, false),
            "hls": delivery(&["hls"], &[], false)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_speaks_protocol_v3_with_all_three_deliveries() {
        let ctx = playback_context("10.0.26100");
        assert_eq!(ctx["protocol_version"], 3);
        assert_eq!(ctx["device"]["platform"], "windows");
        for d in ["original_http", "progressive", "hls"] {
            assert_eq!(ctx["deliveries"][d]["enabled"], true, "{d} enabled");
        }
    }

    #[test]
    fn hls_makes_no_track_or_range_claims() {
        // The claims say mpv picks the audio track and tone-maps by itself; HLS
        // renditions are prepared by the server, so claiming them there would lie.
        let ctx = playback_context("x");
        assert_eq!(ctx["deliveries"]["hls"]["validated_claims"], json!([]));
        let original = ctx["deliveries"]["original_http"]["validated_claims"].as_array().unwrap();
        assert!(original.iter().any(|c| c == "client_selected_audio_track_v1"));
    }

    #[test]
    fn capabilities_are_declared_not_attested() {
        let caps = client_capabilities();
        assert_eq!(caps["video_evidence"], "declared");
        assert_eq!(caps["audio_evidence"], "declared");
        assert!(caps["codecs_video"].as_array().unwrap().iter().any(|c| c == "hevc"));
    }
}
