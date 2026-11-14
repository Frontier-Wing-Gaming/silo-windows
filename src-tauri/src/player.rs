//! Plays one title in mpv and keeps the server informed: start, sequenced
//! progress every few seconds, and a stop with the final position.

use crate::api::{is_status, str_field, Client, Viewer};
use crate::capabilities;
use crate::session::Session;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::process::Command;
use url::Url;

const PROGRESS_EVERY: Duration = Duration::from_secs(10);

/// What the page asked to play: the `/watch/:id` route's parameters.
#[derive(Debug, Deserialize)]
pub struct PlayRequest {
    pub content_id: String,
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub library_id: Option<String>,
    #[serde(default)]
    pub restart: bool,
}

pub struct Player {
    pub client: Client,
    pub session: Arc<Session>,
    pub mpv: PathBuf,
    pub temp: PathBuf,
}

struct Started {
    session_id: String,
    installation_id: String,
    plan: Value,
}

impl Player {
    async fn api(&self, method: Method, path: &str, viewer: &Viewer, body: Option<&Value>) -> Result<Value> {
        self.session.send(&self.client, method, path, viewer, body).await
    }

    /// Resolves the title the route names to the item that actually plays
    /// (a series or season plays its `play_content_id`).
    async fn resolve(&self, req: &PlayRequest, viewer: &Viewer) -> Result<(Value, String)> {
        let mut path = format!("/api/v2/catalog/items/{}", urlencode(&req.content_id));
        let mut query = vec![];
        if let Some(l) = &req.library_id {
            query.push(format!("library_id={}", urlencode(l)));
        }
        if let Some(f) = &req.file_id {
            query.push(format!("file_id={}", urlencode(f)));
        }
        if !query.is_empty() {
            path = format!("{path}?{}", query.join("&"));
        }
        let mut item = self.api(Method::GET, &path, viewer, None).await?;
        if item.get("versions").and_then(Value::as_array).is_none_or(Vec::is_empty) {
            if let Some(next) = item.get("play_content_id").and_then(Value::as_str).filter(|n| *n != req.content_id) {
                let p = format!("/api/v2/catalog/items/{}", urlencode(next));
                item = self.api(Method::GET, &p, viewer, None).await?;
            }
        }
        let versions = item.get("versions").and_then(Value::as_array).cloned().unwrap_or_default();
        let has = |id: &str| versions.iter().any(|v| v.get("file_id").and_then(Value::as_str) == Some(id));
        let file_id = req
            .file_id
            .clone()
            .filter(|f| has(f))
            .or_else(|| item.pointer("/user_data/last_file_id").and_then(Value::as_str).filter(|f| has(f)).map(str::to_owned))
            .or_else(|| versions.first().and_then(|v| v.get("file_id")).and_then(Value::as_str).map(str::to_owned))
            .ok_or_else(|| anyhow!("this title has no playable file"))?;
        Ok((item, file_id))
    }

