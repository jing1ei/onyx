<#
.SYNOPSIS
    Build the Windows release of Onyx: an .exe plus an NSIS installer.

.DESCRIPTION
    Run from an ordinary PowerShell 5+/7+ prompt on Windows x64:

        .\scripts\build-windows.ps1

    The file is deliberately pure ASCII: Windows PowerShell 5.1 reads a script
    without a byte-order mark as ANSI, not UTF-8, so any non-ASCII character
    here would be mangled on exactly the version of PowerShell that ships with
    Windows 10.

    Requirements:
      * Rust (rustup) with the MSVC toolchain,
      * "Desktop development with C++" from the Visual Studio Build Tools
        (link.exe and the Windows SDK),
      * Microsoft Edge WebView2 runtime (present on Windows 10 21H2+/11),
      * Node.js 20.19+ or 22.12+ (Vite 7's own floor; 18 is too old).

    Everything above is checked before anything is built, and every missing
    piece is reported in one pass with the way to install it - see the
    preflight below, and scripts/build-mac.sh, which does the same for macOS.

.PARAMETER Msi
    Also emit an .msi (WiX) next to the NSIS installer.
#>
[CmdletBinding()]
param(
    [switch]$Msi
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

if (-not $IsWindows -and $env:OS -ne 'Windows_NT') {
    throw 'The Windows bundle can only be produced on Windows.'
}

$target = 'x86_64-pc-windows-msvc'

# --- preflight ----------------------------------------------------------------
# Check everything up front and report *all* that is missing, rather than dying
# on the first problem and making the reader run a 20-minute build four times to
# discover four of them. Same policy, and the same wording, as build-mac.sh.
$missing = @()

function Test-Tool($name, $howToInstall) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        $script:missing += "'$name' not found - $howToInstall"
    }
}

Test-Tool 'rustup' 'install Rust: https://rustup.rs (choose the MSVC host toolchain)'
Test-Tool 'cargo'  'install Rust (see above); if it is installed, add %USERPROFILE%\.cargo\bin to PATH'
Test-Tool 'node'   'install Node.js 20.19+ or 22.12+: winget install OpenJS.NodeJS.LTS'
Test-Tool 'npm'    'install Node.js (npm ships with it)'

# Vite 7 requires Node ^20.19 || >=22.12 and dies with an opaque syntax error
# from inside its own ESM on older runtimes, so name the real problem here.
if (Get-Command node -ErrorAction SilentlyContinue) {
    $raw = (& node -v) -replace '^v', ''            # e.g. 22.23.2
    $parts = $raw.Split('.')
    $nodeMajor = [int]$parts[0]
    $nodeMinor = if ($parts.Count -gt 1) { [int]$parts[1] } else { 0 }
    $tooOld = ($nodeMajor -lt 20) -or
              ($nodeMajor -eq 20 -and $nodeMinor -lt 19) -or
              ($nodeMajor -eq 21) -or
              ($nodeMajor -eq 22 -and $nodeMinor -lt 12)
    if ($tooOld) {
        $missing += "Node $raw is too old for Vite 7 - need 20.19+ or 22.12+."
    }
}

# The General MIDI bank of SPEC 18 is linked into the binary with
# include_bytes!, so a checkout without it fails deep inside `cargo build` with
# a path nobody recognises. The licence text is a bundle resource named by
# tauri.conf.json and would fail even later, during bundling.
$bank = 'crates\onyx-core\assets\gm\GeneralUser-GS.sf2'
$bankLicence = 'crates\onyx-core\assets\gm\GeneralUser-GS-LICENSE.txt'
if (-not (Test-Path $bank)) {
    $missing += "$bank is missing - it is the bundled General MIDI bank (SPEC 18, recorded in THIRD-PARTY.md) and is linked into the binary; restore it with: git checkout -- $bank"
} elseif ((Get-Item $bank).Length -lt 1MB) {
    $missing += "$bank is only $((Get-Item $bank).Length) bytes - a truncated or placeholder SoundFont; see THIRD-PARTY.md for the expected size and SHA-256"
}
if (-not (Test-Path $bankLicence)) {
    $missing += "$bankLicence is missing - tauri.conf.json ships it as a bundle resource, so bundling fails without it"
}

# The front end is three documents, and each detached window needs its own
# capability file (SPEC 5): a window whose capability is missing builds fine and
# then cannot even subscribe to an event, which is a bug report, not a build
# error. Cheap to check here, so check it here.
foreach ($doc in 'index.html', 'eq.html', 'theme.html') {
    if (-not (Test-Path $doc)) {
        $missing += "$doc is missing - vite.config.ts builds it as an entry point (SPEC 5)"
    }
}
foreach ($cap in 'default.json', 'eq.json', 'theme.json') {
    $path = "src-tauri\capabilities\$cap"
    if (-not (Test-Path $path)) {
        $missing += "$path is missing - each window needs its own capability whitelist (SPEC 5.1)"
    }
}

if ($missing.Count -gt 0) {
    foreach ($m in $missing) { Write-Host "error: $m" -ForegroundColor Red }
    Write-Host ''
    throw 'Fix what is listed above and run this script again.'
}

