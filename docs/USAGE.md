# pult benutzen

Stand Stufe 1 (im Bau): ein Kommandozeilen-Programm, ein Ausgang direkt über `--to` oder mehrere über ein Profil; Eingänge sind Geräte oder einzelne Programme. Beispiel durchgehend: Alle Programme spielen über das virtuelle Kabel VB-CABLE, pult leitet den Ton über ein Focusrite Scarlett 2i2 zum Verstärker.

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

pult zeigt eine Statuszeile, darunter eine Eingabezeile für Befehle (siehe 7.). Enter auf leerer Zeile beendet:

```
0:12:41 [1]  -18.3 dB out 0 lost 0 drift +12 ppm resync 0 | A1  -18.3 dB clipped 0 | glitches 0
```

| Feld | Bedeutung |
|---|---|
| `[1] -18.3 dB` | Spitzenpegel des Eingangs 1 nach Gain |
| `out` | Aussetzer: der Puffer des Eingangs lief leer |
| `lost` | an der Aufnahme verlorene Frames: der Puffer war voll, weil der Ausgang länger als eine Sekunde nichts abgenommen hat |
| `drift` | Korrektur des Resamplers gegen die Takt-Drift zwischen Eingang und Bus, in ppm (Millionstel): `+12` heißt, der Eingang liefert 12 Frames pro Million mehr, als der Ausgang abnimmt. Echte Takte liegen meist innerhalb weniger zehn ppm; nach dem Start pendelt der Wert ein bis zwei Minuten lang ein. Bei mehreren Bussen steht der größte Wert da |
| `resync` | harte Sprünge zurück auf die Ziel-Pufferung, wenn der Puffer weit überlief; sollte 0 bleiben. Der einmalige Abgleich beim Start zählt hier nicht mit (siehe Protokoll) |
| `A1 … dB` / `clipped` | je Bus: Spitzenpegel der Summe nach dem Bus-Gain, Samples über Vollaussteuerung (auf ±1 begrenzt) oder ungültig (NaN, durch Stille ersetzt). Mit `--to` heißt der eine Bus `A1` |
| `glitches` | Unter- oder Überläufe, die Windows selbst meldet |
| `missing` | statt des Pegels: das Gerät ist gerade weg (siehe unten, Geräte ab- und anstecken) |

Die Lautstärke regelt weiter der Windows-Lautstärkeregler (er steuert `CABLE Input`) und der Verstärker. Solange pult nicht läuft, bleibt es still, weil alle Programme ins Kabel spielen. Zurück geht es, indem man das Standardgerät wieder auf das Scarlett stellt.

### Langzeit-Test mit Protokoll

```
pult run --from "CABLE Output" --to Focusrite --log abend.csv
```

Schreibt alle 10 Sekunden eine Zeile nach `abend.csv` (die Datei wird neu angelegt): Laufzeit, Spitzenpegel des
Abschnitts in dB, dazu alle Zähler der Statuszeile als laufende Summen. Jede Zeile wird sofort gespeichert, auch wenn
das Fenster einfach geschlossen wird. Die Spalte `drift_ppm` zeigt die Korrektur des Resamplers zum Zeitpunkt der
Zeile: Sie pendelt sich nach dem Start ein und bleibt dann ungefähr auf dem Unterschied der beiden Gerätetakte. Die
Statuszeile beginnt mit der Laufzeit (`h:mm:ss`).

Nach dem Start (und nach jedem Aussetzer) misst pult zwei Sekunden lang, wo sich der Puffer einpendelt, und
überspringt einen Überschuss dann auf einmal, statt ihn langsam über den Resampler abzubauen. Die Spalte
`aligned` zählt diese übersprungenen Frames; typisch sind einige hundert (ein paar Millisekunden).

Geht ein Eingang auf mehrere Busse, zählen Statuszeile und Protokoll seine Werte über alle Busse zusammen. Je Bus gibt
es die Spalten `<Bus>_peak_db` und `<Bus>_clipped`. Für jedes Gerät stehen außerdem `present` (1 spielt, 0 ist weg) und
`outages` (wie oft es bisher weg war) im Protokoll.

