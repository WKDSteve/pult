![pult](assets/icons/pult/png/pult-128.png)

# pult

Ein eigener virtueller Audio-Mixer für Windows, in Rust: Ton von Programmen, Mikrofon und Interface-Eingängen mischen und auf Hardware- und virtuelle Ausgänge verteilen. Der Name kommt vom *Mischpult*. pult ist die Schwester-App von [takt](https://github.com/WKDSteve/takt), der Medien-Suite: zwei eigenständige Programme, und läuft pult, kann takt sich später damit verbinden.

> **Status (2026-11-03): Stufe 2 fertig, Stufe 3 (native Oberfläche) im Bau.** Ein Kommandozeilen-Programm, das Eingangsgeräte mischt und auf einen oder mehrere Ausgänge (Busse) spielt, zum Beispiel alle Programme über ein virtuelles Kabel auf das Scarlett 2i2 und weiter zum Verstärker; am echten Aufbau 2 Stunden ohne Aussetzer gelaufen. Welcher Eingang auf welchen Bus geht, mit Gain, Mute und Pan, steht in einem Profil (TOML); während des Laufs lassen sich die Regler per Befehl ändern und als Profil speichern. Pegel, Aussetzer und die Korrektur der Takt-Drift erscheinen live in der Konsole; jeder Eingang läuft durch einen adaptiven Resampler, der dem Takt des Ausgangs folgt. Ein abgestecktes Gerät beendet den Lauf nicht: der Rest spielt weiter, das Gerät wird wieder aufgenommen, sobald es zurück ist. Auch der Ton eines einzelnen Programms kann ein Eingang sein (Prozess-Loopback). Geräte dürfen verschiedene Abtastraten haben. Noch ohne Oberfläche. Einrichtung und Bedienung: [docs/USAGE.md](docs/USAGE.md).

**English.** pult is a personal virtual audio mixer for Windows written in Rust (so far a command line router that mixes input devices onto one or more output buses, configured by a TOML profile). A personal, non-commercial project; the documentation is in German on purpose, code and commit messages are in English.

## Was geht schon

| Funktion | Stand |
|---|---|
| Geräte auflisten (`pult devices`) | fertig |
| Eingänge mischen und auf einen Ausgang spielen (`pult run --from … --to …`), Gain je Eingang | fertig |
| Mehrere Busse (Ausgänge) mit Routing-Matrix, Gain/Mute/Pan je Eingang, Gain/Mute je Bus, als Profil-Datei (`pult run --profile`) | fertig |
| Regler während des Laufs: `gain A1 -6`, `mute 2`, `pan`, `show`, `save profil.toml` in einer Eingabezeile unter der Statuszeile | fertig |
| Testton auf ein Gerät (`pult tone`) | fertig |
| Grundlage der Oberfläche (`pult_core::view`): Solo, Meter vor/nach dem Fader, Matrix während des Laufs umschalten | fertig im Kern; die native Oberfläche (egui) ist im Bau |
| Ausgleich der Takt-Drift zwischen Geräten: adaptiver Resampler (rubato, Sinc) je Eingang und Bus, PI-Regler auf dem geglätteten Füllstand, Korrektur in ppm live | fertig (Stufe 2) |
| Pegel, Aussetzer, Korrekturen, Clipping, Geräte-Glitches live | fertig |
| Verschiedene Abtastraten: jeder Eingang wird auf die Rate seines Busses umgerechnet (44,1 / 48 / 96 kHz gemischt) | fertig (Stufe 2) |
| Geräte ab- und anstecken während des Laufs (Hot-Plug): der Rest spielt weiter, das Gerät kommt von selbst zurück | fertig, am echten Aufbau geprüft |
| Ton einzelner Programme als Eingang (Prozess-Loopback, `--from app:Spotify`, `pult programs`); beendet sich das Programm, wird es beim Neustart wieder aufgenommen | fertig |
| Szenen im Profil: Gain, Pan, Mute und Matrix wechseln ohne Aussetzer, auf Wunsch weich überblendet; keine Änderung springt mehr (`docs/USAGE.md`) | fertig |
| Steuern von außen: `pult mute mikro`, `:My pult` in Neovim, eine Fernbedienung im Browser (`pult open`), Kachel im Hub, MIDI-Controller (`pult learn`) (`docs/USAGE.md`, `docs/CONTROL.md`) | fertig |
| EQ/Kompressor | geplant |

## Plattformen

| Plattform | Stand |
|---|---|
| Windows 10/11 (WASAPI, Shared Mode) | Hauptentwicklung, getestet, voller Umfang |
| Linux (ALSA) | soll genauso funktionieren. Baut und besteht die Tests (CI in WSL), ist aber noch nie mit echtem Ton gelaufen. Fehlt noch: Ton einzelner Programme, Echtzeit-Priorität, Installer |
| macOS (CoreAudio, Apple Silicon) | soll genauso funktionieren. Wird von der CI nur kompiliert, ist also noch nie gelaufen. Es fehlt dasselbe wie unter Linux |

## Aufbau

| Crate | Aufgabe |
|---|---|
| `crates/pult-core` | Mischen, Kanal-Zuordnung, Pegel, Drift-Regelung mit Resampler, Geräteauswahl, Testton; ohne Geräte-I/O, komplett testbar. Diese Typen kann takt später für die Anbindung nutzen. |
| `crates/pult-engine` | Geräte über `cpal` (WASAPI, ALSA, CoreAudio): auflisten, Streams öffnen, Callbacks an den Kern binden; Profile lesen (`profile`); Ton einzelner Programme (`process`, Windows-Aufrufe in `ffi`) |
| `crates/pult-hub` | die lokale Steuerschnittstelle auf `appkit-hub` ([`docs/CONTROL.md`](docs/CONTROL.md)); hält `axum` und `tokio` von den Audio-Crates fern |
| `crates/pult-midi` | der MIDI-Controller auf `midir`: Gerät und Belegung, außerhalb der Audio-Callbacks |
| `crates/pult-cli` | das Programm `pult` |

Echtzeit-Regel wie in takt: In den Audio-Callbacks wird nichts allokiert, nichts gesperrt und nichts geloggt. Ein Debug-Build bricht mit `assert_no_alloc` ab, sobald ein Callback doch allokiert.

`unsafe` gibt es nur im Modul `ffi` von `pult-engine` (die Windows-Aufrufe für den Prozess-Loopback); der Workspace verbietet es sonst, `pult-core` und `pult-cli` verbieten es ganz (`forbid`).

## Bauen und prüfen

```
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo audit --deny warnings
```

Die Programme liegen danach unter `target\release\pult.exe` (Kommandozeile) und `target\release\pult-app.exe` (Fenster).

Die Toolchain gibt `rust-toolchain.toml` vor, die CI läuft auf einem Self-hosted-Windows-Runner (`.github/workflows/ci.yml`). `scripts\test-release.ps1` prüft Zip, Installation und Deinstallation ohne Build und ohne echte Installation.

## Installieren

Alles für den eigenen Benutzer, ohne Setup und ohne Adminrechte: Programme und Skripte nach `%LOCALAPPDATA%\pult\bin`, ein Startmenü-Eintrag „pult“ für das Fenster und ein Eintrag in *Einstellungen > Apps*. Profile und `%APPDATA%\pult` bleiben unberührt. Ein laufendes pult spielt beim Update mit dem alten Programm weiter, bis es geschlossen wird. Nichts ist signiert; nur für private Rechner, nicht für einen verwalteten Arbeitsrechner.

- **Rolling-Build:** Nach jedem grünen CI-Lauf auf `main` liegt der Stand auf dem Pre-Release [`latest`](https://github.com/WKDSteve/pult/releases/tag/latest) als `pult-windows-x86_64.zip`.

  ```
  gh release download latest -R WKDSteve/pult -D pult-latest
  cd pult-latest
  Unblock-File .\pult-windows-x86_64.zip
  Expand-Archive .\pult-windows-x86_64.zip -DestinationPath .\entpackt
  powershell -NoProfile -ExecutionPolicy Bypass -File .\entpackt\install.ps1
  ```

  Die Prüfsumme steht in `SHA256SUMS` daneben, die Schritte im `README.md` des Zips.
- **Release:** Ein Tag `vX.Y.Z` macht einen Entwurf mit `pult-X.Y.Z-x86_64-pc-windows-msvc.zip`, den ein Mensch ansieht und veröffentlicht (`.github/workflows/release.yml`).
- **Aus dem Checkout:** `scripts\build-install.ps1` baut und ruft denselben `install.ps1` (`-BuildOnly` baut nur).

Entfernen: *Einstellungen > Apps > pult* oder `uninstall.ps1` im Installationsordner.

## Planung

Konzept, Stufenplan und Tasks liegen im privaten Vault unter `tasks-vault/pult/`.

## Lizenz

[MIT](LICENSE). VB-CABLE und andere virtuelle Audiotreiber gehören nicht zu pult und werden nicht mitgeliefert.
