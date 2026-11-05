<#
.SYNOPSIS
Install pult for the current user from an unpacked release zip (or a staged folder).

.DESCRIPTION
Run it from the folder you unpacked pult-<version>-x86_64-pc-windows-msvc.zip into. It installs
exactly what that folder holds, all in one folder (%LOCALAPPDATA%\pult\bin, the place the hub looks
for pult):

  pult.exe, pult-app.exe, uninstall.ps1, example-profile.toml, LICENSE, README.md, VERSION

plus a Start menu entry "pult" (the window) and an entry in Settings > Apps (which runs the installed
uninstall.ps1). No administrator rights. The files are all copied first and then swapped in; a copy
or a swap that fails puts the old ones back, so the install is never a mix of versions.

A pult that runs goes on with the old program until it is closed: Windows lets a running program be
renamed, so the sound does not stop for an update.

When pult starts with Windows, the Run value is pointed at the installed window. It is never turned
on here: that stays a choice in the tray menu.

Your profiles and %APPDATA%\pult (settings, logs) are not touched.

Nothing is signed: Windows (SmartScreen) may ask before the first start of the window. Run it in a
normal terminal: a Claude session (the Claude app redirects AppData and HKCU) is refused.

This is the one installer: scripts\build-install.ps1 stages a fresh build in the same shape and calls
this script.

.PARAMETER PayloadDir
The folder to install from (default: the folder of this script, i.e. the unpacked zip).

.PARAMETER AllowElevated
Install although the session is elevated (for a build machine).

.PARAMETER InstallDir
Where the programs go (default $env:PULT_INSTALL_DIR, else %LOCALAPPDATA%\pult\bin).

.PARAMETER StartMenuDir
Where the Start menu entry goes (default $env:PULT_START_MENU_DIR, else the current user's Start menu
programs folder).

.PARAMETER RegistryRoot
The registry key that stands for HKCU\Software: the Settings > Apps entry and the Run value are below
it (default $env:PULT_REGISTRY_ROOT, else HKCU:\Software). For tests.

