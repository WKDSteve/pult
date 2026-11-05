<#
.SYNOPSIS
Test scripts/package-release.ps1, the release zip it makes and its install and uninstall, without a
build and without a real install.

.DESCRIPTION
Packs the release zip from stand-ins (copies of cmd.exe: nothing is built) and checks:

- the zip holds exactly the two programs, the scripts, the example profile, LICENSE, README.md and
  VERSION, with the bytes of the sources;
- SHA256SUMS names the zip with its real SHA-256, and packing twice gives the same bytes;
- the file names and version labels of a release and of the rolling build, and what is refused (a
  version that is not the one of Cargo.toml, a rolling build without a commit, a missing or empty
  program);
- the zip unpacked into a folder and its install.ps1 run into temp places (a throw-away registry root,
  a temp Start menu and install folder; the account's own are never touched): the Settings > Apps
  entry, the Run value, a pult that keeps running through an update, a swap that fails, folders that
  are no release and no install; then the installed uninstall.ps1.

With -Zip the same checks run on that zip too, against the programs of -BuildDir (the workflows check
the zip they are about to publish this way). The real programs are only compared, never started.

Exit 0 when all checks pass.

.PARAMETER Zip
A release zip made from -BuildDir, checked as well.

.PARAMETER BuildDir
The folder with the pult.exe and pult-app.exe the -Zip was made from.

.PARAMETER Label
The version label the -Zip must say (X.Y.Z, or X.Y.Z+<short commit> for the rolling build). Without it
the zip's own VERSION is believed, which checks less.
#>
[CmdletBinding()]
param(
    [string] $Zip = '',
    [string] $BuildDir = '',
    [string] $Label = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$here = $PSScriptRoot
$root = Split-Path -Parent $here
$id = [Guid]::NewGuid().ToString('N')
# A space and a non-ASCII letter in every place: both have broken install scripts before.
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("pult test-release $([char]0xFC)-$id")
$regRoot = "HKCU:\Software\pult-test-release-$id"
New-Item -ItemType Directory -Path $tmp | Out-Null

$script:failures = 0
function Write-Check([bool] $Passed, [string] $Name) {
    if ($Passed) { Write-Output "ok   $Name" }
    else { Write-Output "FAIL $Name"; $script:failures++ }
}

$packager = Join-Path $here 'package-release.ps1'
$programs = 'pult', 'pult-app'
$entries = @('LICENSE', 'README.md', 'VERSION', 'example-profile.toml', 'install.ps1', 'pult-app.exe', 'pult.exe', 'uninstall.ps1')
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$elevated = ([Security.Principal.WindowsPrincipal] $identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'

# The packager in this process: whether it ended, and what it said.
function Invoke-Package([hashtable] $Arguments, [string] $Script = $packager) {
    try {
        $out = @(& $Script @Arguments)
        return [pscustomobject]@{ Ok = $true; Text = ($out -join "`n") }
    }
    catch {
        return [pscustomobject]@{ Ok = $false; Text = $_.Exception.Message }
    }
}

# A script in a child PowerShell with the places of the test (the child runs without CLAUDECODE).
function Invoke-Child([string] $Script, [string[]] $Arguments, [string] $Shell = '') {
    if (-not $Shell) { $Shell = (Get-Process -Id $PID).Path }
    $extra = @()
    if ($elevated) { $extra = @('-AllowElevated') }
    $saved = $env:CLAUDECODE
    $ErrorActionPreference = 'Continue'
    try {
        $env:CLAUDECODE = $null
        $output = & $Shell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $Script @Arguments @extra 2>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally {
        $env:CLAUDECODE = $saved
    }
    return [pscustomobject]@{ Code = $code; Text = $output }
}

function Get-Sha([string] $Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

# The names of a folder's files, sorted the way the zip sorts them.
function Get-Tree([string] $Dir) {
    $names = @(Get-ChildItem -LiteralPath $Dir -Force -File | ForEach-Object { $_.Name })
    [Array]::Sort($names, [StringComparer]::Ordinal)
    return ($names -join '|')
}

function Get-ZipEntryName([string] $Path) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($Path)
    try { return @($archive.Entries | ForEach-Object { $_.FullName }) }
    finally { $archive.Dispose() }
}

# The checks every release zip must pass, against the programs it was made from. Where it was unpacked
# is left in $script:archiveDir: what a function writes is its result, and the checks write.
function Test-Archive([string] $Path, [string] $Programs, [string] $Label, [string] $Title) {
    $names = Get-ZipEntryName $Path
    Write-Check (($names -join '|') -ceq ($entries -join '|')) "$Title holds exactly the programs, the scripts, the profile, LICENSE, README.md and VERSION, in ordinal order"
    if (($names -join '|') -cne ($entries -join '|')) { Write-Output "  it holds: $($names -join ', ')" }
    $dir = Join-Path $tmp ('unpacked-' + [Guid]::NewGuid().ToString('N'))
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($Path, $dir)
    $same = $true
    foreach ($name in 'pult', 'pult-app') {
        if ((Get-Sha (Join-Path $dir "$name.exe")) -ne (Get-Sha (Join-Path $Programs "$name.exe"))) { $same = $false }
    }
    Write-Check $same "$Title carries the bytes of the programs, each under its name"
    $sources = @{
        'install.ps1'          = (Join-Path $here 'install.ps1')
        'uninstall.ps1'        = (Join-Path $here 'uninstall.ps1')
        'example-profile.toml' = (Join-Path $root 'examples\pult.toml')
        'LICENSE'              = (Join-Path $root 'LICENSE')
        'README.md'            = (Join-Path $here 'release-readme.md')
    }
    $sameSources = $true
    foreach ($key in $sources.Keys) {
        if ((Get-Sha (Join-Path $dir $key)) -ne (Get-Sha $sources[$key])) { $sameSources = $false }
    }
    Write-Check $sameSources "$Title carries the scripts, the profile, LICENSE and README.md of the checkout unchanged"
    Write-Check ((Get-Content -LiteralPath (Join-Path $dir 'VERSION') -Raw) -ceq "$Label`n") "$Title says its version is $Label"
    $script:archiveDir = $dir
}

$envNames = 'PULT_INSTALL_DIR', 'PULT_START_MENU_DIR', 'PULT_REGISTRY_ROOT'
$savedEnv = @{}
foreach ($name in $envNames) { $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name) }
$keepRunning = $null
try {
    # Whatever a call forgets to pass, it must not reach the real folders or keys of this account.
    $install = Join-Path $tmp 'install'
    $startMenu = Join-Path $tmp 'start-menu'
    [Environment]::SetEnvironmentVariable('PULT_INSTALL_DIR', $install)
    [Environment]::SetEnvironmentVariable('PULT_START_MENU_DIR', $startMenu)
    [Environment]::SetEnvironmentVariable('PULT_REGISTRY_ROOT', $regRoot)

    $standIns = Join-Path $tmp 'build'
    New-Item -ItemType Directory -Path $standIns | Out-Null
    foreach ($name in $programs) {
        Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\cmd.exe') -Destination (Join-Path $standIns "$name.exe")
        # Two different programs, so a zip that packs one for the other is seen.
        $extra = [Text.Encoding]::ASCII.GetBytes("stand-in $name")
        $exeStream = [IO.File]::Open((Join-Path $standIns "$name.exe"), [IO.FileMode]::Append)
        try { $exeStream.Write($extra, 0, $extra.Length) } finally { $exeStream.Dispose() }
    }
    $cargo = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
    $null = $cargo -match '(?m)^version\s*=\s*"([^"]+)"'
    $version = $Matches[1]

    # 1. A release zip.
    $out1 = Join-Path $tmp 'dist one'
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = $out1 }
    Write-Check $r.Ok 'package a release'
    if (-not $r.Ok) { Write-Output $r.Text }
    $zipName = "pult-$version-x86_64-pc-windows-msvc.zip"
    $zip1 = Join-Path $out1 $zipName
    Write-Check (Test-Path -LiteralPath $zip1) "the file is called $zipName"
    Write-Check (@(Get-ChildItem -LiteralPath $out1 -File).Count -eq 2) 'beside it only SHA256SUMS'
    Test-Archive $zip1 $standIns $version 'the release zip'
    $dir1 = $script:archiveDir
    $sums = (Get-Content -LiteralPath (Join-Path $out1 'SHA256SUMS') -Raw)
    Write-Check ($sums -ceq ("$(Get-Sha $zip1)  $zipName`n")) 'SHA256SUMS names the zip with its real SHA-256 ("<hash>  <file>")'

    # 2. The same programs give the same bytes.
    $out2 = Join-Path $tmp 'dist two'
    # Zip times have a resolution of two seconds: wait longer, so a zip that took the clock would differ.
    Start-Sleep -Seconds 3
    $null = Invoke-Package @{ BuildDir = $standIns; OutDir = $out2 }
    Write-Check ((Get-Sha $zip1) -eq (Get-Sha (Join-Path $out2 $zipName))) 'packing twice gives the same zip, byte for byte'

    # 3. The rolling build: a fixed name, the short commit in the version label.
    $sha = '0123456789abcdef0123456789abcdef01234567'
    $out3 = Join-Path $tmp 'dist rolling'
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = $out3; Rolling = $true; Sha = $sha }
    $zip3 = Join-Path $out3 'pult-windows-x86_64.zip'
    Write-Check ($r.Ok -and (Test-Path -LiteralPath $zip3)) 'package the rolling build under its fixed name'
    if ($r.Ok) { Test-Archive $zip3 $standIns "$version+0123456" 'the rolling zip' }
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = $out3; Rolling = $true }
    Write-Check (-not $r.Ok -and $r.Text -match 'needs -Sha') 'a rolling build without a commit is refused'
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = $out3; Rolling = $true; Sha = 'ABCDEF1' }
    Write-Check (-not $r.Ok -and $r.Text -match 'lower case hex') 'and so is a commit that is not lower case hex'
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = $out3; Sha = $sha }
    Write-Check (-not $r.Ok -and $r.Text -match 'belongs to -Rolling') 'a commit on a release is refused'

    # 4. The version a release says it is.
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = (Join-Path $tmp 'dist v'); Version = "v$version" }
    Write-Check $r.Ok 'the tag form vX.Y.Z of the current version is accepted'
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = (Join-Path $tmp 'dist v2'); Version = '9.9.9' }
    Write-Check (-not $r.Ok -and $r.Text -match 'Cargo.toml says') 'a version that is not the one of Cargo.toml is refused'
    Write-Check (-not (Test-Path -LiteralPath (Join-Path $tmp 'dist v2'))) 'and nothing is written for it'

    # 5. Programs that are not there.
    $partial = Join-Path $tmp 'partial'
    New-Item -ItemType Directory -Path $partial | Out-Null
    Copy-Item -LiteralPath (Join-Path $standIns 'pult.exe') -Destination $partial
    $r = Invoke-Package @{ BuildDir = $partial; OutDir = (Join-Path $tmp 'dist p') }
    Write-Check (-not $r.Ok -and $r.Text -match 'pult-app.exe') 'a missing program is refused, and named'
    Set-Content -LiteralPath (Join-Path $partial 'pult-app.exe') -Value '' -NoNewline
    $r = Invoke-Package @{ BuildDir = $partial; OutDir = (Join-Path $tmp 'dist p') }
    Write-Check (-not $r.Ok -and $r.Text -match 'is empty') 'an empty program is refused'

    # 6. SHA256SUMS keeps one line per zip of the folder, and never the hash of a zip that was not made.
    $both = Join-Path $tmp 'dist both'
    $null = Invoke-Package @{ BuildDir = $standIns; OutDir = $both }
    $null = Invoke-Package @{ BuildDir = $standIns; OutDir = $both; Rolling = $true; Sha = $sha }
    $null = Invoke-Package @{ BuildDir = $standIns; OutDir = $both; Rolling = $true; Sha = $sha }
    $lines = @((Get-Content -LiteralPath (Join-Path $both 'SHA256SUMS')) | Where-Object { $_ })
    Write-Check ($lines.Count -eq 2 -and ($lines -join '|') -match [regex]::Escape($zipName) -and ($lines -join '|') -match 'pult-windows-x86_64.zip') 'a release and a rolling zip in one folder give two lines in SHA256SUMS (packing again replaces a line)'
    $locked = [IO.File]::Open((Join-Path $both $zipName), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
    try { $r = Invoke-Package @{ BuildDir = $standIns; OutDir = $both } }
    finally { $locked.Dispose() }
    $left = (Get-Content -LiteralPath (Join-Path $both 'SHA256SUMS') -Raw)
    Write-Check (-not $r.Ok -and $left -notmatch [regex]::Escape($zipName) -and $left -match 'pult-windows-x86_64.zip') 'a zip that cannot be written leaves no hash of its old self in SHA256SUMS'

    # 7. -StageDir / -NoZip, as build-install.ps1 uses them.
    $stage = Join-Path $tmp 'stage'
    $r = Invoke-Package @{ BuildDir = $standIns; StageDir = $stage; NoZip = $true }
    Write-Check ($r.Ok -and (Test-Path -LiteralPath (Join-Path $stage 'pult-app.exe')) -and -not (Test-Path -LiteralPath (Join-Path $stage 'SHA256SUMS'))) '-NoZip stages the folder and makes no zip'
    $r = Invoke-Package @{ BuildDir = $standIns; StageDir = $stage; NoZip = $true }
    Write-Check (-not $r.Ok -and $r.Text -match 'not empty') 'a stage folder that holds something is refused'
    $r = Invoke-Package @{ BuildDir = $standIns; NoZip = $true }
    Write-Check (-not $r.Ok -and $r.Text -match 'give -StageDir') '-NoZip without a stage folder is refused'

    # 8. The unpacked zip installs (Windows PowerShell 5.1 runs it: the one Windows has).
    $places = @('-InstallDir', $install, '-StartMenuDir', $startMenu, '-RegistryRoot', $regRoot)
    $installer = Join-Path $dir1 'install.ps1'
    $r = Invoke-Child $installer $places -Shell $windowsPowerShell
    Write-Check ($r.Code -eq 0) 'install.ps1 of the unpacked zip installs'
    if ($r.Code -ne 0) { Write-Output $r.Text }
    $wanted = ($entries | Where-Object { $_ -cne 'install.ps1' }) -join '|'
    Write-Check ((Get-Tree $install) -ceq $wanted) 'the install holds exactly the zip but install.ps1, in one folder'
    if ((Get-Tree $install) -cne $wanted) { Write-Output "  it holds: $(Get-Tree $install)" }
    $entryKey = "$regRoot\Microsoft\Windows\CurrentVersion\Uninstall\pult"
    $entry = Get-ItemProperty -LiteralPath $entryKey -ErrorAction SilentlyContinue
    Write-Check ($null -ne $entry -and $entry.DisplayVersion -eq $version -and $entry.InstallLocation -eq $install) 'the Settings > Apps entry says the version of the zip'
    Write-Check ($null -ne $entry -and $entry.UninstallString -match [regex]::Escape("-InstallDir `"$install`"") -and $entry.UninstallString -match 'PauseOnError') 'and its command names the places of this install'
    $link = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $startMenu 'pult.lnk'))
    Write-Check ($link.TargetPath -eq (Join-Path $install 'pult-app.exe')) 'the Start menu entry opens the window'
    $runKey = "$regRoot\Microsoft\Windows\CurrentVersion\Run"
    Write-Check (-not (Test-Path -LiteralPath $runKey)) 'the start with Windows is not turned on by an install'

    # 9. The start with Windows stays what you chose, and starts this install from now on.
    New-Item -Path $runKey -Force | Out-Null
    New-ItemProperty -LiteralPath $runKey -Name 'pult' -Value '"C:\moved\away\pult-app.exe" --tray' -PropertyType String -Force | Out-Null
    $r = Invoke-Child $installer $places
    $app = Join-Path $install 'pult-app.exe'
    Write-Check ((Get-ItemProperty -LiteralPath $runKey).pult -ceq "`"$app`" --tray") 'a Run value of pult-app elsewhere now starts this install'
    Set-ItemProperty -LiteralPath $runKey -Name 'pult' -Value 'cmd /c something-of-yours'
    $r = Invoke-Child $installer $places
    Write-Check ((Get-ItemProperty -LiteralPath $runKey).pult -ceq 'cmd /c something-of-yours' -and $r.Text -match 'left the Run value') 'a Run value in another form is left alone, and named'
    $r = Invoke-Child (Join-Path $install 'uninstall.ps1') $places
    Write-Check ((Get-ItemProperty -LiteralPath $runKey).pult -ceq 'cmd /c something-of-yours') 'and the uninstall leaves it too'
    Remove-ItemProperty -LiteralPath $runKey -Name 'pult'
    $r = Invoke-Child $installer $places

    # 10. An update while pult runs: the program goes on from the old file, the new one is in place.
    Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\cmd.exe') -Destination (Join-Path $install 'pult.exe') -Force
    $keepRunning = Start-Process -FilePath (Join-Path $install 'pult.exe') -ArgumentList '/c', 'ping -n 60 127.0.0.1 >nul' -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 300
    $r = Invoke-Child $installer $places
    Write-Check ($r.Code -eq 0 -and $r.Text -match 'still running with the old program') 'an install while pult runs goes through and says so'
    if ($r.Code -ne 0) { Write-Output $r.Text }
    Write-Check ((Get-Sha (Join-Path $install 'pult.exe')) -eq (Get-Sha (Join-Path $standIns 'pult.exe'))) 'the new program is in place'
    Write-Check (Test-Path -LiteralPath (Join-Path $install 'pult.exe.old')) 'the old one stays while it runs'
    $r = Invoke-Child (Join-Path $install 'uninstall.ps1') $places
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'running from' -and (Test-Path -LiteralPath (Join-Path $install 'pult.exe'))) 'an uninstall while pult runs is refused before anything goes'
    if ($r.Code -eq 0) { Write-Output $r.Text }
    Stop-Process -Id $keepRunning.Id -Force -ErrorAction SilentlyContinue
    $keepRunning.WaitForExit(5000) | Out-Null
    $keepRunning = $null
    Start-Sleep -Milliseconds 300

    # 11. A swap that fails puts the old files back, and leaves no .new behind.
    $r = Invoke-Child $installer $places
    Write-Check ($r.Code -eq 0 -and (Get-Tree $install) -ceq $wanted) 'the next install takes the .old away'
    if ((Get-Tree $install) -cne $wanted) { Write-Output "  it holds: $(Get-Tree $install)" }
    $newer = Join-Path $tmp 'newer'
    Copy-Item -LiteralPath $dir1 -Destination $newer -Recurse
    # pult.exe is swapped before uninstall.ps1, the file that is held.
    Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\cmd.exe') -Destination (Join-Path $newer 'pult.exe') -Force
    $oldProgram = Get-Sha (Join-Path $install 'pult.exe')
    $held = [IO.File]::Open((Join-Path $install 'uninstall.ps1'), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
    try { $r = Invoke-Child (Join-Path $newer 'install.ps1') $places }
    finally { $held.Dispose() }
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'put back') 'a swap that fails is an error'
    Write-Check ((Get-Sha (Join-Path $install 'pult.exe')) -eq $oldProgram) 'and the files swapped before it are the old ones again'
    Write-Check ((Get-Tree $install) -ceq $wanted) 'with no .new or .old left'
    if ((Get-Tree $install) -cne $wanted) { Write-Output "  it holds: $(Get-Tree $install)" }

    # 12. Folders that are no release, and folders that are no install.
    $bad = Join-Path $tmp 'bad payload'
    Copy-Item -LiteralPath $dir1 -Destination $bad -Recurse
    Set-Content -LiteralPath (Join-Path $bad 'other.txt') -Value 'x'
    $r = Invoke-Child (Join-Path $bad 'install.ps1') $places
    Write-Check ($r.Code -ne 0 -and $r.Text -match "other.txt") 'a folder with a file a release does not have is refused'
    Remove-Item -LiteralPath (Join-Path $bad 'other.txt')
    Remove-Item -LiteralPath (Join-Path $bad 'pult-app.exe')
    $r = Invoke-Child (Join-Path $bad 'install.ps1') $places
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'no pult-app.exe') 'a folder without a program is refused'
    Copy-Item -LiteralPath (Join-Path $standIns 'pult-app.exe') -Destination $bad
    Set-Content -LiteralPath (Join-Path $bad 'VERSION') -Value 'not a version'
    $r = Invoke-Child (Join-Path $bad 'install.ps1') $places
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'not a version') 'a VERSION that is no version is refused'
    $foreign = Join-Path $tmp 'foreign'
    New-Item -ItemType Directory -Path $foreign | Out-Null
    Set-Content -LiteralPath (Join-Path $foreign 'README.md') -Value 'mine'
    $r = Invoke-Child $installer @('-InstallDir', $foreign, '-StartMenuDir', $startMenu, '-RegistryRoot', $regRoot)
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'no pult install' -and (Get-Tree $foreign) -ceq 'README.md') 'a folder with files of somebody else is refused and left alone'
    $r = Invoke-Child (Join-Path $install 'uninstall.ps1') @('-InstallDir', $foreign, '-StartMenuDir', $startMenu, '-RegistryRoot', $regRoot)
    Write-Check ((Get-Tree $foreign) -ceq 'README.md' -and (Test-Path -LiteralPath $entryKey)) 'an uninstall of that folder takes nothing, and leaves the entry of the real install'
    $r = Invoke-Child $installer @('-InstallDir', 'C:\', '-StartMenuDir', $startMenu, '-RegistryRoot', $regRoot)
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'drive root') 'a drive root is refused'
    $r = Invoke-Child $installer @('-InstallDir', $install, '-StartMenuDir', $startMenu, '-RegistryRoot', 'HKLM:\Software\x')
    Write-Check ($r.Code -ne 0 -and $r.Text -match 'below HKCU') 'a registry root outside HKCU is refused'

    # 13. In a Claude session it installs nothing (AppData and HKCU are redirected there).
    $savedClaude = $env:CLAUDECODE
    $env:CLAUDECODE = '1'
    $r = Invoke-Package @{ BuildDir = $standIns; OutDir = (Join-Path $tmp 'dist c') }
    Write-Check $r.Ok 'packing works in a Claude session'
    try {
        $null = & $installer -InstallDir $install -StartMenuDir $startMenu -RegistryRoot $regRoot
        $refused = $false
    }
    catch {
        $refused = $_.Exception.Message -match 'not in a Claude session'
    }
    finally {
        $env:CLAUDECODE = $savedClaude
    }
    Write-Check $refused 'install.ps1 refuses a Claude session'

    # 14. The uninstall: a Run value of this install goes with it, everything else stays.
    New-Item -Path $runKey -Force | Out-Null
    New-ItemProperty -LiteralPath $runKey -Name 'pult' -Value "`"$app`" --tray" -PropertyType String -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $install 'pult.exe.new') -Value 'cut short'
    $r = Invoke-Child (Join-Path $install 'uninstall.ps1') $places
    Write-Check ($r.Code -eq 0 -and -not (Test-Path -LiteralPath $install) -and -not (Test-Path -LiteralPath (Join-Path $startMenu 'pult.lnk'))) 'the installed uninstall.ps1 removes it all, with the rest of a cut short install'
    if ($r.Code -ne 0) { Write-Output $r.Text }
    Write-Check (-not (Test-Path -LiteralPath $entryKey)) 'and the Settings > Apps entry'
    Write-Check ($null -eq (Get-ItemProperty -LiteralPath $runKey -Name 'pult' -ErrorAction SilentlyContinue)) 'and the start with Windows of this install'
    $r = Invoke-Child (Join-Path $here 'uninstall.ps1') $places
    Write-Check ($r.Code -eq 0 -and $r.Text -match 'nothing to remove') 'a second run finds nothing to do and says so'

    # 15. An earlier install of build-install.ps1 (programs only) is an install.
    New-Item -ItemType Directory -Path $install | Out-Null
    Copy-Item -LiteralPath (Join-Path $standIns 'pult.exe') -Destination $install
    $r = Invoke-Child $installer $places
    Write-Check ($r.Code -eq 0 -and (Get-Tree $install) -ceq $wanted) 'an install of programs only is taken over'
    $r = Invoke-Child (Join-Path $install 'uninstall.ps1') $places

    # 16. The zip the workflow is about to publish.
    if ($Zip) {
        if (-not (Test-Path -LiteralPath $Zip -PathType Leaf)) { throw "no such zip: $Zip" }
        if (-not $BuildDir) { throw '-Zip needs -BuildDir' }
        if (-not $Label) { throw '-Zip needs -Label' }
        Test-Archive $Zip $BuildDir $Label "the zip $(Split-Path -Leaf $Zip)"
    }
}
finally {
    if ($keepRunning) { Stop-Process -Id $keepRunning.Id -Force -ErrorAction SilentlyContinue }
    foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name]) }
    if (Test-Path -LiteralPath $regRoot) { Remove-Item -LiteralPath $regRoot -Recurse -Force }
    for ($attempt = 1; Test-Path -LiteralPath $tmp; $attempt++) {
        try {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction Stop
        }
        catch {
            if ($attempt -ge 10) {
                Write-Output "FAIL could not remove $tmp : $($_.Exception.Message)"
                $script:failures++
                break
            }
            Start-Sleep -Milliseconds 200
        }
    }
}

if ($script:failures -ne 0) {
    Write-Output ''
    Write-Output "$($script:failures) check(s) failed"
    exit 1
}
Write-Output ''
Write-Output 'all checks passed'
