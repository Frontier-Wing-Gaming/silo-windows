# Silolux for Windows: Privacy Policy

Effective 13 November 2026.

Silolux for Windows ("the app") is an independent client for Silo, a media server you or
someone you know runs. It is published by Frontier Wing Gaming and is not affiliated with the
Silo project.

## The short version

The app talks to one place: the Silo server whose address you enter. Frontier Wing Gaming runs
no servers for the app and receives nothing from it: no analytics, no advertising, no tracking,
no crash reports.

## How the app works

The app opens your Silo server's own web interface in a window and plays video in mpv, a
separate media player installed with it. Everything you do in the web interface (signing in,
browsing, settings) happens between that page and your server, under your server's rules.

## What the app itself sends to your Silo server

When you play something, the app:

- Signs the mpv player in to your server as a separate device, approved by your web sign-in.
- Sends which profile is watching, what you play, and how far you got, so the server can keep
  your place.
- Sends a device identifier the app makes up at random, a device name made of the app's name
  and your PC's computer name (for example "Silolux for Windows (DESKTOP-1234)"), the app name
  and version, the Windows version, and which video and audio formats mpv can play. The server
  uses these to list the PC among your signed-in devices and to choose a stream.

What your server does with this is up to whoever runs it. Silo's own documentation and that
person's policies apply there, not this one.

## What the app keeps on your PC

- The server address and app settings, in `%APPDATA%\com.frontierwinggaming.silolux.windows`.
- The player's sign-in tokens for each server, in the `sessions` folder there, so playback
  works without signing in again. Delete that folder to sign the player out.
- Your web sign-in is stored by Microsoft Edge WebView2, the browser component the window uses,
  like any website's sign-in.
- Subtitle files for the title playing, in a temporary folder, while it plays.

Uninstalling the app removes the app; the `%APPDATA%` folder above can be deleted by hand.

## Images and video

Video, artwork and subtitles come from your Silo server, or from a streaming address that
server hands the app. The player's sign-in token is only ever sent to your own server's address.

## Microsoft

The window is Microsoft Edge WebView2, which Microsoft updates and which can send Microsoft
diagnostic data under Windows' settings. Microsoft's privacy statement covers that:
https://privacy.microsoft.com/privacystatement. The app adds nothing to it.

## Children

The app has no accounts of its own and collects nothing for Frontier Wing Gaming. Profile
restrictions, such as age limits on a child's profile, are set on the Silo server.

## Changes and contact

Changes to this policy are published in this file, and its history is on GitHub. Questions:
open an issue at https://github.com/Frontier-Wing-Gaming/silo-windows/issues.
