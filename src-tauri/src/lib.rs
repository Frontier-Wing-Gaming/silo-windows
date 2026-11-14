mod api;
mod capabilities;
mod player;
mod session;

use api::{Client, Viewer};
use player::{PlayRequest, Player};
use serde::{Deserialize, Serialize};
use serde_json::json;
use session::Session;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::ipc::CapabilityBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use url::Url;

const BRIDGE_JS: &str = include_str!("bridge.js");
const STATE_EVENT: &str = "silo-native://state";

#[derive(Default, Serialize, Deserialize, Clone)]
struct Config {
    server_url: Option<String>,
    device_id: String,
    /// Overrides the bundled mpv, e.g. `C:\Tools\mpv\mpv.exe`.
    mpv_path: Option<String>,
    /// Plays in the browser engine instead of mpv when false.
    #[serde(default = "yes")]
    native_player: bool,
}

fn yes() -> bool {
    true
}

struct AppState {
    dir: PathBuf,
    config: Mutex<Config>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    capabilities: Mutex<Vec<String>>,
    playing: Mutex<bool>,
}

impl AppState {
    fn save(&self) {
        let cfg = self.config.lock().unwrap().clone();
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join("config.json"), serde_json::to_vec_pretty(&cfg).unwrap_or_default());
    }

    fn session_for(&self, base: &Url) -> Arc<Session> {
        let key: String = base.origin().ascii_serialization().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        self.sessions
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Session::load(self.dir.join("sessions").join(format!("{key}.json")))))
            .clone()
    }
}

fn normalize_server(input: &str) -> Result<Url, String> {
    let trimmed = input.trim().trim_end_matches('/');
    let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("http://{trimmed}") };
    let url = Url::parse(&format!("{with_scheme}/")).map_err(|e| format!("That address is not valid: {e}"))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        _ => Err("Use an http:// or https:// address.".into()),
    }
}

/// Lets the server's own pages reach the bridge commands. Only that origin
/// gets the capability, so no other site the window visits can call them.
fn allow_origin(app: &AppHandle, url: &Url) {
    let state = app.state::<AppState>();
    let origin = url.origin().ascii_serialization();
    let mut granted = state.capabilities.lock().unwrap();
    if granted.contains(&origin) {
        return;
    }
    let cap = CapabilityBuilder::new(format!("server-{}", granted.len()))
        .remote(format!("{origin}/*"))
        .window("main")
        .permission("core:event:default");
    match app.add_capability(cap) {
        Ok(()) => granted.push(origin),
        Err(e) => log::error!("could not grant the server origin: {e}"),
    }
}