.EXAMPLE
.\install.ps1
# Windows PowerShell 5.1 under its default execution policy (Restricted), from the unpacked folder:
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
#>
[CmdletBinding()]
param(
    [switch] $AllowElevated,
    [string] $PayloadDir = '',
    [string] $InstallDir = '',
    [string] $StartMenuDir = '',
    [string] $RegistryRoot = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$programs = @('pult', 'pult-app')
# What a payload holds, no more and no less: anything else is a folder that is not a release.
$payloadFiles = @('pult.exe', 'pult-app.exe', 'install.ps1', 'uninstall.ps1', 'example-profile.toml', 'LICENSE', 'README.md', 'VERSION')

function Exit-Install([string] $Message) {
    throw "install.ps1: $Message"
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

# An absolute folder that is not a drive root, without a trailing backslash (a "C:\x\" would end the
# quoted argument of the Settings > Apps command line with \", which escapes the quote).
function Get-Place([string] $Dir) {
    if (-not $Dir -or $Dir.Contains('"') -or -not [IO.Path]::IsPathRooted($Dir)) { Exit-Install "not an absolute path: '$Dir'" }
    $full = [IO.Path]::GetFullPath($Dir)
    if ($full.TrimEnd('\') -eq ([IO.Path]::GetPathRoot($full)).TrimEnd('\')) { Exit-Install "a drive root is no place for this: '$Dir'" }
    return $full.TrimEnd('\')
}

# The registry root stands for HKCU\Software: a key of the current user, never another hive or a file path.
function Get-RegistryRoot([string] $Key) {
    $Key = $Key.TrimEnd('\')
    if ($Key.Contains('"') -or $Key -notmatch '^(HKCU:|Registry::HKEY_CURRENT_USER)\\.') { Exit-Install "not a key below HKCU: '$Key'" }
    return $Key
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
    Exit-Install 'install in a normal terminal, not in a Claude session (the app redirects AppData and the registry)'
}
# Everything goes to the account that runs this: elevated, that may be the administrator's.
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $AllowElevated -and $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Exit-Install 'do not run this elevated: it installs for one user (-AllowElevated for a build machine)'
}

$InstallDir = Get-Place (Get-Setting $InstallDir 'PULT_INSTALL_DIR' (Join-Path $env:LOCALAPPDATA 'pult\bin'))
$StartMenuDir = Get-Place (Get-Setting $StartMenuDir 'PULT_START_MENU_DIR' ([Environment]::GetFolderPath('Programs')))
$RegistryRoot = Get-RegistryRoot (Get-Setting $RegistryRoot 'PULT_REGISTRY_ROOT' 'HKCU:\Software')

# The payload: the shape of the release zip, checked before anything is written.
if (-not $PayloadDir) { $PayloadDir = $PSScriptRoot }
if (-not (Test-Path -LiteralPath $PayloadDir -PathType Container)) { Exit-Install "no such folder: $PayloadDir" }
$PayloadDir = (Resolve-Path -LiteralPath $PayloadDir).ProviderPath.TrimEnd('\')
foreach ($item in Get-ChildItem -LiteralPath $PayloadDir -Force) {
    if ($payloadFiles -cnotcontains $item.Name -or $item.PSIsContainer) {
        Exit-Install "$PayloadDir holds '$($item.Name)', which a release does not: nothing was installed"
    }
}
foreach ($name in $payloadFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $PayloadDir $name) -PathType Leaf)) {
        Exit-Install "no $name in $PayloadDir (run this from the unpacked release zip)"
    }
}
$version = (Get-Content -LiteralPath (Join-Path $PayloadDir 'VERSION') -TotalCount 1).Trim()
if ($version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+(\+[0-9a-f]{7})?$') { Exit-Install "VERSION says '$version', which is not a version of a release" }

# The install folder is new, empty or an earlier install of this program: files of somebody else's (a
# shared tools folder with its own README.md) are neither replaced nor swept away.
if ((Test-Path -LiteralPath $InstallDir -PathType Container) -and -not (Test-Installation $InstallDir) -and
    (Get-ChildItem -LiteralPath $InstallDir -Force | Select-Object -First 1)) {
    Exit-Install "$InstallDir holds files but no pult install: nothing was installed; use an empty folder (-InstallDir) or none"
}

# Everything in the payload but this script goes.
$files = @($payloadFiles | Where-Object { $_ -cne 'install.ps1' })
$stillRunning = @(Get-Process -Name $programs -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and (Test-Under $_.Path $InstallDir) })

Write-Output "==> copying pult $version to $InstallDir"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
# All files are copied to .new first and renamed only when every copy succeeded: a copy that fails
# leaves the old install whole, and its .new files are taken away.
try {
    foreach ($name in $files) {
        Copy-Item -LiteralPath (Join-Path $PayloadDir $name) -Destination (Join-Path $InstallDir "$name.new") -Force
    }
}
catch {
    foreach ($name in $files) {
        Remove-Item -LiteralPath (Join-Path $InstallDir "$name.new") -Force -ErrorAction SilentlyContinue
    }
    Exit-Install "copying the files failed, the installed ones are unchanged: $($_.Exception.Message)"
}
# Then each old file is moved aside to .old and its .new takes its place. A rename that fails (a
# scanner holding a file) puts every file swapped so far back, so the install is again all old, never
# a mix of versions.
$swapped = New-Object System.Collections.Generic.List[object]
try {
    foreach ($name in $files) {
        $dest = Join-Path $InstallDir $name
        $hadOld = Test-Path -LiteralPath $dest
        if ($hadOld) { Move-Item -LiteralPath $dest -Destination "$dest.old" -Force }
        $step = [pscustomobject]@{ Dest = $dest; HadOld = $hadOld; Placed = $false }
        $swapped.Add($step)
        Move-Item -LiteralPath "$dest.new" -Destination $dest -Force
        $step.Placed = $true
    }
}
catch {
    $why = $_.Exception.Message
    for ($i = $swapped.Count - 1; $i -ge 0; $i--) {
        $s = $swapped[$i]
        if ($s.Placed) { Remove-Item -LiteralPath $s.Dest -Force -ErrorAction SilentlyContinue }
        if ($s.HadOld) { Move-Item -LiteralPath "$($s.Dest).old" -Destination $s.Dest -Force -ErrorAction SilentlyContinue }
    }
    foreach ($name in $files) {
        Remove-Item -LiteralPath (Join-Path $InstallDir "$name.new") -Force -ErrorAction SilentlyContinue
    }
    # A pult that kept running through an earlier install holds its .old, which this install needs.
    if ($stillRunning) { Exit-Install "pult runs (PID $(($stillRunning | ForEach-Object { $_.Id }) -join ', ')) from a program an earlier install left: close it and run this again; the installed files were put back: $why" }
    Exit-Install "replacing the files failed, the installed ones were put back: $why"
}
# An .old file still held by a running pult stays; the next install or uninstall.ps1 takes it away.
foreach ($name in $files) {
    Remove-Item -LiteralPath (Join-Path $InstallDir "$name.old") -Force -ErrorAction SilentlyContinue
}
$app = Join-Path $InstallDir 'pult-app.exe'

Write-Output "==> Start menu entry in $StartMenuDir"
New-Item -ItemType Directory -Force -Path $StartMenuDir | Out-Null
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut((Join-Path $StartMenuDir 'pult.lnk'))
$link.TargetPath = $app
$link.WorkingDirectory = $InstallDir
$link.IconLocation = "$app,0"
$link.Description = 'pult: the personal virtual audio mixer'
$link.Save()

# The entry of Settings > Apps runs the copy of uninstall.ps1 with the places of this install.
$uninstallKey = "$RegistryRoot\Microsoft\Windows\CurrentVersion\Uninstall\pult"
Write-Output "==> Settings > Apps entry ($uninstallKey)"
$ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
# -PauseOnError: Settings opens a console window that would close before an error could be read.
$command = "`"$ps`" -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $InstallDir 'uninstall.ps1')`"" +
" -InstallDir `"$InstallDir`" -StartMenuDir `"$StartMenuDir`" -RegistryRoot `"$RegistryRoot`" -PauseOnError"
$size = [int](((Get-ChildItem -LiteralPath $InstallDir -File | Measure-Object -Property Length -Sum).Sum) / 1KB)
# There is one entry: an install in another folder loses it (its files stay there), so say so.
$previous = Get-ItemProperty -LiteralPath $uninstallKey -ErrorAction SilentlyContinue
$previousDir = ''
if ($previous -and $previous.PSObject.Properties['InstallLocation']) { $previousDir = ([string]$previous.InstallLocation).TrimEnd('\') }
if ($previousDir -and $previousDir -ne $InstallDir) {
    Write-Output "the Settings > Apps entry was for the install in $previousDir; it now names this one, that copy stays:"
    Write-Output "  remove it with: & `"$(Join-Path $previousDir 'uninstall.ps1')`" -InstallDir `"$previousDir`""
}
if (Test-Path -LiteralPath $uninstallKey) { Remove-Item -LiteralPath $uninstallKey -Recurse -Force }
New-Item -Path $uninstallKey -Force | Out-Null
$values = [ordered]@{
    DisplayName     = 'pult'
    DisplayVersion  = [string]$version
    Publisher       = 'WKDSteve'
    InstallLocation = $InstallDir
    DisplayIcon     = "$app,0"
    UninstallString = $command
}
foreach ($k in $values.Keys) {
    New-ItemProperty -LiteralPath $uninstallKey -Name $k -Value $values[$k] -PropertyType String -Force | Out-Null
}
foreach ($k in 'NoModify', 'NoRepair') {
    New-ItemProperty -LiteralPath $uninstallKey -Name $k -Value 1 -PropertyType DWord -Force | Out-Null
}
New-ItemProperty -LiteralPath $uninstallKey -Name 'EstimatedSize' -Value $size -PropertyType DWord -Force | Out-Null

# The start with Windows stays what you chose, in the tray or with the window open. When it is on, it
# starts this install from now on (it may name a program that moved, or a build in a checkout). A value
# in any other form is not ours.
$runKey = "$RegistryRoot\Microsoft\Windows\CurrentVersion\Run"
$run = Get-ItemProperty -LiteralPath $runKey -Name 'pult' -ErrorAction SilentlyContinue
if ($run) {
    $how = if ([string]$run.pult -match ' --tray$') { ' --tray' } else { '' }
    $wanted = "`"$app`"$how"
    if ([string]$run.pult -ceq $wanted) {
        Write-Output 'pult starts with Windows: unchanged'
    }
    elseif ([string]$run.pult -match '^"[^"]*pult-app\.exe"( --tray)?$') {
        Set-ItemProperty -LiteralPath $runKey -Name 'pult' -Value $wanted
        Write-Output "pult starts with Windows: now from $app (it was $($run.pult))"
    }
    else {
        Write-Output "left the Run value 'pult' alone, it is not one pult wrote: $($run.pult)"
    }
}

Write-Output ''
Write-Output "installed pult $version to $InstallDir"
Write-Output 'your profiles and %APPDATA%\pult were not touched'
if ($stillRunning) {
    $names = ($stillRunning | ForEach-Object { "$($_.Name) (PID $($_.Id))" }) -join ', '
    Write-Output "still running with the old program until closed: $names"
}
