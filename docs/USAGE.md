# pult benutzen

Stand Stufe 1 (im Bau): ein Kommandozeilen-Programm, ein Ausgang direkt über `--to` oder mehrere über ein Profil; Eingänge sind Geräte oder einzelne Programme. Beispiel durchgehend: Alle Programme spielen über das virtuelle Kabel VB-CABLE, pult leitet den Ton über ein Focusrite Scarlett 2i2 zum Verstärker.

```
Programme ──► CABLE Input ══ VB-CABLE ══ CABLE Output ──► pult ──► Scarlett 2i2 ──► Verstärker
                (Standard-Wiedergabegerät)                  │
                                     Scarlett-Eingänge ─────┘ (optional: Mikro, Gitarre)
```

## 1. Einmalig einrichten

1. **VB-CABLE installieren**, falls noch nicht vorhanden: von der Seite des Herstellers VB-Audio. VB-CABLE ist Donationware und für private Nutzung frei; pult liefert es nicht mit. Den Installer selbst starten (Rechtsklick, als Administrator), danach neu starten.
2. **Abtastrate** (empfohlen, nicht nötig): In *Systemsteuerung, Sound* bei `CABLE Output` (Aufnahme), `CABLE Input` (Wiedergabe) und dem Scarlett (Wiedergabe) unter *Eigenschaften, Erweitert* dasselbe Standardformat wählen, zum Beispiel 24 Bit, 48000 Hz. Weichen die Raten ab, rechnet pult jeden Eingang auf die Rate seines Busses um; mit gleichen Raten bleibt nur der feine Ausgleich der Takt-Drift. `pult run` zeigt beim Start die Rate jedes Geräts.
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
0:12:41 [1]  -18.3 dB out 0 lost 0 drift +12 ppm resync 0 | A1  -18.3 dB clipped 0 | glitches 0 | 0.0 dropouts/h
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
| `dropouts/h` | Aussetzer pro Stunde Laufzeit, hochgerechnet. Gezählt wird, was man hört: `out`, `resync` und `glitches`, die innerhalb einer Sekunde zusammenkommen (etwa wenn ein stockendes Ausgabegerät beide Eingänge zugleich neu ausrichtet), sind ein Aussetzer; hält eine Störung an, zählt sie einmal pro Sekunde. In den ersten Minuten springt der Wert, weil schon ein einzelner Aussetzer stark zählt |
| `missing` | statt des Pegels: das Gerät ist gerade weg (siehe unten, Geräte ab- und anstecken) |

Die Lautstärke regelt weiter der Windows-Lautstärkeregler (er steuert `CABLE Input`) und der Verstärker. Solange pult nicht läuft, bleibt es still, weil alle Programme ins Kabel spielen. Zurück geht es, indem man das Standardgerät wieder auf das Scarlett stellt.

### Langzeit-Test mit Protokoll

```
pult run --from "CABLE Output" --to Focusrite --log abend.csv
```

Schreibt alle 10 Sekunden eine Zeile nach `abend.csv` (die Datei wird neu angelegt; schreibt gerade ein anderer Lauf
hinein, startet pult nicht und lässt sie unberührt, mitlesen geht weiter): Laufzeit, Spitzenpegel des
Abschnitts in dB, dazu alle Zähler der Statuszeile als laufende Summen. Jede Zeile wird sofort gespeichert, auch wenn
das Fenster einfach geschlossen wird. Die Spalte `drift_ppm` zeigt die Korrektur des Resamplers zum Zeitpunkt der
Zeile: Sie pendelt sich nach dem Start ein und bleibt dann ungefähr auf dem Unterschied der beiden Gerätetakte. Die
Spalten `dropouts` (Aussetzer bisher, gezählt wie in der Statuszeile) und `dropouts_per_hour` sind die Kennzahl für
die Stabilität; die Einzelzähler davor zeigen, woher ein Aussetzer kam. Die
Statuszeile beginnt mit der Laufzeit (`h:mm:ss`).

Beim Beenden fasst pult den Lauf in einer Zeile zusammen, zum Beispiel:

