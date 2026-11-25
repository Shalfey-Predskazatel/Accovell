[CmdletBinding()]
param(
  [ValidateSet('Debug', 'Release')]
  [string]$Configuration = 'Release'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

& (Join-Path $PSScriptRoot 'build.ps1') -Configuration $Configuration
if ($LASTEXITCODE -ne 0) {
  throw "Native build failed with exit code $LASTEXITCODE"
}

$pluginDll = Join-Path $repoRoot "out\build\windows-x86-ninja\$Configuration\nsis-duilib-ui.dll"
if (-not (Test-Path -LiteralPath $pluginDll)) {
  throw "Plugin DLL was not produced: $pluginDll"
}

$nsisCache = Join-Path $env:LOCALAPPDATA 'electron-builder\Cache\nsis'
$makeNsis = Get-ChildItem -Path $nsisCache -Recurse -Filter makensis.exe -File |
  Sort-Object FullName |
  Select-Object -First 1 -ExpandProperty FullName
if (-not $makeNsis) {
  throw 'makensis.exe was not found in the electron-builder cache'
}

$outputDir = Join-Path $repoRoot 'out\demo'
$pluginDir = Join-Path $outputDir 'plugins\x86-unicode'
New-Item -ItemType Directory -Force $pluginDir | Out-Null
Copy-Item -LiteralPath $pluginDll -Destination (Join-Path $pluginDir 'nsis-duilib-ui.dll') -Force

$themeDir = Join-Path $repoRoot 'themes\onekey-modern'
$defines = @(
  "/DPLUGIN_DIR=$pluginDir",
  "/DTHEME_DIR=$themeDir",
  "/DOUTPUT_DIR=$outputDir"
)

foreach ($scenario in @(
  @{ Name = 'setup'; Defines = @('/DRECORDING_DELAY_MS=15000') },
  @{ Name = 'install failure'; Defines = @('/DDEMO_FAILURE', '/DRECORDING_DELAY_MS=15000') },
  @{ Name = 'uninstall'; Defines = @('/DDEMO_UNINSTALL', '/DRECORDING_DELAY_MS=5000') },
  @{ Name = 'uninstall failure'; Defines = @('/DDEMO_UNINSTALL_FAILURE', '/DRECORDING_DELAY_MS=5000') }
)) {
  $scenarioDefines = @($defines) + $scenario.Defines
  & $makeNsis @scenarioDefines (Join-Path $repoRoot 'demo\demo.nsi')
  if ($LASTEXITCODE -ne 0) {
    throw "$($scenario.Name) demo compilation failed with exit code $LASTEXITCODE"
  }
}
& $makeNsis @defines (Join-Path $repoRoot 'demo\smoke.nsi')
if ($LASTEXITCODE -ne 0) {
  throw "Smoke installer compilation failed with exit code $LASTEXITCODE"
}

$smokeResult = Join-Path $env:TEMP 'nsis-duilib-ui-smoke.txt'
Remove-Item -LiteralPath $smokeResult -Force -ErrorAction SilentlyContinue
$smokeProcess = Start-Process -FilePath (Join-Path $outputDir 'NsisDuiSmoke.exe') -ArgumentList '/S' -WindowStyle Hidden -Wait -PassThru
if ($smokeProcess.ExitCode -ne 0) {
  throw "Smoke installer exited with code $($smokeProcess.ExitCode)"
}
if (-not (Test-Path -LiteralPath $smokeResult)) {
  throw 'Smoke installer did not write its result file'
}
$result = Get-Content -LiteralPath $smokeResult -Raw
if ($result -notmatch 'init=ok' -or $result -notmatch 'version=0\.1\.0') {
  throw "Unexpected smoke result: $result"
}

Write-Output "Interactive demo: $(Join-Path $outputDir 'OneKeyModernSetupDemo.exe')"
Write-Output "Install failure demo: $(Join-Path $outputDir 'OneKeyModernFailureDemo.exe')"
Write-Output "Uninstall demo: $(Join-Path $outputDir 'OneKeyModernUninstallDemo.exe')"
Write-Output "Uninstall failure demo: $(Join-Path $outputDir 'OneKeyModernUninstallFailureDemo.exe')"
Write-Output "Smoke result:`n$result"
