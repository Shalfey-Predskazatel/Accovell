param(
    [string]$Makensis = "",
    [string]$OutputPath = "",
    [string]$AppBin = "",
    [string]$WebView2Loader = ""
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Makensis)) {
    $candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
        (Join-Path $env:LOCALAPPDATA "tauri\NSIS\makensis.exe"),
        (Join-Path $env:LOCALAPPDATA "electron-builder\Cache\nsis-3.0.4.1\nsis-3.0.4.1-1mx3n\makensis.exe")
    )
    $Makensis = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}

if ([string]::IsNullOrWhiteSpace($Makensis) -or -not (Test-Path -LiteralPath $Makensis)) {
    throw "NSIS не найден. Установите NSIS 3.13+ или передайте -Makensis путь к makensis.exe."
}

$installerDir = $PSScriptRoot
$root = Split-Path -Parent $installerDir
$releaseDir = Join-Path $root 'src-tauri\target\release'
$appBinary = if ([string]::IsNullOrWhiteSpace($AppBin)) {
    Join-Path $releaseDir 'codex-accounts.exe'
} elseif ([System.IO.Path]::IsPathRooted($AppBin)) {
    [System.IO.Path]::GetFullPath($AppBin)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $root $AppBin))
}
$webviewLoaderPath = if ([string]::IsNullOrWhiteSpace($WebView2Loader)) {
    Join-Path $releaseDir 'WebView2Loader.dll'
} elseif ([System.IO.Path]::IsPathRooted($WebView2Loader)) {
    [System.IO.Path]::GetFullPath($WebView2Loader)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $root $WebView2Loader))
}
if (-not (Test-Path -LiteralPath $webviewLoaderPath) -and [string]::IsNullOrWhiteSpace($WebView2Loader)) {
    $webviewBuildRoot = Join-Path $releaseDir 'build'
    $webviewLoaderSource = Get-ChildItem -LiteralPath $webviewBuildRoot -Directory -Filter 'webview2-com-sys-*' -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName 'out\x64\WebView2Loader.dll' } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Select-Object -First 1
    if ($webviewLoaderSource) {
        Copy-Item -LiteralPath $webviewLoaderSource -Destination $webviewLoaderPath -Force
    }
}
$requiredAssets = @(
    $appBinary,
    $webviewLoaderPath,
    (Join-Path $root 'src-tauri\icons\icon.ico'),
    (Join-Path $installerDir 'assets\MicrosoftEdgeWebView2Setup.exe'),
    (Join-Path $installerDir 'third_party\IBMPlexSans-LICENSE.txt'),
    (Join-Path $installerDir 'third_party\nsis-duilib-ui\LICENSE'),
    (Join-Path $installerDir 'third_party\nsis-duilib-ui\THIRD_PARTY_NOTICES.md'),
    (Join-Path $installerDir 'third_party\nsis-duilib-ui\third_party\duilib\LICENSE'),
    (Join-Path $installerDir 'third_party\nsis-duilib-ui\third_party\nlohmann\LICENSE.MIT'),
    (Join-Path $installerDir 'third_party\nsis-duilib-ui\third_party\nsis\LICENSE')
)
foreach ($asset in $requiredAssets) {
    if (-not (Test-Path -LiteralPath $asset)) {
        throw "Файл установщика не найден: $asset"
    }
}

Push-Location $installerDir
try {
    $makensisArgs = @('/INPUTCHARSET', 'UTF8', '/V2')
    if (-not [string]::IsNullOrWhiteSpace($OutputPath)) {
        $resolvedOutput = if ([System.IO.Path]::IsPathRooted($OutputPath)) {
            [System.IO.Path]::GetFullPath($OutputPath)
        } else {
            [System.IO.Path]::GetFullPath((Join-Path $root $OutputPath))
        }
        $outputsRoot = [System.IO.Path]::GetFullPath((Join-Path $root 'outputs'))
        if (-not $resolvedOutput.StartsWith($outputsRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Установщик можно сохранять только в папку outputs проекта: $outputsRoot"
        }
        $makensisArgs += "/DINSTALLER_OUTFILE=$resolvedOutput"
    }
    $makensisArgs += "/DAPP_BIN=$appBinary"
    $makensisArgs += "/DAPP_WEBVIEW2_LOADER=$webviewLoaderPath"
    $makensisArgs += '.\CodexAccounts.nsi'
    & $Makensis @makensisArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Сборка установщика завершилась с кодом $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}
