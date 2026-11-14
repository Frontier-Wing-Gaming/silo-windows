# Silolux for Windows

Silolux is a Windows desktop client for [Silo](https://github.com/Silo-Server/silo-server). It shows the
server's own web app in a WebView2 window and plays video in **mpv**, so most files direct-play
(HEVC, AV1, TrueHD, DTS, ASS subtitles, HDR tone-mapped to SDR) instead of being transcoded for a
browser.

Status: early prototype, `0.1.0`.

Silolux is an independent project by Frontier Wing Gaming. It is not made, endorsed or supported
by Silo Media L.L.C.; "Silo" is their trademark and is used here only to say what this app connects to.

## How it works

- **Launcher.** The first run asks for the server address and checks `GET /api/v2/system/info`
  (`api_major` must be 2). After that the window opens the server directly. Ctrl+Shift+O goes
  back to the launcher.
- **Bridge** (`src-tauri/src/bridge.js`). This script is injected into the server's pages. It
  takes plain `/watch/:id` navigations (Play buttons and cards) and hands them to the native
  player. Watch parties (`room_id`), shuffles, audiobooks and the ebook reader stay in the web
  app. Only the configured server's origin gets IPC access, through a runtime capability.
- **Own login session.** The player never shares the web app's refresh token, because two holders
  of one rotating token would sign each other out. The first time you play something, the app
  opens a device sign-in (`/auth/device/start`), approves it with the web app's current bearer
  token (`/auth/device/approve`), and collects its own tokens. They are stored per server under
  `%APPDATA%\com.frontierwinggaming.silolux.windows\sessions\`. Pairing runs again if you switch accounts.
- **Playback** (`src-tauri/src/player.rs`). Playback uses protocol v3 throughout:
  `/playback/capabilities` gives the `installation_id`; `/playback/start` sends mpv's capabilities
  (`capabilities.rs`) and plays the returned plan. Progress is reported every 10 s and on pause
  through sequenced `/progress` calls, with a stop when mpv closes. Positions use the plan's
  `timeline_offset_seconds`. Credentials go only to the server's own origin. Node and proxy URLs
  rely on their signed `st` reference. Sidecar subtitles are downloaded by the app and passed to
  mpv as local files.

## Build

Requirements: Rust (stable), Node 18+, and the WebView2 runtime (included in Windows 11).

```powershell
npm install
powershell -ExecutionPolicy Bypass -File scripts/fetch-mpv.ps1   # puts mpv.exe in ./mpv
npm run dev      # run against your server
npm run build    # NSIS + MSI installers in src-tauri/target/release/bundle
```

The app looks for mpv in this order: `mpv_path` in `config.json`, then the bundled `mpv\` folder,
then `PATH`. With no mpv, Play falls through to the web player.

## Known gaps

- mpv opens in its own fullscreen window. The next step is embedding it with `--wid`.
- No replan on failure yet. mpv rarely needs one, but quality changes and track changes made in
  mpv are not reported to the server.
- PIN profiles use the web app's `X-Profile-Token`. If the server ties that token to the web
  session, playback returns 403.
- Interception depends on the web app's URLs (`/watch/:id`, `fileId`, `libraryId`, `restart`).
  A small upstream hook in `WatchRoute` (for example `window.siloNativePlayer?.play(request)`)
  would make this robust. The bridge's `pushState` fallback briefly mounts the web player and
  holds its start request.
- Not tested end to end against a live server yet.

## License

AGPL-3.0-or-later, matching Silo. mpv builds are GPL; ship their license with the installer.
