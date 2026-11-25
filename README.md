![Accovell — local account manager](assets/social-preview.png)

# Accovell

**English** · [Русский](README.ru.md)

**A local Windows dashboard for Codex profiles.** Free to download.  
Product identity: **Accovell** · by **Shalfey Predskazatel**

[Download the latest release](https://github.com/Shalfey-Predskazatel/Accovell/releases/latest) · [Source code](https://github.com/Shalfey-Predskazatel/Accovell) · [Release notes](RELEASE-NOTES.md) · [Report an issue](https://github.com/Shalfey-Predskazatel/Accovell/issues)

Accovell brings Codex profile selection, usage information, shared settings, and a local request route together in one Windows app. Sign-in and quota information come from the official Codex CLI installed on your PC.

## What it does

- Add and manage multiple local Codex profiles.
- View the usage and reset information returned by Codex CLI.
- Choose a profile manually, or enable automatic selection for future requests after Codex explicitly reports an exhausted quota. The stopped request is not replayed, and existing response chains stay with the profile that started them.
- Change shared model, reasoning, approval, sandbox, and instruction settings.
- Use the system tray, refresh schedule, startup option, sounds, and app themes.
- Create user-selected backups. An export without a password contains OAuth credentials in plain text; protect it and never upload it to this repository.

## Downloads

- [Windows x64 installer](https://github.com/Shalfey-Predskazatel/Accovell/releases/latest/download/Accovell-0.1.0-Setup-x64.exe)
- [Windows x64 portable build](https://github.com/Shalfey-Predskazatel/Accovell/releases/latest/download/Accovell-0.1.0-Portable.zip)

The installer uses Microsoft's WebView2 bootstrapper if the runtime is missing and needs an internet connection for that step. The portable folder contains the app and WebView2 loader; Windows WebView2 Runtime must already be installed. See the release notes for the exact build contents.

## Requirements

- Windows 10 or 11, x64.
- The official Codex CLI installed and signed in for profile and usage operations.
- A compatible WebView2 Runtime.

Accovell stores its profiles on the local PC and uses Windows DPAPI for credential protection. Do not publish profile exports, app data, or personal Codex configuration.

## Source and build

The repository includes the application source and installer source. See [BUILDING.md](BUILDING.md). Offline dependency caches from the private handoff bundle are not included.

The source is public for inspection. No reuse license is specified for this project. Third-party components keep their own license terms.

## Product name

Accovell is the product brand. The supplied r85 executable and installer still show **Codex Accounts** in some Windows UI labels.

## Independent project

Accovell is an independent community project and is not affiliated with or endorsed by OpenAI. Codex and OpenAI are referenced only to describe compatibility.