    async fn start(&self, req: &PlayRequest, viewer: &Viewer, item: &Value, file_id: &str) -> Result<Started> {
        let caps = self.api(Method::GET, "/api/v2/playback/capabilities", viewer, None).await?;
        if caps.get("allowed").and_then(Value::as_bool) == Some(false) {
            bail!("playback is not configured on this server");
        }
        let installation_id = str_field(&caps, "installation_id")?;
        // The resume point lives in the viewer's watch state; older items
        // also carried it at the top level.
        let start_position = if req.restart {
            0.0
        } else {
            item.pointer("/user_data/position_seconds")
                .or_else(|| item.get("position_seconds"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
        };
        let mut body = json!({
            "installation_id": installation_id,
            "protocol_version": 3,
            "client_features": capabilities::CLIENT_FEATURES,
            "file_id": file_id,
            "profile_id": viewer.profile_id,
            "playback_attempt_id": uuid::Uuid::new_v4().to_string(),
            "quality_preference": "auto",
            "subtitle_fidelity_preference": "preserve",
            "client_capabilities": capabilities::client_capabilities(),
            "client_playback_context": capabilities::playback_context(&os_version()),
        });
        if start_position > 0.0 {
            body["start_position"] = json!(start_position);
        }
        if req.file_id.is_some() {
            body["allow_alternate_versions"] = json!(false);
        }
        let decision = self.api(Method::POST, "/api/v2/playback/start", viewer, Some(&body)).await?;
        if decision.get("outcome").and_then(Value::as_str) != Some("playable") {
            let msg = decision.pointer("/terminal/message").and_then(Value::as_str).unwrap_or("this title cannot play on this device");
            bail!("{msg}");
        }
        Ok(Started {
            session_id: str_field(&decision, "session_id")?,
            installation_id,
            plan: decision.get("playback_plan").cloned().ok_or_else(|| anyhow!("decision has no plan"))?,
        })
    }

    /// Account credentials for a request to `target`: only on the server's
    /// own origin; a node or proxy URL relies on its signed `st` reference.
    async fn auth_headers(&self, target: &Url, viewer: &Viewer) -> Vec<(String, String)> {
        let mut h = vec![];
        if self.client.same_origin(target) {
            if let Some(t) = self.session.access_token().await {
                h.push(("Authorization".into(), format!("Bearer {t}")));
            }
            h.push(("X-Profile-Id".into(), viewer.profile_id.clone()));
            if let Some(pt) = viewer.profile_token.as_deref().filter(|t| !t.is_empty()) {
                h.push(("X-Profile-Token".into(), pt.into()));
            }
        }
        h
    }

    async fn stream_headers(&self, stream_url: &Url, plan: &Value, viewer: &Viewer) -> Vec<String> {
        let mut out: Vec<String> = plan
            .pointer("/stream/headers")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| format!("{k}: {v}"))).collect())
            .unwrap_or_default();
        out.extend(self.auth_headers(stream_url, viewer).await.into_iter().map(|(k, v)| format!("{k}: {v}")));
        out
    }

    /// Downloads the plan's sidecar subtitles to local files, so mpv never
    /// needs account credentials for them.
    async fn fetch_subtitles(&self, plan: &Value, viewer: &Viewer, dir: &Path) -> Vec<(PathBuf, String, String, bool)> {
        let selected = plan.pointer("/selected_tracks/subtitle/id").and_then(Value::as_str).map(str::to_owned);
        let inventory = plan.pointer("/subtitle/inventory").and_then(Value::as_array).cloned().unwrap_or_default();
        let http = reqwest::Client::new();
        let mut out = vec![];
        for (n, track) in inventory.iter().enumerate().take(40) {
            if track.get("delivery").and_then(Value::as_str) != Some("sidecar") {
                continue;
            }
            let Some(raw) = track.get("url").and_then(Value::as_str) else { continue };
            let Ok(url) = self.client.base.join(raw) else { continue };
            let mut rb = http.get(url.clone());
            for (k, v) in self.auth_headers(&url, viewer).await {
                rb = rb.header(k, v);
            }
            let Ok(res) = rb.send().await else { continue };
            if !res.status().is_success() {
                continue;
            }
            let Ok(bytes) = res.bytes().await else { continue };
            let ext = url.path().rsplit('.').next().filter(|e| e.len() <= 4).unwrap_or("srt");
            let path = dir.join(format!("sub{n}.{ext}"));
            if tokio::fs::write(&path, &bytes).await.is_err() {
                continue;
            }
            let label = track.get("label").and_then(Value::as_str).unwrap_or("").to_string();
            let lang = track.get("language").and_then(Value::as_str).unwrap_or("").to_string();
            let is_selected = selected.as_deref() == track.get("track_id").and_then(Value::as_str);
            out.push((path, label, lang, is_selected));
        }
        out
    }

    /// Plays the request to the end. Errors are for the page to show.
    pub async fn play(&self, req: PlayRequest, viewer: Viewer, title_hint: Option<String>) -> Result<()> {
        let (item, file_id) = self.resolve(&req, &viewer).await?;
        let started = self.start(&req, &viewer, &item, &file_id).await?;
        let result = self.run_mpv(&started, &viewer, &item, title_hint).await;
        if let Err(e) = &result {
            log::warn!("playback ended with an error: {e:#}");
        }
        result.map(|_| ())
    }

    async fn run_mpv(&self, s: &Started, viewer: &Viewer, item: &Value, title_hint: Option<String>) -> Result<()> {
        let plan = &s.plan;
        let raw = plan.pointer("/stream/url").and_then(Value::as_str).ok_or_else(|| anyhow!("plan has no stream URL"))?;
        let stream_url = self.client.base.join(raw)?;
        let player_start = plan.pointer("/timeline/player_start_seconds").and_then(Value::as_f64).unwrap_or(0.0);
        let offset = plan.pointer("/timeline/timeline_offset_seconds").and_then(Value::as_f64).unwrap_or(0.0);
        let delivery = plan.get("delivery").and_then(Value::as_str).unwrap_or("");

        let dir = self.temp.join(&s.session_id);
        tokio::fs::create_dir_all(&dir).await.ok();
        let subs = self.fetch_subtitles(plan, viewer, &dir).await;

        let pipe = format!(r"\\.\pipe\silo-mpv-{}", uuid::Uuid::new_v4().simple());
        let title = title_hint.unwrap_or_else(|| display_title(item));
        let mut cmd = Command::new(&self.mpv);
        cmd.arg("--no-config")
            .arg("--force-window=immediate")
            .arg("--fullscreen")
            .arg("--keep-open=no")
            .arg("--hwdec=auto-safe")
            .arg("--osc=yes")
            .arg("--input-default-bindings=yes")
            .arg(format!("--title={title} — Silolux"))
            .arg(format!("--input-ipc-server={pipe}"))
            .arg(format!("--start={player_start:.3}"));
        if delivery == "original_http" {
            if let Some(i) = plan.pointer("/selected_tracks/audio/index").and_then(Value::as_u64) {
                cmd.arg(format!("--aid={}", i + 1));
            }
        }
        for h in self.stream_headers(&stream_url, plan, viewer).await {
            cmd.arg(format!("--http-header-fields-append={h}"));
        }
        cmd.arg("--").arg(stream_url.as_str());
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().with_context(|| format!("could not start mpv at {}", self.mpv.display()))?;

        let ipc = connect_ipc(&pipe).await;
        let (mut position, mut paused) = (player_start, false);
        let mut sequence: i64 = 0;
        let mut ended_by_admin = false;

        if let Ok(pipe) = ipc {
            let (rd, mut wr) = tokio::io::split(pipe);
            for c in [
                json!({"command": ["observe_property", 1, "time-pos"]}),
                json!({"command": ["observe_property", 2, "pause"]}),
            ] {
                let _ = wr.write_all(format!("{c}\n").as_bytes()).await;
            }
            for (path, label, lang, selected) in &subs {
                let flag = if *selected { "select" } else { "auto" };
                let c = json!({"command": ["sub-add", path.to_string_lossy(), flag, label, lang]});
                let _ = wr.write_all(format!("{c}\n").as_bytes()).await;
            }
            let mut lines = BufReader::new(rd).lines();
            let mut tick = tokio::time::interval(PROGRESS_EVERY);
            tick.tick().await;
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        let Ok(Some(line)) = line else { break };
                        let Ok(ev) = serde_json::from_str::<Value>(&line) else { continue };
                        if ev.get("event").and_then(Value::as_str) == Some("property-change") {
                            match ev.get("name").and_then(Value::as_str) {
                                Some("time-pos") => if let Some(p) = ev.get("data").and_then(Value::as_f64) { position = p },
                                Some("pause") => if let Some(p) = ev.get("data").and_then(Value::as_bool) {
                                    if p != paused { paused = p; tick.reset_immediately(); }
                                },
                                _ => {}
                            }
                        }
                    }
                    _ = tick.tick() => {
                        sequence += 1;
                        let body = json!({ "installation_id": s.installation_id, "sequence": sequence,
                                           "position": (position + offset).max(0.0), "is_paused": paused });
                        let path = format!("/api/v2/playback/{}/progress", s.session_id);
                        match self.api(Method::POST, &path, viewer, Some(&body)).await {
                            Err(e) if is_status(&e, 410) => { ended_by_admin = true; break; }
                            Err(e) => log::warn!("progress report failed: {e:#}"),
                            Ok(_) => {}
                        }
                        // New connections (seeks, reconnects) use the current token.
                        let headers = self.stream_headers(&stream_url, plan, viewer).await;
                        let c = json!({"command": ["set_property", "http-header-fields", headers]});
                        let _ = wr.write_all(format!("{c}\n").as_bytes()).await;
                    }
                }
            }
        } else {
            log::warn!("mpv IPC unavailable; progress will only be reported at stop");
        }
        if ended_by_admin {
            let _ = child.start_kill();
        }
        let _ = child.wait().await;
        tokio::fs::remove_dir_all(&dir).await.ok();
        if ended_by_admin {
            bail!("an administrator ended this playback session");
        }

        sequence += 1;
        let stop = json!({ "installation_id": s.installation_id, "stop_id": uuid::Uuid::new_v4().to_string(),
                           "sequence": sequence, "position": (position + offset).max(0.0), "is_paused": true });
        let path = format!("/api/v2/playback/{}", s.session_id);
        for attempt in 0..3 {
            match self.api(Method::DELETE, &path, viewer, Some(&stop)).await {
                Ok(_) => break,
                Err(e) if is_status(&e, 404) || is_status(&e, 410) => break,
                Err(e) if attempt == 2 => return Err(e.context("reporting the stop")),
                Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
        Ok(())
    }
}

async fn connect_ipc(name: &str) -> Result<NamedPipeClient> {
    for _ in 0..100 {
        if let Ok(c) = ClientOptions::new().open(name) {
            return Ok(c);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("mpv did not open its IPC pipe")
}

fn display_title(item: &Value) -> String {
    let title = item.get("title").and_then(Value::as_str).unwrap_or("Silolux");
    match (item.get("series_title").and_then(Value::as_str), item.get("season_number"), item.get("episode_number")) {
        (Some(series), Some(s), Some(e)) => format!("{series} S{s}E{e} · {title}"),
        _ => title.to_string(),
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn os_version() -> String {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split("Version ").nth(1).map(|v| v.trim().trim_end_matches(']').to_string()))
        .unwrap_or_default()
}