```
ran 1:00:00: 0 dropouts (0.0 per hour), 0 frames lost, 0 samples clipped
```

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
- Kommt das Gerät mit einer anderen Abtastrate zurück als beim Start, meldet pult das einmal und wartet, bis die Rate
  wieder stimmt (oder pult neu startet). Ein
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
label = "Boxen"            # optional: so nennt ihn das Fenster
gain_db = 0                # optional
mute = false               # optional
eq_on = true               # optional: false schaltet den EQ aus, die Bänder bleiben eingestellt

[[bus.eq]]                 # optional: ein Band des EQ dieses Busses
band = 2                   # 1 bis 4
kind = "bell"              # optional: low, bell oder high
freq_hz = 500              # optional, 20 bis 20000
gain_db = 3                # optional, -24 bis 24
q = 0.7                    # optional, 0.1 bis 10: je höher, desto schmaler

[[bus]]
name = "A2"
device = "Kopfhörer"
gain_db = -6

[[input]]
device = "CABLE Output"
buses = ["A1", "A2"]       # die Matrix: auf welche Busse dieser Eingang geht
label = "Musik"            # optional: so nennt ihn das Fenster statt des Gerätenamens
gain_db = 0                # optional, -96 bis 24
pan = 0                    # optional, -1 links bis 1 rechts
gate = 0                   # optional, 0 (aus) bis 10: unter einem Pegel still
comp = 0                   # optional, 0 (aus) bis 10: gleichmäßiger und lauter
mute = false               # optional
```

```
pult run --profile meinprofil.toml --log abend.csv
```

- **Busse:** Jeder Bus ist ein eigener Ausgang mit eigenem Takt. Zwei Busse dürfen nicht dasselbe Gerät benutzen. Alle
  Geräte dürfen verschiedene Abtastraten haben: Jeder Eingang wird auf die Rate des Busses umgerechnet, auf den er geht.
- **Matrix:** Ein Eingang auf mehreren Bussen bekommt je Bus einen eigenen Puffer mit eigenem Resampler, der dem Takt
  dieses Busses folgt. pult legt beim Start für jedes Paar aus Eingang und Bus einen Puffer an; nicht geroutete
  bleiben zu und kosten keine Rechenzeit. So lässt sich die Matrix später in der Oberfläche während des Laufs
  umschalten, ohne Neustart (mit `route` und `unroute`, siehe 7.). Gain,
  Mute und Pan gelten für alle seine Busse gemeinsam; der Bus-Gain wirkt auf die Summe.
- **Pan:** Bei Mono-Eingängen ein Panorama, bei Stereo-Eingängen eine Balance: in der Mitte beide Seiten voll, zur
  Seite hin wird die andere leiser. Wirkt nur auf Ausgänge mit mindestens zwei Kanälen.
- **Gate und Kompressor:** Jeder Eingang hat beides, mit je einem Regler von 0 (aus) bis 10, vor seinem Fader.
  Das **Gate** lässt nur durch, was lauter ist als eine Schwelle: bei 1 liegt sie bei -56 dB, bei 10 bei -20 dB. So bleibt
  ein Mikrofon zwischen den Wörtern still. Es öffnet in 1 ms, hält 150 ms und schließt in 200 ms. Der **Kompressor** macht
  einen Eingang gleichmäßiger und insgesamt lauter: Je höher der Regler, desto tiefer die Schwelle (bis -30 dB), desto
  stärker das Verhältnis (bis 8:1) und desto mehr Pegel kommt danach dazu (bis 12 dB). Vollaussteuerung kommt dabei nie
  über Vollaussteuerung heraus. Beide messen den lautesten Kanal und regeln alle Kanäle gleich, das Stereobild bleibt.
  Die Eingangs-Meter zeigen den Pegel dahinter.
- **EQ:** Jeder Bus hat einen Equalizer mit vier Bändern, hinter der Summe und vor seinem Fader. Am Anfang sind sie
  flach: Band 1 ein Kuhschwanz unten bei 100 Hz (`low`), Band 2 und 3 Glocken bei 500 und 2500 Hz (`bell`), Band 4 ein
  Kuhschwanz oben bei 8 kHz (`high`). Jedes Band lässt sich in Art, Frequenz, Gain und Breite (`q`) ändern; im Profil
  steht nur, was abweicht. `eq_on = false` schaltet alle Bänder aus, ohne ihre Einstellung zu verlieren. Hebt der EQ
  über Vollaussteuerung, begrenzt der Bus und zählt es als Clipping wie sonst auch.
- **Gemutet:** Ein gemuteter Eingang oder Bus spielt Stille, sein Puffer läuft aber weiter, damit er beim Einschalten
  ohne Versatz einsetzt. Umschalten geht auch während des Laufs (siehe 7.).
- **Szenen:** `[[scene]]` benennt einen Zustand des Mischpults, siehe unten.
- **Tippfehler:** Unbekannte Schlüssel, Werte außerhalb der Bereiche und Busse, die es nicht gibt, sind Fehler mit
  Zeilenangabe; pult startet dann nicht.

`--profile` lässt sich nicht mit `--from`, `--to` oder `--latency-ms` kombinieren.

### Szenen

Eine Szene ändert nur Werte und öffnet kein Gerät neu, also gibt es beim Wechsel keinen Aussetzer. Sie nennt nur, was
abweicht; alles andere bleibt, wie es gerade ist.

```toml
[[scene]]
name = "abend"
blend_ms = 500               # optional, wie lange der Wechsel dauert, höchstens 60000

