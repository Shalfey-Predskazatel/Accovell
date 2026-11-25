param(
  [ValidateSet("Debug", "Release")]
  [string]$Configuration = "Debug",
  [switch]$SkipTests
)

$ErrorActionPreference = "Stop"

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path -LiteralPath $vswhere)) {
  throw "vswhere.exe was not found. Install Visual Studio 2022."
}

$vsPath = & $vswhere `
  -latest `
  -products "*" `
  -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
  -property installationPath
if (-not $vsPath) {
  throw "Visual Studio 2022 with the C++ desktop toolset was not found."
}

$devCmd = Join-Path $vsPath "Common7\Tools\VsDevCmd.bat"
$cmakeDir = Join-Path $vsPath "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin"
$ninjaDir = Join-Path $vsPath "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja"
$cmake = Join-Path $cmakeDir "cmake.exe"

$environmentLines = & cmd.exe /d /s /c `
  "`"$devCmd`" -no_logo -arch=x86 -host_arch=x64 >nul && set"
if ($LASTEXITCODE -ne 0) {
  throw "Failed to initialize the Visual Studio x86 build environment."
}

foreach ($line in $environmentLines) {
  $separator = $line.IndexOf("=")
  if ($separator -gt 0) {
    $name = $line.Substring(0, $separator)
    $value = $line.Substring($separator + 1)
    [Environment]::SetEnvironmentVariable($name, $value, "Process")
  }
}

$env:PATH = "$cmakeDir;$ninjaDir;$env:PATH"

& $cmake --preset windows-x86
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

$presetSuffix = $Configuration.ToLowerInvariant()
& $cmake --build --preset "windows-x86-$presetSuffix"
if ($LASTEXITCODE -ne 0) {
  exit $LASTEXITCODE
}

if (-not $SkipTests) {
  & (Join-Path $cmakeDir "ctest.exe") --preset "windows-x86-$presetSuffix"
  exit $LASTEXITCODE
}
