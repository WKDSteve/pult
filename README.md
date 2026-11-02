# pult

Ein eigener virtueller Audio-Mixer für Windows, in Rust: Ton von Programmen, Mikrofon und Interface-Eingängen mischen und auf Hardware- und virtuelle Ausgänge verteilen. Der Name kommt vom *Mischpult*. pult ist die Schwester-App von [takt](https://github.com/WKDSteve/takt), der Medien-Suite: zwei eigenständige Programme, und läuft pult, kann takt sich später damit verbinden.

> **Status (2026-11-02): Meilenstein 1.** Ein Kommandozeilen-Programm, das ein oder mehrere Eingangsgeräte mischt und auf ein Ausgangsgerät spielt, zum Beispiel alle Programme über ein virtuelles Kabel auf das Scarlett 2i2 und weiter zum Verstärker. Pegel, Aussetzer und Drift-Korrekturen erscheinen live in der Konsole. Noch ohne Oberfläche, Profile, Resampler und Prozess-Loopback. Einrichtung und Bedienung: [docs/USAGE.md](docs/USAGE.md).

**English.** pult is a personal virtual audio mixer for Windows written in Rust (milestone 1: a command line router that mixes input devices into one output device). Private, non-commercial; the documentation is in German on purpose, code and commit messages are in English.

## Was geht schon

| Funktion | Stand |
|---|---|
| Geräte auflisten (`pult devices`) | fertig |
| Eingänge mischen und auf einen Ausgang spielen (`pult run`), Gain je Eingang | fertig |
| Testton auf ein Gerät (`pult tone`) | fertig |
| Ausgleich der Takt-Drift zwischen Geräten (einzelne Frames, geglätteter Füllstand) | fertig; der adaptive Resampler folgt in Stufe 2 |
| Pegel, Aussetzer, Korrekturen, Clipping, Geräte-Glitches live | fertig |
| Verschiedene Abtastraten | noch nicht: alle Geräte einer Route brauchen dieselbe Rate |
| Oberfläche (Tauri), Profile, Szenen, Hub, Prozess-Loopback, EQ/Kompressor | geplant |

## Plattformen

| Plattform | Stand |
|---|---|
| Windows 10/11 (WASAPI, Shared Mode) | Hauptentwicklung, getestet |
| Linux, macOS | nicht vorgesehen |

## Aufbau

| Crate | Aufgabe |
|---|---|
| `crates/pult-core` | Mischen, Kanal-Zuordnung, Pegel, Drift-Regelung, Geräteauswahl, Testton; ohne Geräte-I/O, komplett testbar. Diese Typen kann takt später für die Anbindung nutzen. |
| `crates/pult-engine` | Geräte über `cpal` (WASAPI): auflisten, Streams öffnen, Callbacks an den Kern binden |
| `crates/pult-cli` | das Programm `pult` |

Echtzeit-Regel wie in takt: In den Audio-Callbacks wird nichts allokiert, nichts gesperrt und nichts geloggt. Ein Debug-Build bricht mit `assert_no_alloc` ab, sobald ein Callback doch allokiert.

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
