<#
.SYNOPSIS
Pack the release zip of pult from prebuilt programs: pult-<version>-x86_64-pc-windows-msvc.zip.

.DESCRIPTION
Nothing is built here. The zip holds, in one folder, exactly what scripts\install.ps1 installs:

  pult.exe, pult-app.exe    the command line and the window
  install.ps1               installs this zip's content for the current user
  uninstall.ps1, example-profile.toml (examples\pult.toml), LICENSE, README.md (how to install), VERSION

The zip is reproducible: entries are sorted, use forward slashes and a fixed timestamp, so the same
programs give the same bytes (and the same checksum) when packed by the same PowerShell/.NET runtime.
SHA256SUMS ("<hash>  <file>", one line per zip packed into that folder) is written next to it for a
local check; the workflows do not upload that file, the publish action of appkit's release-kit makes
the one of the release.

It packs the programs that are in -BuildDir; it does not know whether they are fresh.

The release is a zip plus scripts, not a setup.exe: an unsigned setup would be stopped by SmartScreen
exactly like the programs, and the scripts already do the per-user install. There are two programs, so
the kit's own install script (which wants one program with an `install` command, or a setup) does not
fit either.

build-install.ps1 uses -StageDir and -NoZip: it stages the same folder from a fresh build and hands it
to install.ps1, so the checkout install and the zip install are one code path.

.PARAMETER BuildDir
The folder with pult.exe and pult-app.exe (default target\release of this checkout).

.PARAMETER OutDir
Where the zip and SHA256SUMS go (default dist\ in the checkout). Not needed with -NoZip.

.PARAMETER StageDir
Stage the folder here (it must not exist or be empty) and keep it. Without it a temporary folder is
used and removed.

.PARAMETER NoZip
Only stage (-StageDir is then required).

.PARAMETER Version
The version the release says it is (a tag without its v). It must be the version of the workspace in
Cargo.toml.

.PARAMETER Rolling
Pack the rolling build: a fixed file name (pult-windows-x86_64.zip) and the version label
<version>+<short commit>; needs -Sha.

.PARAMETER Sha
The commit of the rolling build (7 to 40 hex digits); the first 7 go into the version label.

