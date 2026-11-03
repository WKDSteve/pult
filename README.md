# pult

Ein eigener virtueller Audio-Mixer für Windows, in Rust: Ton von Programmen, Mikrofon und Interface-Eingängen mischen und auf Hardware- und virtuelle Ausgänge verteilen. Der Name kommt vom *Mischpult*. pult ist die Schwester-App von [takt](https://github.com/WKDSteve/takt), der Medien-Suite: zwei eigenständige Programme, und läuft pult, kann takt sich später damit verbinden.

> **Status (2026-11-03): Meilenstein 1 bestanden, Stufe 1 im Bau.** Ein Kommandozeilen-Programm, das Eingangsgeräte mischt und auf einen oder mehrere Ausgänge (Busse) spielt, zum Beispiel alle Programme über ein virtuelles Kabel auf das Scarlett 2i2 und weiter zum Verstärker; am echten Aufbau 2 Stunden ohne Aussetzer gelaufen. Welcher Eingang auf welchen Bus geht, mit Gain, Mute und Pan, steht in einem Profil (TOML); während des Laufs lassen sich die Regler per Befehl ändern und als Profil speichern. Pegel, Aussetzer und Drift-Korrekturen erscheinen live in der Konsole. Ein abgestecktes Gerät beendet den Lauf nicht: der Rest spielt weiter, das Gerät wird wieder aufgenommen, sobald es zurück ist. Auch der Ton eines einzelnen Programms kann ein Eingang sein (Prozess-Loopback). Noch ohne Oberfläche und Resampler. Einrichtung und Bedienung: [docs/USAGE.md](docs/USAGE.md).

**English.** pult is a personal virtual audio mixer for Windows written in Rust (so far a command line router that mixes input devices onto one or more output buses, configured by a TOML profile). Private, non-commercial; the documentation is in German on purpose, code and commit messages are in English.

## Was geht schon

| Funktion | Stand |
|---|---|
| Geräte auflisten (`pult devices`) | fertig |
| Eingänge mischen und auf einen Ausgang spielen (`pult run --from … --to …`), Gain je Eingang | fertig |
| Mehrere Busse (Ausgänge) mit Routing-Matrix, Gain/Mute/Pan je Eingang, Gain/Mute je Bus, als Profil-Datei (`pult run --profile`) | fertig |
| Regler während des Laufs: `gain A1 -6`, `mute 2`, `pan`, `show`, `save profil.toml` in einer Eingabezeile unter der Statuszeile | fertig |
| Testton auf ein Gerät (`pult tone`) | fertig |
| Ausgleich der Takt-Drift zwischen Geräten (einzelne Frames, geglätteter Füllstand) | fertig; der adaptive Resampler folgt in Stufe 2 |
| Pegel, Aussetzer, Korrekturen, Clipping, Geräte-Glitches live | fertig |
| Verschiedene Abtastraten | noch nicht: alle Geräte einer Route brauchen dieselbe Rate |
| Geräte ab- und anstecken während des Laufs (Hot-Plug): der Rest spielt weiter, das Gerät kommt von selbst zurück | fertig; Handtest mit echtem Abstecken steht aus |
| Ton einzelner Programme als Eingang (Prozess-Loopback, `--from app:Spotify`, `pult programs`); beendet sich das Programm, wird es beim Neustart wieder aufgenommen | fertig |
| Oberfläche (Tauri), Szenen, Hub, EQ/Kompressor | geplant |

## Plattformen

| Plattform | Stand |
|---|---|
| Windows 10/11 (WASAPI, Shared Mode) | Hauptentwicklung, getestet |
| Linux, macOS | nicht vorgesehen |

## Aufbau

| Crate | Aufgabe |
|---|---|
| `crates/pult-core` | Mischen, Kanal-Zuordnung, Pegel, Drift-Regelung, Geräteauswahl, Testton; ohne Geräte-I/O, komplett testbar. Diese Typen kann takt später für die Anbindung nutzen. |
| `crates/pult-engine` | Geräte über `cpal` (WASAPI): auflisten, Streams öffnen, Callbacks an den Kern binden; Profile lesen (`profile`); Ton einzelner Programme (`process`, Windows-Aufrufe in `ffi`) |
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

Das Programm liegt danach unter `target\release\pult.exe`. Die Toolchain gibt `rust-toolchain.toml` vor, die CI läuft auf einem Self-hosted-Windows-Runner (`.github/workflows/ci.yml`).

## Planung

Konzept, Stufenplan und Tasks liegen im privaten Vault unter `tasks-vault/pult/`.

## Lizenz

[MIT](LICENSE). VB-CABLE und andere virtuelle Audiotreiber gehören nicht zu pult und werden nicht mitgeliefert.
