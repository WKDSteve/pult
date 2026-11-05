pult {tag} for Windows. Unsigned, for private machines only: not for the Tricentis workstation.

**This is a zip with install scripts, not a setup.exe.**

1. Download `pult-<version>-x86_64-pc-windows-msvc.zip` (the repository is private, so use `gh release download {tag}` or the page while signed in).
   Before you unpack it, check it against `SHA256SUMS` (`(Get-FileHash <zip> -Algorithm SHA256).Hash.ToLower()` must be the hash in that file)
   and run `Unblock-File <zip>`, so the installed programs carry no Mark-of-the-Web.
2. Unpack it into a new, empty folder, open a normal (not elevated) PowerShell in it and run
   `powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1`.
3. Open *pult* from the Start menu. It plays what a profile says: `example-profile.toml` in `%LOCALAPPDATA%\pult\bin` is the start for your own copy.

SmartScreen may stop the first start of an unsigned program ("More info", then "Run anyway").
Update: run the new zip's `install.ps1` (a pult that runs goes on with the old program until you close it). Uninstall: *Settings > Apps > pult*.
Details: the `README.md` inside the zip.
