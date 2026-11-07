<#
.SYNOPSIS
Remove pult for the current user on Windows, without administrator rights.

.DESCRIPTION
Takes away what scripts/install.ps1 made (also through build-install.ps1): the files of the install
folder (%LOCALAPPDATA%\pult\bin, or $env:PULT_INSTALL_DIR), the Start menu entry, the entry in
Settings > Apps (which runs this script from there) and the start with Windows when it starts this
install. Only the files the installer puts there go; a folder that holds anything else stays.

Your profiles and %APPDATA%\pult (settings, logs) are never touched. A second run finds nothing to do
and says so; it is not an error. A program that still runs cannot be removed: that is an error, not a
"removed". It is refused before anything goes, and the PIDs are named.

.PARAMETER AllowElevated
Remove although the session is elevated (for a build machine).

.PARAMETER PauseOnError
On an error, print it and wait for Enter (the Settings > Apps entry passes it: its console window
would close before the message could be read).

.PARAMETER InstallDir
The install folder (default $env:PULT_INSTALL_DIR, else %LOCALAPPDATA%\pult\bin).

.PARAMETER StartMenuDir
The folder of the Start menu entry (default $env:PULT_START_MENU_DIR, else the current user's Start
menu programs folder).

.PARAMETER RegistryRoot
The registry key that stands for HKCU\Software (default $env:PULT_REGISTRY_ROOT, else HKCU:\Software).
For tests.