.EXAMPLE
.\scripts\package-release.ps1 -OutDir dist
.\scripts\package-release.ps1 -Rolling -Sha 0123456789abcdef0123456789abcdef01234567 -OutDir dist
#>
[CmdletBinding()]
param(
    [string] $BuildDir = '',
    [string] $OutDir = '',
    [string] $StageDir = '',
    [switch] $NoZip,
    [string] $Version = '',
    [switch] $Rolling,
    [string] $Sha = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$programs = @('pult', 'pult-app')

function Exit-Package([string] $Message) {
    throw "package-release.ps1: $Message"
}

$root = Split-Path -Parent $PSScriptRoot
if (-not $BuildDir) { $BuildDir = Join-Path $root 'target\release' }
if ($NoZip -and -not $StageDir) { Exit-Package '-NoZip stages only: give -StageDir' }
if (-not $NoZip -and -not $OutDir) { $OutDir = Join-Path $root 'dist' }

# The version of the app is the one of the workspace; a release must say the same. The first version
# at the start of a line is that of [workspace.package]: dependencies carry theirs inside a line.
$cargoText = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargoText -notmatch '(?m)^version\s*=\s*"([^"]+)"') { Exit-Package 'no version in Cargo.toml' }
$appVersion = $Matches[1]
if ($appVersion -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+$') { Exit-Package "the version '$appVersion' is not X.Y.Z" }
if ($Version) {
    $asked = $Version -replace '^v', ''
    if ($asked -ne $appVersion) {
        Exit-Package "asked for version $asked but Cargo.toml says $appVersion (bump it in its own commit first)"
    }
}

if ($Rolling) {
    if ($Sha -cnotmatch '^[0-9a-f]{7,40}$') { Exit-Package "-Rolling needs -Sha, a commit id in lower case hex (got '$Sha')" }
    $label = "$appVersion+$($Sha.Substring(0, 7))"
    $zipName = 'pult-windows-x86_64.zip'
}
else {
    if ($Sha) { Exit-Package '-Sha belongs to -Rolling' }
    $label = $appVersion
    $zipName = "pult-$appVersion-x86_64-pc-windows-msvc.zip"
}

foreach ($name in $programs) {
    $exe = Join-Path $BuildDir "$name.exe"
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { Exit-Package "no $exe (build first: cargo build --release --locked)" }
    if ((Get-Item -LiteralPath $exe).Length -eq 0) { Exit-Package "$exe is empty" }
}

if ($StageDir) {
    if ((Test-Path -LiteralPath $StageDir) -and (Get-ChildItem -LiteralPath $StageDir -Force)) {
        Exit-Package "$StageDir is not empty"
    }
    $stage = $StageDir
    $temporary = $false
}
else {
    $stage = Join-Path ([IO.Path]::GetTempPath()) ('pult-stage-' + [Guid]::NewGuid().ToString('N'))
    $temporary = $true
}

function Copy-Into([string] $From, [string] $Name) {
    Copy-Item -LiteralPath $From -Destination (Join-Path $stage $Name) -Force
}

try {
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    foreach ($name in $programs) { Copy-Into (Join-Path $BuildDir "$name.exe") "$name.exe" }
    Copy-Into (Join-Path $PSScriptRoot 'install.ps1') 'install.ps1'
    Copy-Into (Join-Path $PSScriptRoot 'uninstall.ps1') 'uninstall.ps1'
    Copy-Into (Join-Path $root 'examples\pult.toml') 'example-profile.toml'
    Copy-Into (Join-Path $root 'LICENSE') 'LICENSE'
    Copy-Into (Join-Path $PSScriptRoot 'release-readme.md') 'README.md'
    # LF and no BOM: it is read by install.ps1 and by people.
    [IO.File]::WriteAllText((Join-Path $stage 'VERSION'), "$label`n", (New-Object Text.UTF8Encoding($false)))

    if ($NoZip) {
        Write-Output "staged $label in $stage"
        return
    }

    # The zip: entries sorted by ordinal comparison, forward slashes, one fixed timestamp.
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $zipPath = Join-Path (Resolve-Path -LiteralPath $OutDir).ProviderPath $zipName
    # SHA256SUMS keeps one line per zip of this folder. The line of this zip goes first: a zip that
    # fails halfway must not leave the old hash beside it (or beside no zip at all).
    $sumsPath = Join-Path (Split-Path -Parent $zipPath) 'SHA256SUMS'
    $sumLines = @()
    if (Test-Path -LiteralPath $sumsPath) {
        $sumLines = @([IO.File]::ReadAllText($sumsPath) -split "\r?\n" | Where-Object { $_ -and -not $_.EndsWith("  $zipName", [StringComparison]::Ordinal) })
        if ($sumLines.Count) { [IO.File]::WriteAllText($sumsPath, (($sumLines -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false))) }
        else { Remove-Item -LiteralPath $sumsPath -Force }
    }
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    $names = @(Get-ChildItem -LiteralPath $stage -File -Force | ForEach-Object { $_.Name })
    [Array]::Sort($names, [StringComparer]::Ordinal)
    Add-Type -AssemblyName System.IO.Compression
    $fixed = New-Object DateTimeOffset(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
    $stream = [IO.File]::Create($zipPath)
    try {
        $archive = New-Object IO.Compression.ZipArchive($stream, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($rel in $names) {
                $entry = $archive.CreateEntry($rel, [IO.Compression.CompressionLevel]::Optimal)
                $entry.LastWriteTime = $fixed
                $target = $entry.Open()
                try {
                    $source = [IO.File]::OpenRead((Join-Path $stage $rel))
                    try { $source.CopyTo($target) } finally { $source.Dispose() }
                }
                finally { $target.Dispose() }
            }
        }
        finally { $archive.Dispose() }
    }
    finally { $stream.Dispose() }

    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $allLines = @($sumLines) + @("$hash  $zipName")
    $sortKeys = @($allLines | ForEach-Object { $_.Substring($_.IndexOf('  ') + 2) })
    [Array]::Sort($sortKeys, $allLines, [StringComparer]::Ordinal)
    [IO.File]::WriteAllText($sumsPath, (($allLines -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))
    Write-Output "packaged $label"
    Write-Output $zipPath
    Write-Output "$hash  $zipName"
}
finally {
    if ($temporary -and (Test-Path -LiteralPath $stage)) { Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue }
}