# link.exe is a warning rather than an error: it is on PATH only inside a
# developer prompt, and `cargo` may still find the toolchain through the VS
# installation it discovers itself.
if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
    Write-Warning 'link.exe is not on PATH. If the build fails with "linker `link.exe` not found", run this from a "Developer PowerShell for VS" prompt, or install "Desktop development with C++" from the Visual Studio Build Tools.'
}

# WebView2 is a *runtime* dependency: without it the build succeeds and the app
# then refuses to open a window, which is a much worse way to find out.
$webview2 = @(
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
) | Where-Object { Test-Path $_ -ErrorAction SilentlyContinue }
if (-not $webview2) {
    Write-Warning 'The Edge WebView2 runtime was not found in the registry. It ships with Windows 11 and Windows 10 21H2+; on older machines the built app will start and show no window until you install the Evergreen runtime from https://developer.microsoft.com/microsoft-edge/webview2/.'
}

# A release build of this workspace plus the bundling step needs room: target\
# reaches roughly 6-10 GB for a single triple. Warn rather than refuse - the
# number is an estimate, not a contract.
$drive = (Get-Item .).PSDrive
if ($drive -and $drive.Free) {
    $freeGb = [math]::Round($drive.Free / 1GB, 1)
    if ($freeGb -lt 12) {
        Write-Warning "Only $freeGb GB free on $($drive.Name): a release build needs roughly 10 GB under target\."
    }
}

# --- toolchain ---------------------------------------------------------------
$installed = & rustup target list --installed
if ($installed -notcontains $target) {
    Write-Host "> installing Rust target $target"
    & rustup target add $target
    if ($LASTEXITCODE -ne 0) { throw "rustup target add $target failed" }
}

# --- front end ---------------------------------------------------------------
if (-not (Test-Path node_modules)) {
    Write-Host '> npm install'
    & npm install
    if ($LASTEXITCODE -ne 0) { throw 'npm install failed' }
}

# --- optional code signing ----------------------------------------------------
# Unsigned installers trigger a SmartScreen warning. To sign, set these before
# running (Tauri passes them to signtool):
#
#   $env:TAURI_SIGNING_PRIVATE_KEY      = '...'   # updater key, optional
#   $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '...'
#
# and put the certificate thumbprint in src-tauri/tauri.conf.json under
# bundle.windows.certificateThumbprint (plus digestAlgorithm / timestampUrl).

$bundles = if ($Msi) { 'nsis,msi' } else { 'nsis' }

# `tauri build` runs beforeBuildCommand = `npm run build`, which typechecks,
# runs the EQ / A-B / theme contract checks and then builds all three documents.
Write-Host "> npm run tauri build -- --target $target --bundles $bundles"
& npm run tauri build -- --target $target --bundles $bundles
if ($LASTEXITCODE -ne 0) { throw 'tauri build failed' }

# NOTE: this is a cargo *workspace* (root Cargo.toml with members src-tauri and
# crates/onyx-core), so the target directory is at the repo root, not under
# src-tauri\. The raw executable is named after the cargo binary
# (src-tauri/Cargo.toml package.name = "onyx"), not after bundle.productName.
$out = "target\$target\release"
$exe = Get-ChildItem -Path $out -Filter '*.exe' -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notmatch '^build-' } | Select-Object -First 1
$nsis = Get-ChildItem -Path "$out\bundle\nsis" -Filter '*.exe' -File -ErrorAction SilentlyContinue |
    Select-Object -First 1

# All three documents have to be *in* the bundle, not merely buildable: a
# missing eq.html turns "E" into a blank window at runtime.
$shipped = 'index.html', 'eq.html', 'theme.html' | Where-Object { Test-Path "dist\$_" }
if ($shipped.Count -ne 3) {
    Write-Warning "dist\ carries $($shipped.Count) of the 3 documents ($($shipped -join ', ')): the EQ (SPEC 12) or theme editor (SPEC 20) window would come up blank."
}

Write-Host ''
Write-Host '+ done'
if ($exe)  { Write-Host "  exe   : $($exe.FullName)  ($([math]::Round($exe.Length / 1MB, 1)) MB, General MIDI bank included)" }
else       { Write-Host "  exe   : not found under $out" }
if ($nsis) { Write-Host "  nsis  : $($nsis.FullName)  ($([math]::Round($nsis.Length / 1MB, 1)) MB)" }
else       { Write-Host "  nsis  : not found under $out\bundle\nsis" }
if ($Msi) {
    $msi = Get-ChildItem -Path "$out\bundle\msi" -Filter '*.msi' -File -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($msi) { Write-Host "  msi   : $($msi.FullName)" }
    else      { Write-Host "  msi   : not found under $out\bundle\msi" }
}
Write-Host ''
Write-Host '  The installer registers the file associations, so "Open with > Onyx"'
Write-Host '  works after installing (running the raw .exe does not register them).'

# Read the list out of the config rather than repeating it here: a hard-coded
# list in this script is one more copy to rot, and the config is the only thing
# the installer actually reads.
try {
    $conf = Get-Content 'src-tauri\tauri.conf.json' -Raw | ConvertFrom-Json
    foreach ($assoc in $conf.bundle.fileAssociations) {
        $exts = ($assoc.ext | ForEach-Object { ".$_" }) -join ' '
        Write-Host "    $($assoc.name): $exts"
    }
} catch {
    Write-Warning "Could not read the file associations out of src-tauri\tauri.conf.json: $_"
}