.EXAMPLE
.\scripts\uninstall.ps1
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', 'PauseOnError', Justification = 'read in the trap')]
[CmdletBinding()]
param(
    [switch] $AllowElevated,
    [switch] $PauseOnError,
    [string] $InstallDir = '',
    [string] $StartMenuDir = '',
    [string] $DesktopDir = '',
    [string] $RegistryRoot = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

trap {
    if ($PauseOnError) {
        Write-Output "uninstall failed: $($_.Exception.Message)"
        Read-Host 'press Enter to close' | Out-Null
    }
    break
}

$programs = @('pult', 'pult-app')

function Exit-Uninstall([string] $Message) {
    throw "uninstall.ps1: $Message"
}

function Get-Setting([string] $Given, [string] $EnvName, [string] $Default) {
    if ($Given) { return $Given }
    $fromEnv = [Environment]::GetEnvironmentVariable($EnvName)
    if ($fromEnv) { return $fromEnv }
    return $Default
}

# Whether $Path lies inside $Dir (Windows paths: letter case does not count).
function Test-Under([string] $Path, [string] $Dir) {
    try {
        $p = [IO.Path]::GetFullPath($Path)
        $d = [IO.Path]::GetFullPath($Dir).TrimEnd('\') + '\'
    }
    catch {
        return $false
    }
    return $p.StartsWith($d, [StringComparison]::OrdinalIgnoreCase)
}

# An absolute folder that is not a drive root, without a trailing backslash (as install.ps1).
function Get-Place([string] $Dir) {
    if (-not $Dir -or $Dir.Contains('"') -or -not [IO.Path]::IsPathRooted($Dir)) { Exit-Uninstall "not an absolute path: '$Dir'" }
    $full = [IO.Path]::GetFullPath($Dir)
    if ($full.TrimEnd('\') -eq ([IO.Path]::GetPathRoot($full)).TrimEnd('\')) { Exit-Uninstall "a drive root is no place for this: '$Dir'" }
    return $full.TrimEnd('\')
}

# The registry root stands for HKCU\Software: a key of the current user, never another hive or a file path.
function Get-RegistryRoot([string] $Key) {
    $Key = $Key.TrimEnd('\')
    if ($Key.Contains('"') -or $Key -notmatch '^(HKCU:|Registry::HKEY_CURRENT_USER)\\.') { Exit-Uninstall "not a key below HKCU: '$Key'" }
    return $Key
}

# A file that cannot be removed is an error, never a "removed".
function Invoke-Removal([string] $Path, [switch] $Recurse) {
    try { Remove-Item -LiteralPath $Path -Force -Recurse:$Recurse }
    catch { Exit-Uninstall "cannot remove ${Path}: $($_.Exception.Message)" }
    if (Test-Path -LiteralPath $Path) { Exit-Uninstall "$Path is still there" }
}

# A folder that is empty goes; one that cannot (a terminal whose current folder it is) stays with a
# word, not an error: nothing of pult is in it, and the Settings > Apps entry must still go.
function Invoke-EmptyFolderRemoval([string] $Dir) {
    if (-not (Test-Path -LiteralPath $Dir -PathType Container) -or (Get-ChildItem -LiteralPath $Dir -Force)) { return }
    try { Remove-Item -LiteralPath $Dir -Force }
    catch { Write-Output "kept the empty folder $Dir (in use: $($_.Exception.Message))" }
}

# Whether $Dir holds a pult install: a program of ours (also a .new/.old left by a cut short install),
# or the VERSION and uninstall.ps1 of a release.
function Test-Installation([string] $Dir) {
    foreach ($name in $programs) {
        foreach ($suffix in '', '.new', '.old') {
            if (Test-Path -LiteralPath (Join-Path $Dir "$name.exe$suffix") -PathType Leaf) { return $true }
        }
    }
    return (Test-Path -LiteralPath (Join-Path $Dir 'uninstall.ps1') -PathType Leaf) -and (Test-Path -LiteralPath (Join-Path $Dir 'VERSION') -PathType Leaf)
}

if ($env:CLAUDECODE) {
    Exit-Uninstall 'remove in a normal terminal, not in a Claude session (the app redirects AppData and the registry)'
}
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $AllowElevated -and $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Exit-Uninstall 'do not run this elevated: it removes the installation of one user (-AllowElevated for a build machine)'
}

$InstallDir = Get-Place (Get-Setting $InstallDir 'PULT_INSTALL_DIR' (Join-Path $env:LOCALAPPDATA 'pult\bin'))
$StartMenuDir = Get-Place (Get-Setting $StartMenuDir 'PULT_START_MENU_DIR' ([Environment]::GetFolderPath('Programs')))
$RegistryRoot = Get-RegistryRoot (Get-Setting $RegistryRoot 'PULT_REGISTRY_ROOT' 'HKCU:\Software')

$running = @(Get-Process -Name $programs -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and (Test-Under $_.Path $InstallDir) })
if ($running) {
    $names = ($running | ForEach-Object { "$($_.Name) (PID $($_.Id))" }) -join ', '
    Exit-Uninstall "running from ${InstallDir}: $names; close pult (Quit in the tray menu), then run this again"
}

$removed = @()

# 1. The start with Windows, when it starts this install; one of another copy stays.
$runKey = "$RegistryRoot\Microsoft\Windows\CurrentVersion\Run"
$run = Get-ItemProperty -LiteralPath $runKey -Name 'pult' -ErrorAction SilentlyContinue
if ($run) {
    if ([string]$run.pult -match '^"([^"]*)"( --tray)?$' -and (Test-Under $Matches[1] $InstallDir)) {
        Remove-ItemProperty -LiteralPath $runKey -Name 'pult'
        $removed += 'the start with Windows'
    }
    else {
        Write-Output "kept the Run value 'pult' (it is not this install): $($run.pult)"
    }
}

# 2. The Start menu entry, when it leads into the install folder.
$link = Join-Path $StartMenuDir 'pult.lnk'
if (Test-Path -LiteralPath $link -PathType Leaf) {
    $target = (New-Object -ComObject WScript.Shell).CreateShortcut($link).TargetPath
    if ($target -and (Test-Under $target $InstallDir)) {
        Invoke-Removal $link
        $removed += 'the Start menu entry'
    }
    else {
        Write-Output "kept $link (it starts $target, not this install)"
    }
}

# 2b. The desktop shortcut, when it leads into the install folder.
if ($DesktopDir -eq '') { $DesktopDir = $(if ($env:PULT_DESKTOP_DIR) { $env:PULT_DESKTOP_DIR } else { [Environment]::GetFolderPath('Desktop') }) }
$deskLink = Join-Path $DesktopDir 'pult.lnk'
if (Test-Path -LiteralPath $deskLink -PathType Leaf) {
    $deskTarget = (New-Object -ComObject WScript.Shell).CreateShortcut($deskLink).TargetPath
    if ($deskTarget -and (Test-Under $deskTarget $InstallDir)) {
        Invoke-Removal $deskLink
        $removed += 'the desktop shortcut'
    }
}

# 3. The files install.ps1 puts there; a folder that holds anything else stays. The .new and .old files
# are what an install that was cut short, or that a running pult kept, leaves. Only a folder that is a
# pult install gets this: a wrong -InstallDir (a checkout, a work folder) must not lose its README.md.
$installed = @('pult.exe', 'pult-app.exe', 'uninstall.ps1', 'example-profile.toml', 'LICENSE', 'README.md', 'VERSION')
if (Test-Installation $InstallDir) {
    foreach ($name in $installed | ForEach-Object { $_; "$_.new"; "$_.old" }) {
        $file = Join-Path $InstallDir $name
        if (Test-Path -LiteralPath $file) {
            Invoke-Removal $file
            if ($installed -contains $name) { $removed += $file }
        }
    }
}
elseif (Test-Path -LiteralPath $InstallDir) {
    Write-Output "kept $InstallDir (no pult program and no VERSION with uninstall.ps1 in it: it is not an install of this program)"
}
Invoke-EmptyFolderRemoval $InstallDir
# The parent only when it is the folder the installer makes (never a folder you chose).
$parent = Split-Path -Parent $InstallDir
if ((Split-Path -Leaf $InstallDir) -eq 'bin' -and (Split-Path -Leaf $parent) -eq 'pult') { Invoke-EmptyFolderRemoval $parent }

# 4. The Settings > Apps entry last: while anything above fails, it stays to run again.
$uninstallKey = "$RegistryRoot\Microsoft\Windows\CurrentVersion\Uninstall\pult"
$entry = Get-ItemProperty -LiteralPath $uninstallKey -ErrorAction SilentlyContinue
if ($entry) {
    $location = ''
    if ($entry.PSObject.Properties['InstallLocation']) { $location = [string]$entry.InstallLocation }
    if ($location -and ([IO.Path]::GetFullPath($location).TrimEnd('\') -eq $InstallDir)) {
        Invoke-Removal $uninstallKey -Recurse
        $removed += 'the Settings > Apps entry'
    }
    else {
        Write-Output "kept the Settings > Apps entry (it is for $location, not this install)"
    }
}

if (-not $removed) {
    Write-Output "nothing to remove: pult is not installed for this user ($InstallDir)"
    return
}
Write-Output ''
$removed | ForEach-Object { Write-Output "removed $_" }
Write-Output 'your profiles and %APPDATA%\pult were not touched'