[[scene.input]]
input = 2                    # der zweite [[input]], wie in input/2/gain
gain_db = -12                # außerdem pan, mute und buses = ["A1", "A2"]

[[scene.bus]]
bus = "A1"
mute = false                 # außerdem gain_db
```

Gewechselt wird mit `scene abend` (Groß- und Kleinschreibung zählen nicht), im Fenster über das Menü **Scene** oder von
außen ([`CONTROL.md`](CONTROL.md)). Wer danach speichert, speichert die Werte der Szene als Stand des Profils; die Szenen
selbst und ihre Kommentare bleiben unberührt. Solo gehört nicht dazu.

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
- Windows liefert den Ton als Stereo in der Abtastrate des ersten Busses, auf den das Programm geht; welche Rate das
  Programm selbst nutzt, spielt keine Rolle.
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
> gain 1 -12 over 2000
  [1] CABLE Output (VB-Audio Virtual Cable): gain -12.0 dB over 2000 ms
> scene abend
  scene abend over 500 ms
> save abend.toml
  saved the current state to abend.toml
```

| Befehl | Wirkung |
|---|---|
| `gain <Ziel> <dB> [over <ms>]` | Gain eines Eingangs oder Busses setzen, -96 bis 24 dB, auf Wunsch in so vielen Millisekunden (höchstens 60000) |
| `mute <Ziel>`, `unmute <Ziel>` | stumm schalten, wieder einschalten; die Statuszeile zeigt dann `muted` |
| `solo <Eingang>`, `unsolo <Eingang>` | nur die Eingänge mit Solo hören, Solo wieder lösen. Es wird nicht gespeichert |
| `pan <Eingang> <-1..1> [over <ms>]` | Panorama (Mono-Eingang) bzw. Balance (Stereo-Eingang) |
| `gate <Eingang> <0..10>`, `comp <Eingang> <0..10>` | Gate bzw. Kompressor eines Eingangs einstellen (siehe 5.); `0` oder `off` schaltet aus |
| `eq <Bus> on`, `eq <Bus> off` | den EQ eines Busses ein- oder ausschalten; die Bänder bleiben eingestellt |
| `eq <Bus> <Band> gain <dB>`, `… freq <Hz>`, `… q <Breite>`, `… kind low\|bell\|high` | eine Einstellung eines der vier Bänder (siehe 5.), zum Beispiel `eq A1 2 gain 6` |
| `route <Eingang> <Bus>`, `unroute <Eingang> <Bus>` | einen Eingang zusätzlich auf einen Bus legen, von ihm nehmen; der letzte Bus eines Eingangs bleibt (dafür `mute`) |
| `scene <Name>`, `scenes` | zu einer Szene des Profils wechseln (siehe 5.), die Szenen auflisten |
| `show` | alle Eingänge und Busse mit Gain, Pan, Gate, Kompressor, EQ und Mute |
| `save <Datei>` | den aktuellen Stand als Profil schreiben, später mit `--profile` laden. Ein Profil mit denselben Bussen und Eingängen behält seine Kommentare und Reihenfolge, nur geänderte Werte werden ersetzt; jede andere Datei wird überschrieben |
| `help` | die Befehle |
| `quit`, Enter auf leerer Zeile, Strg+C | beenden |

