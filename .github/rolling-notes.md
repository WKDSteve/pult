The rolling build of the newest green commit of `main`, for looking at the state of the work. Not a release, unsigned, replaced after every green CI run.
For private machines only: not for the Tricentis workstation.

The file is `pult-windows-x86_64.zip` (a zip with install scripts, not a setup.exe); the repository is private, so use `gh release download latest -R WKDSteve/pult` or the page while signed in. Check it against `SHA256SUMS` (`(Get-FileHash <zip> -Algorithm SHA256).Hash.ToLower()`), run `Unblock-File <zip>`, unpack it into a new, empty folder and run `install.ps1` from there:
`powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1`.
The version shown in *Settings > Apps* is `<version>+<short commit>`. Details: the `README.md` inside the zip.
