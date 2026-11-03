<#
.SYNOPSIS
Remove pult for the current user on Windows, without administrator rights.

.DESCRIPTION
Removes what scripts/build-install.ps1 installed: pult.exe and pult-app.exe in the install folder
(%LOCALAPPDATA%\pult\bin, or $env:PULT_INSTALL_DIR) and the Start menu entry "pult". Your profiles
are never touched. A second run finds nothing to do and says so; it is not an error. A program that
still runs cannot be removed: that is an error, not a "removed".

.PARAMETER AllowElevated
Remove although the session is elevated (for a build machine).

.EXAMPLE
.\scripts\uninstall.ps1
#>
[CmdletBinding()]
param(
    [switch] $AllowElevated
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Exit-Uninstall([string] $Message) {
    throw "uninstall.ps1: $Message"
}

if ($env:CLAUDECODE) {
    Exit-Uninstall 'remove in a normal terminal, not in a Claude session (the app redirects AppData and the registry)'
}
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $AllowElevated -and $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Exit-Uninstall 'do not run this elevated: it removes the installation of one user (-AllowElevated for a build machine)'
}

$dir = if ($env:PULT_INSTALL_DIR) { $env:PULT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'pult\bin' }
$menu = Join-Path ([Environment]::GetFolderPath('Programs')) 'pult.lnk'
$files = @('pult.exe', 'pult-app.exe', 'pult.exe.new', 'pult-app.exe.new', 'pult.exe.old', 'pult-app.exe.old') | ForEach-Object { Join-Path $dir $_ }
$present = @($files + $menu | Where-Object { Test-Path -LiteralPath $_ })
if ($present.Count -eq 0) {
    Write-Output "nothing to remove: no pult in $dir and no Start menu entry"
    return
}

# Only the files the installer puts there; a folder that holds anything else stays.
foreach ($file in $present) {
    try { Remove-Item -LiteralPath $file -Force }
    catch { Exit-Uninstall "cannot remove ${file} (is pult running? close it and run again): $($_.Exception.Message)" }
    if (Test-Path -LiteralPath $file) { Exit-Uninstall "$file is still there" }
    Write-Output "removed $file"
}
if ((Test-Path -LiteralPath $dir) -and -not (Get-ChildItem -LiteralPath $dir -Force)) {
    Remove-Item -LiteralPath $dir -Force -ErrorAction SilentlyContinue
    # The parent only when it is the folder the installer makes (never a folder you chose).
    $parent = Split-Path -Parent $dir
    if ((Split-Path -Leaf $dir) -eq 'bin' -and (Split-Path -Leaf $parent) -eq 'pult' -and -not (Get-ChildItem -LiteralPath $parent -Force)) {
        Remove-Item -LiteralPath $parent -Force -ErrorAction SilentlyContinue
    }
}
Write-Output 'your profiles were not touched'