- **Ziel:** ein Eingang über seine Nummer (`1`) oder einen eindeutigen Teil seines Namens oder Labels (`cable`), ein Bus
  über seinen Namen (`A1`) oder einen eindeutigen Teil seines Labels (`boxen`). Passt etwas auf mehreres, auch auf einen
  Eingang und einen Bus zugleich, lehnt pult ab und nennt die Treffer. Groß- und Kleinschreibung zählen nicht.
- **Bedienen:** Pfeil hoch und runter holen frühere Befehle zurück, Esc leert die Zeile.
- **Ohne Knacken:** Keine Änderung springt. Gain, Pan, Mute und Solo gleiten mindestens 5 ms, ein Eingang blendet beim
  Ein- und Ausrouten aus bzw. ein. Gate und Kompressor gehen beim Ausschalten in ihrer eigenen Zeit zurück. Ein Band des EQ
  gleitet im Gain, wandert in Frequenz und Breite und wechselt seine Art über die flache Stellung. Anzeigen und `show` nennen schon das Ziel, der Ton folgt in der angegebenen Zeit. Geht
  ein Eingang neu auf einen Bus, setzt er nach dem Füllen seines Puffers ein; rund 2 s später gleicht pult einmalig seinen
  Versatz aus, was bei einem größeren Versatz leise zu hören sein kann.
- **Von außen:** Andere Programme auf diesem Rechner können eine laufende Route lesen und steuern, siehe
  [`CONTROL.md`](CONTROL.md). Was sie ändern, steht als `remote: …` über der Eingabezeile.
- **Gespeichert** wird nur mit `save`; beim nächsten Start gilt wieder das Profil bzw. die Kommandozeile. Ein mit
  `--from`/`--to` gestarteter Lauf lässt sich so in ein Profil verwandeln.
- Läuft pult nicht in einer Konsole (Eingabe aus einer Datei oder Pipe), ist jede Zeile ein Befehl, eine leere beendet.

## 8. Von außen steuern: `pult <Befehl>`

Läuft eine Route (`pult run` oder das Fenster), wirken dieselben Wörter wie in der Eingabezeile auch aus jedem anderen
Terminal, aus Skripten und aus dem Editor:

```
pult mute mikro
pult gain A1 -6 over 500
pult scene abend
pult show
```

| Befehl | Wirkung |
|---|---|
| `pult gain`, `mute`, `unmute`, `solo`, `unsolo`, `pan`, `gate`, `comp`, `eq`, `route`, `unroute`, `scene` | wie in der Eingabezeile (siehe 7.) |
| `pult scenes` | die Szenen, je eine Zeile |
| `pult show` | Eingänge, Busse und Szenen; `pult show --json` den ganzen Zustand für Programme |
| `pult save` | in das Profil speichern, aus dem die Route kam; eine andere Datei geht von außen nicht |
| `pult open` | die Fernbedienung im Browser öffnen, `pult open --print` nennt nur den Link |
| `pult stop` | die Route beenden, wie Enter in ihr oder Beenden im Fenster; läuft keine, ist das kein Fehler |
| `pult start` | das Fenster starten, außer es läuft schon eine Route; `pult start --open` zeigt danach die Fernbedienung |

- **Eingänge** findet pult über die Nummer, einen eindeutigen Teil des Namens oder des Labels. Anführungszeichen sind
  nicht nötig: `pult mute Analogue 1` und `pult mute "Analogue 1"` meinen dasselbe.
