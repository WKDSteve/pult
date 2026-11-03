# pult benutzen

Stand Stufe 1 (im Bau): ein Kommandozeilen-Programm, ein Ausgang direkt über `--to` oder mehrere über ein Profil. Beispiel durchgehend: Alle Programme spielen über das virtuelle Kabel VB-CABLE, pult leitet den Ton über ein Focusrite Scarlett 2i2 zum Verstärker.

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
0:12:41 [1]  -18.3 dB out 0 lost 0 fix -0/+0/0 | A1  -18.3 dB clipped 0 | glitches 0
```

| Feld | Bedeutung |
|---|---|
| `[1] -18.3 dB` | Spitzenpegel des Eingangs 1 nach Gain |
| `out` | Aussetzer: der Puffer des Eingangs lief leer |
| `lost` | an der Aufnahme verlorene Frames: der Puffer war voll, weil der Ausgang länger als eine Sekunde nichts abgenommen hat |
| `fix -a/+b/c` | gegen Takt-Drift verworfene (`a`) und wiederholte (`b`) Einzel-Frames, harte Resyncs (`c`); der einmalige Abgleich beim Start zählt hier nicht mit (siehe Protokoll) |
| `A1 … dB` / `clipped` | je Bus: Spitzenpegel der Summe nach dem Bus-Gain, Samples über Vollaussteuerung (auf ±1 begrenzt) oder ungültig (NaN, durch Stille ersetzt). Mit `--to` heißt der eine Bus `A1` |
| `glitches` | Unter- oder Überläufe, die Windows selbst meldet |

Die Lautstärke regelt weiter der Windows-Lautstärkeregler (er steuert `CABLE Input`) und der Verstärker. Solange pult nicht läuft, bleibt es still, weil alle Programme ins Kabel spielen. Zurück geht es, indem man das Standardgerät wieder auf das Scarlett stellt.

### Langzeit-Test mit Protokoll

```
pult run --from "CABLE Output" --to Focusrite --log abend.csv
```

Schreibt alle 10 Sekunden eine Zeile nach `abend.csv` (die Datei wird neu angelegt): Laufzeit, Spitzenpegel des
Abschnitts in dB, dazu alle Zähler der Statuszeile als laufende Summen. Jede Zeile wird sofort gespeichert, auch wenn
das Fenster einfach geschlossen wird. Am Verlauf sieht man, ob Korrekturen nur beim Start anfallen oder gleichmäßig
weiterlaufen (echte Takt-Drift). Die Statuszeile beginnt mit der Laufzeit (`h:mm:ss`).

Nach dem Start (und nach jedem Aussetzer) misst pult zwei Sekunden lang, wo sich der Puffer einpendelt, und
überspringt einen Überschuss dann auf einmal, statt ihn über viele Sekunden Frame für Frame abzubauen. Die Spalte
`aligned` zählt diese übersprungenen Frames; typisch sind einige hundert (ein paar Millisekunden).

Geht ein Eingang auf mehrere Busse, zählen Statuszeile und Protokoll seine Werte über alle Busse zusammen. Je Bus gibt
es die Spalten `<Bus>_peak_db` und `<Bus>_clipped`.

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

## 5. Profile: mehrere Ausgänge und Routing-Matrix

Ein Profil ist eine Textdatei im TOML-Format. Sie legt fest, welche Eingänge es gibt, welche Busse (je ein
Ausgangsgerät) und welcher Eingang auf welche Busse geht. Ein kommentiertes Beispiel liegt unter
[`examples/pult.toml`](../examples/pult.toml).

```toml
latency_ms = 40

[[bus]]
name = "A1"                # 1 bis 8 Zeichen: Buchstaben, Ziffern, - oder _
device = "Focusrite"       # eindeutiger Teil des Gerätenamens
gain_db = 0                # optional
mute = false               # optional

[[bus]]
name = "A2"
device = "Kopfhörer"
gain_db = -6

[[input]]
device = "CABLE Output"
buses = ["A1", "A2"]       # die Matrix: auf welche Busse dieser Eingang geht
gain_db = 0                # optional, -96 bis 24
pan = 0                    # optional, -1 links bis 1 rechts
mute = false               # optional
```

```
pult run --profile meinprofil.toml --log abend.csv
```

- **Busse:** Jeder Bus ist ein eigener Ausgang mit eigenem Takt. Zwei Busse dürfen nicht dasselbe Gerät benutzen. Alle
  Geräte brauchen dieselbe Abtastrate; maßgeblich ist der erste Bus.
- **Matrix:** Ein Eingang auf mehreren Bussen bekommt je Bus einen eigenen Puffer mit eigener Drift-Regelung. Gain,
  Mute und Pan gelten für alle seine Busse gemeinsam; der Bus-Gain wirkt auf die Summe.
- **Pan:** Bei Mono-Eingängen ein Panorama, bei Stereo-Eingängen eine Balance: in der Mitte beide Seiten voll, zur
  Seite hin wird die andere leiser. Wirkt nur auf Ausgänge mit mindestens zwei Kanälen.
- **Gemutet:** Ein gemuteter Eingang oder Bus spielt Stille, sein Puffer läuft aber weiter, damit er beim Einschalten
  ohne Versatz einsetzt. Umschalten während des Laufs kommt mit der Steuerschnittstelle; bis dahin Profil ändern und neu starten.
- **Tippfehler:** Unbekannte Schlüssel, Werte außerhalb der Bereiche und Busse, die es nicht gibt, sind Fehler mit
  Zeilenangabe; pult startet dann nicht.

`--profile` lässt sich nicht mit `--from`, `--to` oder `--latency-ms` kombinieren.

## Fehlermeldungen

| Meldung | Ursache und Abhilfe |
|---|---|
| `no device matches "…"` | Name falsch geschrieben oder Gerät nicht angesteckt: `pult devices` |
| `"…" matches several devices: …` | einen längeren, eindeutigen Teil des Namens nehmen |
| `… runs at 44100 Hz but … at 48000 Hz` | Abtastraten angleichen (Schritt 1.2) |
| `… uses the sample format …` | das Gerät meldet im Shared Mode kein 32-Bit-Float; anderes Gerät oder Treiber |
| `stopped, …: …` | ein Gerät ist während des Laufs verschwunden (USB gezogen); neu starten |
| `buses … both play on …` | zwei Busse eines Profils nennen dasselbe Ausgangsgerät |
| `… feeds bus "…", which is not defined` | im Profil fehlt ein `[[bus]]` mit diesem Namen, oder er ist anders geschrieben |
| `unknown field …` | Tippfehler im Profil; die Meldung nennt Zeile und erlaubte Schlüssel |

Exit-Codes: 0 fertig, 1 Fehler beim Öffnen oder im Lauf, 2 falscher Aufruf.