### Geräte ab- und anstecken

Verschwindet ein Gerät während des Laufs (USB gezogen, in Windows deaktiviert, Treiber neu gestartet), läuft pult weiter:

```
0:41:07 A1 Lautsprecher (2- Focusrite USB Audio) is gone (…); the rest plays on, it is taken back when it returns
0:41:31 A1 Lautsprecher (3- Focusrite USB Audio) is back
```

- Die übrigen Eingänge und Busse spielen ohne Unterbrechung weiter. Fehlt ein Eingang, ist er auf seinen Bussen still;
  fehlt ein Bus, hört man ihn nicht, seine Eingänge laufen auf den anderen Bussen weiter.
- pult sucht das Gerät jede Sekunde unter dem Namen aus `--from`, `--to` oder dem Profil. Deshalb findet es das Gerät auch
  dann wieder, wenn Windows die Nummer im Namen ändert (`2- Focusrite` wird zu `3- Focusrite`).
- Kommt das Gerät mit einer anderen Abtastrate zurück, meldet pult das einmal und wartet, bis die Rate wieder stimmt. Ein
  Eingang muss mit derselben Kanalzahl zurückkommen, sonst hilft nur ein Neustart von pult.
- Auch ein Gerät, das ohne Fehlermeldung einfach keine Daten mehr liefert, gilt nach zwei Sekunden als weg.
- Beim Start müssen alle Geräte da sein; fehlt eines, startet pult nicht.

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
- **Matrix:** Ein Eingang auf mehreren Bussen bekommt je Bus einen eigenen Puffer mit eigenem Resampler, der dem Takt
  dieses Busses folgt. Gain,
  Mute und Pan gelten für alle seine Busse gemeinsam; der Bus-Gain wirkt auf die Summe.
- **Pan:** Bei Mono-Eingängen ein Panorama, bei Stereo-Eingängen eine Balance: in der Mitte beide Seiten voll, zur
  Seite hin wird die andere leiser. Wirkt nur auf Ausgänge mit mindestens zwei Kanälen.
- **Gemutet:** Ein gemuteter Eingang oder Bus spielt Stille, sein Puffer läuft aber weiter, damit er beim Einschalten
  ohne Versatz einsetzt. Umschalten geht auch während des Laufs (siehe 7.).
- **Tippfehler:** Unbekannte Schlüssel, Werte außerhalb der Bereiche und Busse, die es nicht gibt, sind Fehler mit
  Zeilenangabe; pult startet dann nicht.

`--profile` lässt sich nicht mit `--from`, `--to` oder `--latency-ms` kombinieren.

## 6. Ton einzelner Programme

Statt eines Geräts kann ein Eingang der Ton eines laufenden Programms sein (Prozess-Loopback, ab Windows 10 Version
2004). So lässt sich etwa Musik getrennt vom Spiel regeln oder nur ein Programm auf einen zweiten Bus legen.

```
pult programs
pult run --from app:Spotify --from "CABLE Output" --to Focusrite
```

Im Profil steht dafür `process` statt `device`:

```toml
[[input]]
process = "Spotify"        # eindeutiger Teil des Programmnamens (Spotify.exe)
buses = ["A2"]
```

- `pult programs` listet die Namen aller laufenden Programme. Wie bei Geräten reicht ein eindeutiger Teil, Groß- und
  Kleinschreibung zählen nicht.
- Mitgenommen wird das Programm samt seiner Unterprozesse; bei einem Browser also alle Tabs. Laufen mehrere getrennte
  Programme gleichen Namens, nimmt pult das erste.
- Windows liefert den Ton als Stereo in der Abtastrate des Routings; eine abweichende Rate ist hier kein Problem.
- Spielt das Programm nichts, zeigt pult Stille (`-inf dB`), ohne Aussetzer.
- Beendet sich das Programm, meldet pult das wie ein abgestecktes Gerät; startet es wieder, wird es aufgenommen.
- **Doppelt hören vermeiden:** pult nimmt den Ton ab, das Programm spielt ihn aber weiter auf sein eigenes Ausgabegerät.
  Spielt es ins Kabel und geht `CABLE Output` ebenfalls über pult, kommt es doppelt. Abhilfe: in Windows unter
  *Einstellungen, System, Sound, Lautstärkemixer* dem Programm ein Ausgabegerät geben, das sonst nicht zu hören ist.