- **Ende:** 0 hat geklappt; 1 kein pult läuft oder es hat abgelehnt; 2 die Wörter passen nicht. Der Grund steht auf stderr.
- **Dauer:** Eine Änderung wirkt mit der nächsten Runde der Route, bei `pult run` nach höchstens 200 ms.
- **Sitzung:** pult merkt sich die Anmeldung in `%APPDATA%\pult\cli`, nur für deinen Benutzer lesbar, und löscht sie am
  Ende des Laufs.

### Die Fernbedienung im Browser

`pult open` zeigt eine kleine Seite: Pegel, Gain, Mute, Szenen und welche Route läuft, in Hell und Dunkel, auch schmal. Sie
ist eine Fernbedienung, kein zweites Pult; was angezeigt wird, kommt immer von pult. Der Link gilt einmal und 60 Sekunden,
die Anmeldung danach nur für diesen Tab, bis die Route endet. Nach dem Ende der Route verlangt die Seite einen neuen Link
(`pult open`). Sie hört nur auf `127.0.0.1` und lädt nichts von anderswo.

### Aus dem Hub

In `hub-desktop` ist pult eine Kachel: Sie startet das Fenster mit dem zuletzt geöffneten Profil, zeigt die Fernbedienung,
zeigt, ob eine Route läuft, und beendet sie. Dafür müssen `pult.exe` und `pult-app.exe` nebeneinander liegen, wie
`scripts\build-install.ps1` sie ablegt.

### Ein MIDI-Controller

Regler und Tasten eines Controllers steuern pult, gelernt auf dieselben Adressen wie überall:

```
pult learn input/1/gain
pult learn bus/A1/mute
pult learn scene/abend
```

Nach `pult learn` wartet pult bis zu 30 Sekunden auf die nächste Bewegung am Gerät und bindet sie; das geht auch als `learn …` in
der Eingabezeile von `pult run`. Eine Taste kann Mute, Solo, Matrix und Szenen schalten, aber keinen Pegel setzen. Solange eine
Route läuft, leuchten die gebundenen Tasten: Mute rot, Solo gelb, Matrix grün, Szenen gelb. Beim Beenden gehen sie aus.

| Befehl | Wirkung |
|---|---|
| `pult learn <Adresse>` | die nächste Bewegung bindet sie; ein Regler verlässt dabei seine alte Adresse |
| `pult unlearn <Adresse>` | die Bindung lösen |
| `pult midi` | das Gerät und was gebunden ist |
| `pult midi devices` | die MIDI-Anschlüsse dieses Rechners, auch ohne laufendes pult |
| `pult midi listen [Sekunden]` | zeigen, was die Anschlüsse senden; dafür darf kein pult sie offen halten |

- **Endlos-Regler und Fader:** Ein Endlos-Regler geht von dort weiter, wo der Wert gerade steht; auf dem APC Key 25 mk2 ist der ganze Weg gut eine langsame Umdrehung. Ein Fader oder Regler mit Anschlägen
  wirkt erst, wenn er den aktuellen Wert erreicht hat (`… waits until it reaches the value`): Ohne Motor stünde er nach einer
  Szene woanders, und die erste Berührung wäre ein Sprung in der Lautstärke. pult erkennt die Art an der ersten Bewegung; irrt es
  sich, ändert `relative = true` oder `false` in der Datei das.
- **Belegung:** `%APPDATA%\pult\midi.toml`. pult schreibt sie beim Lernen neu, Kommentare darin bleiben nicht. Eine Datei, die sich
  nicht lesen lässt, bleibt unberührt, und MIDI bleibt aus, bis sie repariert ist. `pult learn` und `pult midi` sagen das; was genau
  nicht stimmt, meldet das laufende pult beim Start.
- **Anschluss:** Ohne Belegung öffnet pult keinen MIDI-Anschluss. Ob ein anderes Programm ihn zugleich nutzen kann, hängt vom Treiber
  ab; ein zweites Programm, das dieselben Tasten beleuchtet, überschreibt aber die Farben von pult. Geräte lassen
  sich ab- und anstecken, pult findet sie nach wenigen Sekunden wieder und leuchtet neu. MIDI läuft nie in den Audio-Callbacks.
