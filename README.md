# Silolux for Windows

Silolux is a Windows desktop client for [Silo](https://github.com/Silo-Server/silo-server). It shows the
server's own web app in a WebView2 window and plays video in **mpv**, so most files direct-play
(HEVC, AV1, TrueHD, DTS, ASS subtitles, HDR tone-mapped to SDR) instead of being transcoded for a
browser.

Status: early prototype, `0.2.3`.

Silolux is an independent project by Frontier Wing Gaming. It is not made, endorsed or supported
by Silo Media L.L.C.; "Silo" is their trademark and is used here only to say what this app connects to.

## How it works

- **Launcher.** The first run asks for the server address and checks `GET /api/v2/system/info`
  (`api_major` must be 2). After that, each launch makes the same check on the saved server
  and moves straight on to it if it answers; if it doesn't, the launcher stays up with the
  error and the address, so another server can be picked. Mid-session, a server page that
  fails to load (server down, network gone, a proxy's 5xx) leads to a check of the server, and
  if that fails too, back to the launcher with the reason: WebView2's own error page can't run
  the bridge, so it would leave no way out. Ctrl+Shift+O goes back to the launcher.
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
  mpv as local files. Play from start always sends `start_position` 0 (the server would
  otherwise resume).
- **Recovery.** mpv runs with `--idle=yes`, so a stream it can't open leaves it waiting instead
  of closing. The app then asks for a `failure_recovery` replan at the current position and loads
  the new plan into the same window, up to four plans per title.
- **Smooth film on a fixed refresh rate.** A PC can't switch its refresh rate per film, so 24 fps
  on a 60 Hz screen judders. mpv runs with `--video-sync=display-resample --interpolation=yes
  --tscale=oversample`, which blends only the frames that straddle a refresh.

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

`cargo test` (in `src-tauri`) runs the unit tests: server address parsing, the rule that account
credentials only go to the server's own origin, and the playback capabilities the app declares.

## Releases

Set the version in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`, then push a tag such as
`v0.2.0`. The Release workflow fetches mpv, builds the MSI and NSIS installers, and attaches them
to a draft GitHub release for you to review and publish. The installers are not code-signed, so
Windows SmartScreen warns on first run.

## Known gaps

- mpv opens in its own fullscreen window. The next step is embedding it with `--wid`.
- Audio and subtitle tracks changed in mpv are not reported to the server, so the next title
  starts with the server's default again. mpv has no quality menu; the server's *auto* quality
  applies.
- PIN profiles use the web app's `X-Profile-Token`. If the server ties that token to the web
  session, playback returns 403.
- Interception depends on the web app's URLs (`/watch/:id`, `fileId`, `libraryId`, `restart`).
  A small upstream hook in `WatchRoute` (for example `window.siloNativePlayer?.play(request)`)
  would make this robust. The bridge's `pushState` fallback briefly mounts the web player and
  holds its start request.
- Tested against a local Docker Silo server and a remote one.

## Privacy

The app sends data only to the Silo server you enter; see [PRIVACY.md](PRIVACY.md).

## License

MIT; see [LICENSE](LICENSE). mpv is not part of this repository and has its own GPL license.
`scripts/fetch-mpv.ps1` saves that license (`mpv/LICENSE.GPL.txt`) and the source location of the
exact build (`mpv/SOURCE.txt`) next to mpv, so installers that bundle it carry both.