## 7. Während des Laufs regeln

Unter der Statuszeile steht eine Eingabezeile (`> `). Dort nimmt pult Befehle an, die Meldungen erscheinen darüber:

```
> gain A1 -6
  A1: gain -6.0 dB
> mute 2
  [2] Analogue 1 + 2 (2- Focusrite USB Audio): muted
> save abend.toml
  saved the current state to abend.toml
```

| Befehl | Wirkung |
|---|---|
| `gain <Ziel> <dB>` | Gain eines Eingangs oder Busses setzen, -96 bis 24 dB |
| `mute <Ziel>`, `unmute <Ziel>` | stumm schalten, wieder einschalten; die Statuszeile zeigt dann `muted` |
| `pan <Eingang> <-1..1>` | Panorama (Mono-Eingang) bzw. Balance (Stereo-Eingang) |
| `show` | alle Eingänge und Busse mit Gain, Pan und Mute |
| `save <Datei>` | den aktuellen Stand als Profil schreiben (eine vorhandene Datei wird ersetzt), später mit `--profile` laden |
| `help` | die Befehle |
| `quit`, Enter auf leerer Zeile, Strg+C | beenden |

- **Ziel:** ein Eingang über seine Nummer (`1`) oder einen eindeutigen Teil seines Namens (`cable`), ein Bus über seinen
  Namen (`A1`). Groß- und Kleinschreibung zählen nicht.
- **Bedienen:** Pfeil hoch und runter holen frühere Befehle zurück, Esc leert die Zeile.
- **Gespeichert** wird nur mit `save`; beim nächsten Start gilt wieder das Profil bzw. die Kommandozeile. Ein mit
  `--from`/`--to` gestarteter Lauf lässt sich so in ein Profil verwandeln.
- Läuft pult nicht in einer Konsole (Eingabe aus einer Datei oder Pipe), ist jede Zeile ein Befehl, eine leere beendet.
- Welcher Eingang auf welchen Bus geht, lässt sich während des Laufs noch nicht ändern: dafür Profil ändern und neu starten.

## Fehlermeldungen

| Meldung | Ursache und Abhilfe |
|---|---|
| `no device matches "…"` | Name falsch geschrieben oder Gerät nicht angesteckt: `pult devices` |
| `"…" matches several devices: …` | einen längeren, eindeutigen Teil des Namens nehmen |
| `… runs at 44100 Hz but … at 48000 Hz` | Abtastraten angleichen (Schritt 1.2) |
| `… uses the sample format …` | das Gerät meldet im Shared Mode kein 32-Bit-Float; anderes Gerät oder Treiber |
| `… is there but does not open yet: …` | ein verlorenes Gerät ist wieder da, passt aber nicht (Abtastrate, Format); pult versucht es weiter |
| `… came back with … channels instead of …` | ein Eingang kam mit anderer Kanalzahl zurück; pult neu starten |
| `buses … both play on …` | zwei Busse eines Profils nennen dasselbe Ausgangsgerät |
| `… feeds bus "…", which is not defined` | im Profil fehlt ein `[[bus]]` mit diesem Namen, oder er ist anders geschrieben |
| `unknown field …` | Tippfehler im Profil; die Meldung nennt Zeile und erlaubte Schlüssel |
| `not done: …` | ein Befehl während des Laufs passte nicht (Tippfehler, Wert außerhalb des Bereichs); nichts wurde geändert |
| `no running program matches "…"` | das Programm läuft nicht oder heißt anders: `pult programs` |
| `capturing …: …` | Windows hat die Aufnahme des Programms abgelehnt (älter als Windows 10 2004?) |

Exit-Codes: 0 fertig, 1 Fehler beim Öffnen oder im Lauf, 2 falscher Aufruf.
