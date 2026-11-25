# nsis-duilib-ui

A native, themeable DuiLib UI plugin for modern NSIS installers. It provides a
Win10/Win11-style installer window without embedding Chromium or WebView2 and
falls back to the original NSIS window when initialization fails.

The plugin targets the **x86 Unicode NSIS host** used by electron-builder. This
is independent of the packaged application architecture: the Electron payload
can remain x64 or ARM64 while the NSIS plugin remains Win32/x86.

## What is included

- `nsis-duilib-ui.dll`: NSIS plugin with a stable, small command surface.
- `ThemePreview.exe`: interactive theme preview and deterministic PNG capture.
- `themes/onekey-modern`: schema-versioned sample theme in English and Chinese.
- `demo`: interactive and silent smoke-test NSIS installers.
- Focused tests for versions, theme validation, and template rendering.

The sample theme uses the real OneKey vector geometry, a native GDI+ light
backdrop, and an animated progress control with eased value changes, shimmer,
and a live endpoint. Its welcome, installing, and finish pages share one
900x560 logical canvas. The renderer scales that canvas for the monitor DPI and
clamps the effective scale to the monitor work area on smaller displays.

The current runtime baseline is Windows 10. The binaries are Per-Monitor-V2 DPI
aware, use the static MSVC runtime, and depend only on Windows system DLLs.

## Build prerequisites

- Windows 10 or later
- Visual Studio 2022 with **Desktop development with C++**
- CMake 3.24 or later (the script can use the Visual Studio bundled copy)
- NSIS 3.x for the demo; `build-demo.ps1` also recognizes electron-builder's
  local NSIS cache

## Build and test

```powershell
./scripts/build.ps1 -Configuration Debug
./scripts/build.ps1 -Configuration Release
./scripts/build-demo.ps1 -Configuration Release
```

`build.ps1` selects the x64-hosted MSVC compiler targeting Win32, then invokes
CMake and Ninja. It runs the native test suite after compiling. Release output
is placed in:

```text
out/build/windows-x86-ninja/Release/
  nsis-duilib-ui.dll
  ThemePreview.exe
```

Release builds use `/MT`, so installing a separate Visual C++ Redistributable
is not required.

## Preview a theme

```powershell
./out/build/windows-x86-ninja/Release/ThemePreview.exe `
  --theme ./themes/onekey-modern `
  --locale zh-CN `
  --page installing `
  --version 6.6.0
```

Valid sample pages are `welcome`, `options`, `installing`, `finish`,
`uninstallWelcome`, `uninstalling`, `uninstallFinish`, `uninstallError`, and `error`. Add
`--screenshot output.png`
to render a PNG and exit. This makes theme review possible without rebuilding
an Electron application or running an installer. Use `--progress 18` to capture
a deterministic progress state.

Use `--event-result output.txt` for interactive automation. The preview waits
for a real UI event, writes `primary`, `cancel`, `closed`, or the selected scope
event to the file, and exits. This supports deterministic button and close-flow
acceptance tests without installing an application.

`build-demo.ps1` also produces safe interactive acceptance programs for the
normal installation, installation failure/retry, uninstall success, and
uninstall failure/retry flows. The uninstall demos create and remove only one
temporary QA payload inside NSIS' plug-in directory; they do not inspect or
modify an installed OneKey application.

## Theme contract

Each theme is a self-contained directory. `theme.json` selects the schema,
engine compatibility, window geometry, page templates, tokens, localized
strings, and optional content cards:

```text
theme.json
tokens.json
content.json
locales/en-US.json
locales/zh-CN.json
pages/*.xml
assets/*                  # optional local images
```

`schemaVersion` currently equals `1`. A theme can be replaced without changing
or recompiling the DLL as long as it follows that schema. Colors and typography
belong in `tokens.json`; product copy belongs in locale files; release
highlights or promotional cards belong in `content.json`; structure belongs in
the page XML files.

