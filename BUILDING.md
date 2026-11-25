# Building Accovell

Accovell is a Windows x64 desktop application built with Tauri 2, React, TypeScript, and Rust.

## Prerequisites

- Node.js and npm.
- Rust with the MSVC Windows target.
- Visual Studio Build Tools with the C++ desktop workload and Windows SDK.
- NSIS 3.13 or newer to create the branded installer.
- WebView2 Runtime for running the built application.

Install the locked JavaScript packages from the repository root:

    npm ci

Build the Windows x64 application:

    npm run tauri -- build --no-bundle --target x86_64-pc-windows-msvc -- --locked

The release executable and WebView2 loader are produced under the Tauri target directory. Pass their full paths to the installer script:

    New-Item -ItemType Directory -Path ./outputs -Force
    ./installer/build.ps1 -AppBin PATH_TO/codex-accounts.exe -WebView2Loader PATH_TO/WebView2Loader.dll -OutputPath outputs/Accovell-0.1.0-Setup-x64.exe

The installer script requires the bundled installer assets and writes its output under the repository's outputs directory.

## Dependency caches

The repository does not include offline dependency caches. The local r85 handoff bundle contains a separate offline build cache and NVIDIA SDK package; the public source repository does not. Supply the toolchain and licensed NVIDIA components required by the source before attempting a reproducible native build. Observe the license terms shipped with every third-party component.

The application was not functionally retested as part of this publication.