- **Farben** sind für die APC-Reihe hinterlegt (APC mini, die mk2-Geräte wie der APC Key 25 mk2); andere Geräte bekommen den Wert 127.

### In Neovim

Mit `my.nvim` gibt es dieselben Wörter als `:My pult` (`:My pult scene abend`), mit Vervollständigung von Befehlen, Szenen,
Eingängen und Bussen; einen Namen aus mehreren Wörtern vervollständigt es Wort für Wort. `pult` muss dafür im `PATH` liegen, oder `vim.g.pult_exe` nennt das Programm.

## 9. Beim Anmelden starten

**Options** im Fenster (in der Kopfzeile, ohne offenes Profil auf dem Startbildschirm) hat zwei Häkchen:

- **Start with Windows** (auf Linux und macOS **Start on login**): pult startet, sobald du dich anmeldest. Standard: aus.
  Dasselbe Häkchen steht im Tray-Menü.
- **Start in the background**: Dann läuft das zuletzt geöffnete Profil, und das Fenster bleibt im Tray. Ohne dieses Häkchen
  öffnet sich das Fenster. Es wirkt nur, solange das erste an ist.

Windows merkt sich das als Wert `pult` unter `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`: `"…\pult-app.exe" --tray`
für den Hintergrund, `"…\pult-app.exe"` für das offene Fenster. Was dort steht, gilt: pult liest es bei jedem Start. Ein
Update behält deine Wahl, eine Deinstallation nimmt den Wert mit.

## Fehlermeldungen

| Meldung | Ursache und Abhilfe |
|---|---|
| `no device matches "…"` | Name falsch geschrieben oder Gerät nicht angesteckt: `pult devices` |
| `"…" matches several devices: …` | einen längeren, eindeutigen Teil des Namens nehmen |
| `… came back at 44100 Hz instead of 48000 Hz` | ein verlorenes Gerät kam mit anderer Rate zurück: Rate zurückstellen oder pult neu starten |
| `… uses the sample format …` | das Gerät meldet im Shared Mode kein 32-Bit-Float; anderes Gerät oder Treiber |
| `… is there but does not open yet: …` | ein verlorenes Gerät ist wieder da, passt aber nicht (Abtastrate, Format); pult versucht es weiter |
| `… came back with … channels instead of …` | ein Eingang kam mit anderer Kanalzahl zurück; pult neu starten |
| `buses … both play on …` | zwei Busse eines Profils nennen dasselbe Ausgangsgerät |
| `… feeds bus "…", which is not defined` | im Profil fehlt ein `[[bus]]` mit diesem Namen, oder er ist anders geschrieben |
| `unknown field …` | Tippfehler im Profil; die Meldung nennt Zeile und erlaubte Schlüssel |
| `note: another pult run is active…`, `note: the pult window is open…` | beim Start: Ein anderer `pult run` oder das Fenster läuft schon. Der Lauf startet trotzdem; auf denselben Geräten hört man beide doppelt |
| `… waits until it reaches the value of …` | ein Fader wirkt erst, wenn er den aktuellen Wert erreicht hat, siehe „Ein MIDI-Controller“ |
| `no pult is running; …` | `pult <Befehl>` ohne laufende Route: erst `pult run` oder das Fenster starten |
| `note: no control interface: …` | die Steuerschnittstelle läuft nicht, meist weil ein anderer pult sie hat; der Ton läuft trotzdem |
| `not done: …` | ein Befehl während des Laufs passte nicht (Tippfehler, Wert außerhalb des Bereichs); nichts wurde geändert |
| `no running program matches "…"` | das Programm läuft nicht oder heißt anders: `pult programs` |
| `capturing …: …` | Windows hat die Aufnahme des Programms abgelehnt (älter als Windows 10 2004?) |

Exit-Codes: 0 fertig, 1 Fehler beim Öffnen oder im Lauf, 2 falscher Aufruf.