The loader rejects absolute paths, directory traversal, invalid colors,
unsupported schema/engine versions, non-HTTPS action URLs, missing files, and
invalid window or font ranges. Text inserted into XML templates is escaped.

## NSIS API

All commands return one value on the NSIS stack. Commands that mutate the UI
return `ok` or `error:<message>`.

| Command | Stack arguments | Result |
| --- | --- | --- |
| `GetVersion` | none | engine semantic version |
| `Init` | theme root, locale, app version | `ok` or error |
| `Hide` | none | `ok` or error; keeps the native host concealed |
| `ConcealHost` | none | `ok` or error; rebinds and synchronously conceals the current native host |
| `Show` | none | `ok` or error |
| `SetPage` | page name | `ok` or error |
| `SetProgress` | integer 0-100 | `ok` or error; values are monotonic |
| `ResetProgress` | integer 0-100 | `ok` or error; starts a new attempt |
| `SetInstallDirectory` | path | `ok` or error |
| `SetPerUserInstallDirectory` | path | `ok` or error |
| `SetPerMachineInstallDirectory` | path | `ok` or error |
| `SetStatus` | text | `ok` or error |
| `SetControlText` | control name, text | `ok` or error |
| `SetVersion` | text | `ok` or error |
| `PollEvent` | none | `none`, primary/browse scope variants, scope choice, `cancel`, or `closed` |
| `WaitForEvent` | none | primary/browse scope variants, scope choice, `cancel`, or `closed` |
| `Shutdown` | none | `ok` |
| `ShutdownHidden` | none | `ok`; destroys the modern window but keeps the native host concealed |

After `Init`, the plugin mirrors the largest native NSIS progress control at
80 ms intervals. The first explicit `SetProgress` call switches the current
session to manual progress so native polling cannot overwrite caller-provided
values. Native and manual samples use a high-water mark, so an installation
attempt never moves backwards. Call `ResetProgress` only when a new attempt
begins. Closing or cancelling the custom window forwards the cancel request to
NSIS, so installation remains owned by the original engine. Integrations that
actively call `PollEvent` own cancellation themselves; native forwarding pauses
while that polling loop is active so the event can be handled deterministically.

Only hide `$HWNDPARENT` after both `Init` and `Show` returned `ok`. On any error,
call `Shutdown` and leave or restore `$HWNDPARENT`; this is the compatibility
fallback and is demonstrated in `demo/demo.nsi`.

## electron-builder integration

1. Copy the Release DLL to a private build-resource plugin directory, for
   example `build/nsis-duilib-ui/plugin/x86-unicode/nsis-duilib-ui.dll`.
2. Copy a complete theme beside it, for example
   `build/nsis-duilib-ui/onekey-modern`.
3. Preserve this project's `LICENSE` and the DuiLib, JSON, and NSIS notices
   listed in `THIRD_PARTY_NOTICES.md` in the distributed installer.
4. Add an `installer.nsh` through `nsis.include`.
5. In that include, register the private directory with
   `!addplugindir /x86-unicode`, extract the theme to `$PLUGINSDIR`, initialize
   the plugin, and hide the native parent only on success.

Use a private plugin directory instead of the special root-level
`build/x86-unicode`. electron-builder automatically scans that root directory;
registering it again from `installer.nsh` can make makensis report a duplicate
plugin command, and its asynchronously assembled header does not provide a
reliable relative ordering for an early custom include.

The same `installer.nsh` can define electron-builder's `customInstall`,
`customUnInstall`, and custom-page hooks for installer and uninstaller flows.
The packaged Electron architecture does not select the plugin architecture;
the NSIS process that loads the DLL does.

## DuiLib base and licenses

The renderer is based on the MIT-licensed DuiLib Ultimate source pinned in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). DuiLib and the JSON parser
are linked statically. Their exact revisions and notices are recorded in that
file. This project itself is licensed under MIT; no nsNiuniuSkin source or
assets are included.