fn mpv_path(app: &AppHandle, cfg: &Config) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![];
    if let Some(p) = &cfg.mpv_path {
        candidates.push(p.into());
    }
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join("mpv").join("mpv.exe"));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        candidates.push(dir.join("mpv").join("mpv.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("mpv.exe")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

// ---- commands -------------------------------------------------------------

#[tauri::command]
fn launcher_state(state: tauri::State<AppState>) -> serde_json::Value {
    let cfg = state.config.lock().unwrap();
    json!({ "server_url": cfg.server_url, "version": api::APP_VERSION })
}

#[tauri::command]
async fn connect(app: AppHandle, address: String) -> Result<String, String> {
    let url = normalize_server(&address)?;
    let device_id = app.state::<AppState>().config.lock().unwrap().device_id.clone();
    let client = Client::new(url.clone(), device_id).map_err(|e| e.to_string())?;
    let info = client.system_info().await.map_err(|e| format!("No Silo server answered at {url}: {e:#}"))?;
    if info.get("api_major").and_then(|v| v.as_i64()) != Some(2) {
        return Err("This server speaks a Silo API this app does not support. Update the server or the app.".into());
    }
    {
        let state = app.state::<AppState>();
        state.config.lock().unwrap().server_url = Some(url.to_string());
        state.save();
    }
    allow_origin(&app, &url);
    if let Some(w) = app.get_webview_window("main") {
        w.navigate(url.clone()).map_err(|e| e.to_string())?;
    }
    Ok(url.to_string())
}

#[tauri::command]
fn change_server(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.config.lock().unwrap().server_url = None;
    state.save();
    if let Some(w) = app.get_webview_window("main") {
        // Where Tauri serves the bundled launcher on Windows (WebView2).
        let launcher = Url::parse("http://tauri.localhost/index.html").expect("static URL");
        w.navigate(launcher).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct NativePlay {
    request: PlayRequest,
    viewer: Viewer,
    /// The web app's current bearer, used only to pair the player's own
    /// login session when it has none for this account.
    bearer: String,
    title: Option<String>,
}

/// Whether the page should hand playback to mpv. False when mpv is missing
/// or the user turned the native player off; the web player then runs.
#[tauri::command]
fn native_available(app: AppHandle) -> bool {
    let cfg = app.state::<AppState>().config.lock().unwrap().clone();
    cfg.native_player && mpv_path(&app, &cfg).is_some()
}

#[tauri::command]
async fn native_play(app: AppHandle, webview: tauri::Webview, args: NativePlay) -> Result<(), String> {
    let page = webview.url().map_err(|e| e.to_string())?;
    let (base, mpv, temp, session) = {
        let state = app.state::<AppState>();
        let cfg = state.config.lock().unwrap().clone();
        let base = cfg.server_url.as_deref().and_then(|s| Url::parse(s).ok()).ok_or("No server is configured.")?;
        // Only the configured server's own pages may start playback.
        if page.origin() != base.origin() {
            return Err("Playback was requested from another site.".into());
        }
        let mpv = mpv_path(&app, &cfg).ok_or("mpv was not found. Reinstall the app or set mpv_path.")?;
        let temp = std::env::temp_dir().join("silolux-windows");
        let session = state.session_for(&base);
        {
            let mut playing = state.playing.lock().unwrap();
            if *playing {
                return Err("Something is already playing.".into());
            }
            *playing = true;
        }
        (base, mpv, temp, session)
    };
    let device_id = app.state::<AppState>().config.lock().unwrap().device_id.clone();
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |state: &str, message: Option<String>| {
            let _ = task_app.emit(STATE_EVENT, json!({ "state": state, "message": message }));
        };
        emit("starting", None);
        let result: anyhow::Result<()> = async {
            let client = Client::new(base, device_id)?;
            session.ensure(&client, &args.bearer).await?;
            let player = Player { client, session: session.clone(), mpv, temp };
            emit("playing", None);
            player.play(args.request, args.viewer, args.title).await
        }
        .await;
        *task_app.state::<AppState>().playing.lock().unwrap() = false;
        match result {
            Ok(()) => emit("ended", None),
            Err(e) => emit("error", Some(format!("{e:#}"))),
        }
    });
    Ok(())
}

// ---- startup --------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![launcher_state, connect, change_server, native_available, native_play])
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let mut cfg: Config = std::fs::read(dir.join("config.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Config { native_player: true, ..Default::default() });
            if cfg.device_id.is_empty() {
                cfg.device_id = format!("silolux-windows-{}", uuid::Uuid::new_v4());
            }
            let server = cfg.server_url.as_deref().and_then(|s| Url::parse(s).ok());
            app.manage(AppState {
                dir,
                config: Mutex::new(cfg),
                sessions: Mutex::new(HashMap::new()),
                capabilities: Mutex::new(vec![]),
                playing: Mutex::new(false),
            });
            app.state::<AppState>().save();

            let start = match &server {
                Some(url) => {
                    allow_origin(app.handle(), url);
                    WebviewUrl::External(url.clone())
                }
                None => WebviewUrl::App("index.html".into()),
            };
            WebviewWindowBuilder::new(app, "main", start)
                .title("Silolux")
                .inner_size(1440.0, 900.0)
                .min_inner_size(800.0, 560.0)
                .initialization_script(BRIDGE_JS)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Silolux");
}
