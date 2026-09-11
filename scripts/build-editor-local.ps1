param([string]$OutputDirectory = '.tools/windows-release', [switch]$Installer)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
function Check-Exit { if ($LASTEXITCODE -ne 0) { throw "Build failed: $LASTEXITCODE" } }
if ($Installer) {
    npm.cmd run tauri -- build --bundles nsis --ci
    Check-Exit
} else {
    npm.cmd run build
    Check-Exit
    cargo build -p onyx --release
    Check-Exit
}
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
Copy-Item -LiteralPath (Join-Path $targetRoot 'release/onyx.exe') -Destination (Join-Path $OutputDirectory 'Onyx.exe')
Copy-Item -LiteralPath LICENSE,THIRD-PARTY.md,WINDOWS_RELEASE.zh-CN.md -Destination $OutputDirectory
Copy-Item -LiteralPath crates/onyx-core/assets/gm/GeneralUser-GS-LICENSE.txt -Destination $OutputDirectory
if ($Installer) {
    $installers = @(Get-ChildItem -LiteralPath (Join-Path $targetRoot 'release/bundle/nsis') -Filter '*.exe')
    if ($installers.Count -ne 1) { throw 'Expected one NSIS installer' }
    Copy-Item -LiteralPath $installers[0].FullName -Destination (Join-Path $OutputDirectory 'Onyx-Branch-1.0.0-windows-x64-setup.exe')
}
Write-Output (Resolve-Path $OutputDirectory).Path
