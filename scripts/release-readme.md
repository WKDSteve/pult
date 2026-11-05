# pult: install from this zip

This is the Windows release of pult, the personal virtual audio mixer. It is a zip with install
scripts, not a setup.exe. It is not signed (see "SmartScreen" below), and it is meant for private
machines, not for a managed workstation.

## What you need

- Windows 10 or 11 (64 bit).
- For the sound of all programs at once, a virtual cable such as VB-CABLE, which you install yourself:
  it does not belong to pult and is not in this zip. A single program can be a source without one.

## Check and unblock the download

Do this in the folder you downloaded the zip to, **before you unpack it**. Compare the hash with the
line for this zip in `SHA256SUMS` of the same release:

```powershell
(Get-FileHash .\pult-*.zip -Algorithm SHA256).Hash.ToLower()
Unblock-File .\pult-*.zip      # removes the Mark-of-the-Web; what you unpack from it carries none
```

## Install

Unpack the zip into a **new, empty folder** (the installer stops when it finds files in it that a
release does not have). If you unpacked it before unblocking it, run `Get-ChildItem | Unblock-File` in
the unpacked folder now: the installer copies the marks along to the installed programs, and Windows
would stop their first start (see "SmartScreen"). Then, in a normal (not elevated) PowerShell in that
folder:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
```

It installs for the current user, without administrator rights:

| What | Where |
|---|---|
| `pult.exe` (command line), `pult-app.exe` (window) | `%LOCALAPPDATA%\pult\bin` |
| `uninstall.ps1`, `example-profile.toml`, `LICENSE`, `README.md`, `VERSION` | the same folder |
| Start menu entry *pult*, entry in *Settings > Apps* | your Start menu, your registry |

Your profiles and `%APPDATA%\pult` (settings, logs) are never touched. Installing again replaces the
files; a pult that runs goes on with the old program until you close it, so the sound does not stop.
It refuses a folder (`-InstallDir`) that holds files of somebody else's and no pult install.

## First start

pult plays what a profile says. Copy `%LOCALAPPDATA%\pult\bin\example-profile.toml` to a place of your
own, name your devices in it (`pult devices` lists them), and open it once:

```powershell
& "$env:LOCALAPPDATA\pult\bin\pult-app.exe" "$HOME\pult\studio.toml"
```

The window remembers it and opens it the next time, from the Start menu too. Do not edit the copy in
the install folder: the next install replaces it. Without a profile the window offers a demo desk.

Closing the window leaves pult in the tray and the sound running; *Quit pult* in the tray menu ends
it. *Start with Windows* in the same menu is off until you turn it on. *Options* in the window has it
too, and there you choose whether pult then starts in the background (in the tray) or with its window
open.

## SmartScreen

Nothing here is code-signed. A zip you downloaded in a browser carries a Mark-of-the-Web, and so does
everything you unpack from it with Explorer. `-ExecutionPolicy Bypass` runs the scripts regardless, but
the first start of `pult-app.exe` can be stopped ("Windows protected your PC": *More info*, then *Run
anyway*) if the mark is still on the file. That is why the zip is unblocked before it is unpacked,
above. The checksum tells you the zip is the release's; it is no signature.

## Uninstall

*Settings > Apps > pult*, or:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\pult\bin\uninstall.ps1"
```

Close pult first (*Quit pult* in the tray menu). Your profiles and `%APPDATA%\pult` stay; delete that
folder by hand if you want the settings gone too.
