# Runs the release gates and creates a separate NSIS package.
# It never installs, replaces, or stops the currently installed GPTVoice app.
$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$CargoManifest = Join-Path $ProjectRoot "src-tauri\Cargo.toml"
$TargetTriple = "x86_64-pc-windows-msvc"

# Start with the Rust toolchain and load the installed Visual Studio developer environment.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$VsWhere = "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
$VsInstallPath = $null
if (Test-Path $VsWhere) {
    $VsInstallPath = (& $VsWhere -latest -products "*" -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
}
if ($VsInstallPath) {
    $VsDevCmd = Join-Path $VsInstallPath "Common7\Tools\VsDevCmd.bat"
    if (Test-Path $VsDevCmd) {
        $CmdLine = '"' + $VsDevCmd + '" -arch=amd64 -host_arch=amd64 >nul && set'
        $DeveloperEnvironment = cmd.exe /d /s /c $CmdLine
        foreach ($Line in $DeveloperEnvironment) {
            if ($Line -match "^([^=]+)=(.*)$") {
                [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
            }
        }
    }
}

# Locate CMake from PATH or the per-user WinGet package installation.
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    $WinGetPackages = Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Packages"
    if (Test-Path $WinGetPackages) {
        $CMakePackage = Get-ChildItem $WinGetPackages -Directory -Filter "Kitware.CMake_*" -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($CMakePackage) {
            $CMakeExe = Get-ChildItem $CMakePackage.FullName -Filter "cmake.exe" -Recurse -File -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match "\\bin\\cmake\.exe$" } | Select-Object -First 1
            if ($CMakeExe) {
                $env:PATH = "$(Split-Path -Parent $CMakeExe.FullName);$env:PATH"
            }
        }
    }
}
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Cargo was not found. Install Rust stable MSVC and reopen PowerShell."
}
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    throw "MSVC link.exe is missing. Install the Desktop development with C++ workload using docs\vs-buildtools.vsconfig."
}
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    throw "CMake is missing. Install Kitware CMake before building the bundled Opus library."
}
if (-not $env:WindowsSdkDir) {
    throw "Windows SDK environment is not loaded. Run this script from a Visual Studio developer environment."
}

$MtTool = Join-Path $env:WindowsSdkDir ("bin\" + $env:WindowsSDKVersion.TrimEnd("\") + "\x64\mt.exe")
if (-not (Test-Path $MtTool)) {
    throw "Manifest Tool mt.exe was not found in the installed Windows SDK: $MtTool"
}

Push-Location $ProjectRoot
try {
    Write-Host "== Rust formatting ==" -ForegroundColor Cyan
    cargo fmt --manifest-path $CargoManifest --all -- --check
    if ($LASTEXITCODE -ne 0) { throw "cargo fmt failed with exit code $LASTEXITCODE" }

    Write-Host "== Locked Cargo metadata ==" -ForegroundColor Cyan
    cargo metadata --manifest-path $CargoManifest --no-deps --format-version 1 --locked | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed with exit code $LASTEXITCODE" }

    Write-Host "== Frontend syntax ==" -ForegroundColor Cyan
    node --check .\tauri-ui\main.js
    if ($LASTEXITCODE -ne 0) { throw "tauri-ui/main.js syntax check failed" }
    node --check .\src\browser\media-bridge.js
    if ($LASTEXITCODE -ne 0) { throw "media-bridge.js syntax check failed" }

    Write-Host "== Node regression tests ==" -ForegroundColor Cyan
    npm.cmd test
    if ($LASTEXITCODE -ne 0) { throw "Node regression tests failed with exit code $LASTEXITCODE" }

    Write-Host "== Compile Rust library tests ==" -ForegroundColor Cyan
    cargo test --lib --manifest-path $CargoManifest --target $TargetTriple --locked --no-run
    if ($LASTEXITCODE -ne 0) { throw "Rust test compilation failed with exit code $LASTEXITCODE" }

    # Cargo's test harness needs the Common Controls v6 activation manifest
    # because Tauri's Windows dependency imports TaskDialogIndirect.
    $TestBuildRoot = Join-Path $ProjectRoot ("src-tauri\target\" + $TargetTriple + "\debug\build\gptvoice-tauri")
    $TestExecutable = Get-ChildItem $TestBuildRoot -Filter "gptvoice_tauri_lib-*.exe" -Recurse -File -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $TestExecutable) {
        throw "Rust test executable was not produced under $TestBuildRoot"
    }
    $TestManifest = $TestExecutable.FullName + ".manifest"
    $ManifestXml = @"
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="amd64" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"@
    [IO.File]::WriteAllText($TestManifest, $ManifestXml, [Text.UTF8Encoding]::new($false))
    & $MtTool "-manifest" $TestManifest ("-outputresource:" + $TestExecutable.FullName + ";#1")
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to embed the Common Controls v6 manifest into the Rust test executable."
    }

    Write-Host "== Rust library tests ==" -ForegroundColor Cyan
    cargo test --lib --manifest-path $CargoManifest --target $TargetTriple --locked
    if ($LASTEXITCODE -ne 0) { throw "Rust library tests failed with exit code $LASTEXITCODE" }

    Write-Host "== Tauri CLI check ==" -ForegroundColor Cyan
    cargo tauri --version
    if ($LASTEXITCODE -ne 0) { throw "Tauri CLI is not installed or failed to run." }

    Write-Host "== Tauri release package ==" -ForegroundColor Cyan
    npm.cmd run tauri:build
    if ($LASTEXITCODE -ne 0) { throw "Tauri release build failed with exit code $LASTEXITCODE" }

    $BundleDirectory = Join-Path $ProjectRoot "src-tauri\target\release\bundle\nsis"
    $Installers = @(Get-ChildItem $BundleDirectory -Filter "*.exe" -File -ErrorAction Stop)
    if ($Installers.Count -eq 0) {
        throw "Tauri build completed but no NSIS installer was found in $BundleDirectory"
    }

    Write-Host "Build and automated checks completed. Installer artifacts:" -ForegroundColor Green
    $Installers | Select-Object FullName, Length, LastWriteTime | Format-Table -AutoSize
    foreach ($Installer in $Installers) {
        Get-FileHash -Algorithm SHA256 $Installer.FullName
    }
    Write-Host "The installed application was not changed." -ForegroundColor Green
}
finally {
    Pop-Location
}