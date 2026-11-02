# pult benutzen

Stand Meilenstein 1: ein Kommandozeilen-Programm. Beispiel durchgehend: Alle Programme spielen über das virtuelle Kabel VB-CABLE, pult leitet den Ton über ein Focusrite Scarlett 2i2 zum Verstärker.

```
Programme ──► CABLE Input ══ VB-CABLE ══ CABLE Output ──► pult ──► Scarlett 2i2 ──► Verstärker
                (Standard-Wiedergabegerät)                  │
                                     Scarlett-Eingänge ─────┘ (optional: Mikro, Gitarre)
```

## 1. Einmalig einrichten

1. **VB-CABLE installieren**, falls noch nicht vorhanden: von der Seite des Herstellers VB-Audio. VB-CABLE ist Donationware und für private Nutzung frei; pult liefert es nicht mit. Den Installer selbst starten (Rechtsklick, als Administrator), danach neu starten.
2. **Gleiche Abtastrate**: In *Systemsteuerung, Sound* bei `CABLE Output` (Aufnahme), `CABLE Input` (Wiedergabe) und dem Scarlett (Wiedergabe) unter *Eigenschaften, Erweitert* dasselbe Standardformat wählen, zum Beispiel 24 Bit, 48000 Hz. Weichen die Raten ab, sagt `pult run` das und nennt beide Geräte.
3. **Bauen**: `cargo build --release` im Repo. Das Programm liegt dann unter `target\release\pult.exe`.

## 2. Geräte ansehen

```
pult devices
```

Das zeigt alle Ein- und Ausgänge mit Kanälen, Rate und Format; `*` markiert das Standardgerät. Für `--from` und `--to` reicht ein eindeutiger Teil des Namens, Groß- und Kleinschreibung zählen nicht: `Focusrite` findet `Lautsprecher (2- Focusrite USB Audio)`. Passt ein Teil auf mehrere Geräte, nennt pult alle Treffer.

## 3. Kette mit dem Testton prüfen

Den Verstärker zuerst leise drehen, dann:

```
pult tone --to Focusrite
```

Spielt einen Sinus mit 440 Hz bei -30 dB, bis Enter gedrückt wird. Lauter oder anders: `--gain-db -20`, `--freq 1000`. Kommt der Ton aus den Boxen, stimmt der Weg Scarlett → Verstärker.

## 4. Ton über pult leiten

1. In Windows `CABLE Input` als **Standard-Wiedergabegerät** wählen (Taskleiste, Lautsprecher-Symbol, Ausgabegerät). Ab jetzt spielen alle Programme ins Kabel.
2. pult starten:

```
pult run --from "CABLE Output" --to Focusrite
```

pult zeigt eine Statuszeile und hört bei Enter auf:

```
[1]  -18.3 dB out 0 lost 0 fix -0/+0/0 | mix  -18.3 dB clipped 0 | glitches 0
```

| Feld | Bedeutung |
|---|---|
| `[1] -18.3 dB` | Spitzenpegel des Eingangs 1 nach Gain |
| `out` | Aussetzer: der Puffer des Eingangs lief leer |
| `lost` | an der Aufnahme verlorene Frames: der Puffer war voll, weil der Ausgang länger als eine Sekunde nichts abgenommen hat |
| `fix -a/+b/c` | gegen Takt-Drift verworfene (`a`) und wiederholte (`b`) Einzel-Frames, harte Resyncs (`c`) |
| `mix` / `clipped` | Spitzenpegel der Summe, Samples über Vollaussteuerung (auf ±1 begrenzt) oder ungültig (NaN, durch Stille ersetzt) |
| `glitches` | Unter- oder Überläufe, die Windows selbst meldet |

Die Lautstärke regelt weiter der Windows-Lautstärkeregler (er steuert `CABLE Input`) und der Verstärker. Solange pult nicht läuft, bleibt es still, weil alle Programme ins Kabel spielen. Zurück geht es, indem man das Standardgerät wieder auf das Scarlett stellt.

### Mehr Eingänge mischen

Jedes `--from` ist ein Eingang, ein Gain in dB hängt mit `@` am Namen:

```
pult run --from "CABLE Output" --from "Analogue 1 + 2@-6" --to Focusrite
```

Damit kommen auch die Eingänge des Scarlett (Mikrofon, Gitarre) über die Boxen, um 6 dB leiser. Für das Abhören der eigenen Stimme ist der Direct-Monitor-Schalter des Scarlett schneller, weil er keine Latenz hat.

### Latenz und Aussetzer

`--latency-ms` legt fest, wie viel pult je Eingang puffert. Der Standard sind 40 ms; gemessen auf dem Entwicklungsrechner: eine Minute über VB-CABLE ohne Aussetzer und ohne Korrektur. Knackt es oder zählt `out` hoch, den Wert erhöhen (`--latency-ms 80`). Kleinere Werte machen Videos lippensynchroner, die Regelung muss dann aber öfter eingreifen.

### Was man nicht tun sollte

`--from "CABLE Output" --to "CABLE Input"` ist eine Rückkopplung: Was ins Kabel geht, kommt sofort wieder heraus.

## Fehlermeldungen

| Meldung | Ursache und Abhilfe |
|---|---|
| `no device matches "…"` | Name falsch geschrieben oder Gerät nicht angesteckt: `pult devices` |
| `"…" matches several devices: …` | einen längeren, eindeutigen Teil des Namens nehmen |
| `… runs at 44100 Hz but … at 48000 Hz` | Abtastraten angleichen (Schritt 1.2) |
| `… uses the sample format …` | das Gerät meldet im Shared Mode kein 32-Bit-Float; anderes Gerät oder Treiber |
| `stopped, …: …` | ein Gerät ist während des Laufs verschwunden (USB gezogen); neu starten |

Exit-Codes: 0 fertig, 1 Fehler beim Öffnen oder im Lauf, 2 falscher Aufruf.
