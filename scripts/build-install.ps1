<#
.SYNOPSIS
Build pult from this checkout and install it for the current user, from the terminal.

.DESCRIPTION
The same entry point the other desktop apps have (scripts/build-install.ps1, scripts/uninstall.ps1),
for a machine where no setup may be started: `cargo build --release --locked` builds both programs
(pult.exe, the command line; pult-app.exe, the window), package-release.ps1 stages them in the shape
of the release zip and install.ps1 installs that, the one installer for a build and for a release:
%LOCALAPPDATA%\pult\bin (or $env:PULT_INSTALL_DIR), a Start menu entry "pult" that opens the window
and an entry in Settings > Apps. No administrator rights; a file built on this machine carries no
Mark-of-the-Web. A pult that runs goes on with the old program until it is closed. Your profiles are
not touched.

Run it in a normal terminal: a Claude session (the Claude app redirects AppData and HKCU) may
build (-BuildOnly), installing there is refused.

.PARAMETER BuildOnly
Build target\release\pult.exe and pult-app.exe and stop (no install).

.PARAMETER AllowElevated
Install although the session is elevated (for a build machine).

.EXAMPLE
.\scripts\build-install.ps1
.\scripts\build-install.ps1 -BuildOnly
#>
[CmdletBinding()]
param(
    [switch] $BuildOnly,
    [switch] $AllowElevated
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$programs = @('pult.exe', 'pult-app.exe')

function Exit-Build([string] $Message) {
    throw "build-install.ps1: $Message"
}

if (-not $BuildOnly) {
    if ($env:CLAUDECODE) {
        Exit-Build 'install in a normal terminal, not in a Claude session (the app redirects AppData and the registry); -BuildOnly builds'
    }
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $AllowElevated -and $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Exit-Build 'do not run this elevated: it installs for one user (-AllowElevated for a build machine)'
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Exit-Build 'cargo is not on PATH' }

$root = Split-Path -Parent $PSScriptRoot
$built = Join-Path $root 'target\release'

# cargo cannot replace a program that runs from target\release ("access denied"): name it instead.
$running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
        $path = $null
        try { $path = $_.Path } catch { }
        $path -and ($programs | Where-Object { $path -ieq (Join-Path $built $_) })
    })
if ($running.Count -gt 0) {
    $which = ($running | ForEach-Object { "$($_.Name) (PID $($_.Id))" }) -join ', '
    Exit-Build "$which runs from $built and blocks the build: close it and run again"
}

Push-Location $root
try {
    Write-Output '==> cargo build --release --locked'
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { Exit-Build "cargo build failed (exit code $LASTEXITCODE)" }
    foreach ($name in $programs) {
        $exe = Join-Path $built $name
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { Exit-Build "the build made no $name" }
        # cargo leaves an exe that is already current untouched; the time says "built now" to :MyNative
        (Get-Item -LiteralPath $exe).LastWriteTime = Get-Date
    }
    Write-Output "built $($programs -join ', ') ($built)"
    if ($BuildOnly) { return }

    $stage = Join-Path ([IO.Path]::GetTempPath()) ('pult-build-install-' + [Guid]::NewGuid().ToString('N'))
    try {
        & (Join-Path $PSScriptRoot 'package-release.ps1') -BuildDir $built -StageDir $stage -NoZip
        & (Join-Path $PSScriptRoot 'install.ps1') -PayloadDir $stage -AllowElevated:$AllowElevated
    }
    finally {
        if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue }
    }
}
finally {
    Pop-Location
}
