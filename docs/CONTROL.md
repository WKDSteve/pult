# Die Steuerschnittstelle von pult

Während eine Route läuft (`pult run` oder das Fenster), lässt sie sich von anderen Programmen auf demselben Rechner
lesen und steuern. Audio fließt hier nicht, das bleibt bei den Geräten.

**Protokoll-Version:** 1 (Feld `version` im Zustand). Was ein alter Client falsch verstehen würde, erhöht sie.

## Erreichbarkeit und Anmeldung

- Der Server hört nur auf `127.0.0.1`, normalerweise auf Port **17619**. `PULT_PORT` wählt einen anderen; ist der Port von
  einem fremden Programm belegt, nimmt pult einen freien.
- Es läuft höchstens ein Server. Ein zweiter `pult run` spielt trotzdem und sagt `note: no control interface: another
  pult has it`. Am Ton ändert ein Problem mit dem Server nie etwas.
- Im Konfigurationsordner (`%APPDATA%\pult`, oder `PULT_CONFIG_DIR`) stehen während des Laufs zwei Dateien: `hub.json`
  mit Port und Instanz, und `control.secret`, nur für deinen Benutzer lesbar. Am Ende verschwinden beide. Nach einem
  Absturz bleiben sie liegen und werden beim nächsten Start ersetzt.

Ein lokales Programm meldet sich in drei Schritten an:

1. `GET /api/pult/hello` (ohne Anmeldung) nennt `app` und `instance`. Das Geheimnis nur senden, wenn beides zu `hub.json`
   passt: Sonst könnte ein fremdes Programm auf dem Port lauschen.
2. `POST /api/control/pair` mit `Authorization: Bearer <control.secret>` und dem Inhalt `{}` liefert `{"code": "…"}`, 60 s
   gültig und einmal einlösbar.
3. `POST /auth/exchange` mit `Origin: http://127.0.0.1:<port>` und `{"code": "…"}` liefert `{"token": "…"}`.

Aus Rust nimmt `pult_hub::Client` das alles ab; `pult <Befehl>` nutzt ihn (siehe `USAGE.md`, Abschnitt 8). Eine Sitzung lohnt
sich aufzuheben: Der Server nimmt nur 10 Kopplungen pro Minute und schließt über 16 Sitzungen die älteste.

Alle Routen unten brauchen `Authorization: Bearer <token>`. Mit dem Ende des Laufs endet auch die Sitzung. Prüfung,
Limits und Pairing kommen aus [appkit](https://github.com/WKDSteve/appkit), die Sicherheitstests des Kits laufen auch für
pult.

## Routen

### `GET /api/pult/state`

Der ganze Zustand als JSON. `503 not_ready`, bis der Besitzer der Route das erste Mal berichtet hat.

```json
{
  "version": 1,
  "elapsed_s": 12.4,
  "dropouts_per_hour": 0.0,
  "glitches": 0,
  "meter_point": "post",
  "profile": "abend.toml",
  "inputs": [
    {
      "number": 1,
      "name": "CABLE Output (VB-Audio Virtual Cable)",
      "label": "Musik",
      "source": "device",
      "sample_rate": 48000,
      "gain_db": 0.0,
      "muted": false,
      "solo": false,
      "pan": 0.0,
      "gate": 0.0,
      "comp": 0.0,
      "buses": ["A1"],
      "present": true,
      "peak_db": -12.1,
      "drift_ppm": 14.0,
      "dropouts": 0
    }
  ],
  "buses": [
    {
      "name": "A1",
      "label": null,
      "device": "Lautsprecher (2- Focusrite USB Audio)",
      "sample_rate": 48000,
      "gain_db": 0.0,
      "muted": false,
      "eq_on": true,
      "eq": [
        { "kind": "low", "freq_hz": 100.0, "gain_db": 0.0, "q": 0.7 },
        { "kind": "bell", "freq_hz": 500.0, "gain_db": 3.0, "q": 0.7 },
        { "kind": "bell", "freq_hz": 2500.0, "gain_db": 0.0, "q": 0.7 },
        { "kind": "high", "freq_hz": 8000.0, "gain_db": 0.0, "q": 0.7 }
      ],
      "limit_on": true,
      "limit_db": -0.3,
      "limit_lookahead_ms": 1.0,
      "limited_db": 0.0,
      "delay_ms": 0.0,
      "present": true,
      "peak_db": -12.1,
      "clipped": 0
    }
  ],
  "events": [
    { "seq": 1, "elapsed_s": 5, "type": "lost", "text": "A1 Focusrite is gone (…)" }
  ],
  "scenes": ["abend", "laut"]
}
```

- `peak_db` ist die höchste Spitze seit dem vorigen Zustand, `null` bei Stille. Ein neuer Zustand entsteht höchstens
  zehnmal pro Sekunde; `pult run` berichtet alle 200 ms.
- `events` sind die letzten 32 Ereignisse; `seq` zählt hoch und zeigt, welche ein Client schon kennt. `type` ist `lost`,
  `back`, `refused` oder `slow`.
- `gain_db`, `pan`, `muted` und `buses` nennen das Ziel. Läuft eine Rampe, folgt der Ton noch.
- `eq` sind die vier Bänder des EQ eines Busses in ihrer Reihenfolge, `eq_on` sein Schalter. `kind` ist `low`, `bell`
  oder `high`. Ein pult, das noch keinen EQ kennt, lässt beides weg: `eq` liest sich dann als leer.
- `limit_on`, `limit_db` und `limit_lookahead_ms` sind Schalter, Decke und Vorschau des Limiters eines Busses;
  `limited_db` sagt, wie weit er seit dem letzten Zustand höchstens zurückgedreht hat, 0 heißt gar nicht. Um die Vorschau
  verlässt der Ton den Bus später. Ein pult, das noch keinen Limiter kennt, lässt die Felder weg: `limit_on` liest sich
  dann als `false`.
- `delay_ms` ist die Verzögerung eines Busses in Millisekunden; ein pult, das sie noch nicht kennt, lässt sie weg: Sie
  liest sich dann als 0.
- `gate` und `comp` sind die Regler von Gate und Kompressor des Eingangs, 0 (aus) bis 10. Ein pult, das beides noch nicht
  kennt, lässt die Felder weg; sie lesen sich dann als 0.
- `scenes` sind die Szenen des Profils in dessen Reihenfolge.
- `midi` nennt `device`, `connected`, `learning` und `bindings` des Controllers, wo einer läuft, sonst `null`. `problem` sagt ohne
  Ordner, warum MIDI aus ist, etwa bei einer `midi.toml`, die sich nicht lesen lässt; sonst `null`.
- `profile` ist nur der Dateiname, ohne Ordner.

### `GET /api/pult/stream`

Server-Sent Events: je ein `event: state` mit dem ganzen Zustand, höchstens zehn pro Sekunde, dazwischen `: ping`. Jede
Nachricht ist vollständig, ein langsamer Client verpasst also nur Zustände. Je Sitzung sind 4 Streams offen, darüber
`429 too_many_streams`.

### `POST /api/pult/ops`

Eine Operation als JSON-Objekt, `Content-Type: application/json`. Unbekannte Felder oder Operationen ergeben
`400 bad_op`.

| Operation | Wirkung |
|---|---|
| `{"op": "set", "address": "input/2/gain", "value": -6}` | einen Parameter setzen |
| `{"op": "set", "address": "input/2/gain", "value": -6, "ramp_ms": 500}` | Gain oder Pan in so vielen Millisekunden erreichen, höchstens 60000; bei anderen Adressen abgelehnt |
| `{"op": "scene", "name": "abend"}` | zu einer Szene wechseln, Groß- und Kleinschreibung zählen nicht |
| `{"op": "meter_point", "value": "pre"}` | Eingangs-Meter auf das legen, was ankommt, vor Gate und Kompressor (`pre`), oder auf das, was in den Mix geht (`post`) |
| `{"op": "save"}` | den Stand in das Profil speichern, aus dem die Route kam |
| `{"op": "stop"}` | die Route beenden; wer sie betreibt, macht das in der nächsten Runde selbst und räumt auf |
| `{"op": "learn", "target": "input/1/gain"}` | die nächste Bewegung am MIDI-Controller bindet die Adresse oder `scene/<name>` |
| `{"op": "unlearn", "target": "input/1/gain"}` | die Bindung lösen |

| Adresse | Wert |
|---|---|
| `input/<n>/gain`, `bus/<name>/gain` | Zahl, -96 bis 24 dB; außerhalb wird abgelehnt, nicht begrenzt |
| `input/<n>/pan` | Zahl, -1 bis 1 |
| `input/<n>/gate`, `input/<n>/comp` | Zahl, 0 (aus) bis 10; außerhalb wird abgelehnt |
| `input/<n>/mute`, `input/<n>/solo`, `bus/<name>/mute`, `bus/<name>/eq`, `bus/<name>/limit` | `true` oder `false` |
| `bus/<name>/limit/ceiling` | Zahl, -20 bis 0 dB |
| `bus/<name>/limit/lookahead` | Zahl, 0 bis 5 ms |
| `bus/<name>/eq/<band>/gain` | Zahl, -24 bis 24 dB; `<band>` zählt von 1 bis 4 |
| `bus/<name>/eq/<band>/freq` | Zahl, 20 bis 20000 Hz |
| `bus/<name>/eq/<band>/q` | Zahl, 0.1 bis 10 |
| `bus/<name>/eq/<band>/kind` | `"low"`, `"bell"` oder `"high"` |
| `matrix/<n>/<bus>` | `true` oder `false` |
| `input/<n>/automix` | `true` oder `false`: im Automixer |
| `input/<n>/automix/weight` | Zahl, -12 bis 12 dB: Vorrang im Automixer |
| `input/<n>/polarity` | `true` oder `false`: Polarität umgedreht |
| `input/<n>/hpf` | Zahl, 20 (aus) bis 1000 Hz: Hochpass des Kanalzugs |
| `input/<n>/lpf` | Zahl, 1000 bis 20000 Hz (aus): Tiefpass des Kanalzugs |
| `input/<n>/delay` | Zahl, 0 bis 250 ms |
| `input/<n>/width` | Zahl, 0 (mono) bis 200 %: Breite eines Stereo-Eingangs |
| `input/<n>/duck` | Zahl, 0 (aus) bis 60 dB: wie weit der Eingang abgesenkt wird |
| `input/<n>/duck/key` | Zahl: Nummer des Eingangs, der ihn absenkt; 0 für keinen |
| `input/<n>/duck/threshold` | Zahl, -70 bis 0 dB: Pegel des Schlüssels, ab dem abgesenkt wird |
| `input/<n>/duck/attack`, `/hold`, `/release` | Zahl in ms: 1 bis 1000, 0 bis 5000, 10 bis 5000 |
| `bus/<name>/comp` | Zahl, 0 (aus) bis 10: der Bus-Kompressor |
| `bus/<name>/comp/mix` | Zahl, 0 (trocken) bis 100 (ganz komprimiert) |
| `bus/<name>/comp/hpf` | Zahl, 20 bis 500 Hz: Hochpass im Steuersignal |
| `bus/<name>/delay` | Zahl, 0 bis 250 ms: die Verzögerung des Busses |
| `bus/<name>/mb/low`, `/mb/mid`, `/mb/high` | Zahl, 0 (aus) bis 10: die Bänder des Multiband-Kompressors |
| `bus/<name>/mb/xlow` | Zahl, 60 bis 1000 Hz: Trennfrequenz zwischen unten und Mitte |
| `bus/<name>/mb/xhigh` | Zahl, 1000 bis 10000 Hz: Trennfrequenz zwischen Mitte und oben |
| `bus/<name>/dim` | `true` oder `false`: der Bus ist um seinen Dim-Pegel leiser |
| `bus/<name>/dim/level` | Zahl, 0 bis 60 dB: der Dim-Pegel |
| `input/<n>/talkback` | `true` oder `false`: Talkback-Mikrofon, still bis `talk` |
| `talk` | `true` oder `false`: Talkback-Mikrofone offen, Monitor-Busse gedimmt |
| `pfl` | `true` oder `false`: Solo hört auf dem Monitor-Bus vor dem Fader (PFL) statt danach (AFL) |
| `bus/<name>/monitor` | `true` oder `false`: der Monitor-Bus, auf dem allein Solo wirkt |
| `send/<n>/<bus>` | Zahl, der Pegel dieser Kreuzung in dB (wie ein Gain, gleitet mit `ramp_ms`); 0 am Anfang |
| `sendpre/<n>/<bus>` | `true` (vor dem Fader des Eingangs) oder `false` (danach, am Anfang) |
| `input/<n>/meter`, `bus/<name>/meter` | nur lesbar, im Zustand als `peak_db` |

Jeder Bus trägt im Zustand `monitor`, `minus` (die Nummer des Eingangs, für den er Mix-Minus ist, sonst `null`) und
`loudness` nach EBU R128: `momentary_lufs` (400 ms), `short_term_lufs` (3 s), `integrated_lufs` (seit dem Start),
`range_lu` und `true_peak_db` (höchster True Peak seit dem Start); jeweils `null`, bis gemessen ist. Gemessen wird auf einem
eigenen Thread, der Callback reicht nur die Daten weiter.

Im Zustand trägt jeder Eingang außerdem, was Gate und Kompressor wegnehmen (nur lesbar, seit dem letzten Zustand):
`automix`, `automix_weight_db`, `automix_gain_db` (was der Automixer gerade gibt), `gate_reduction_db` (0 offen oder aus), `gate_open` (`true` offen oder aus), `comp_reduction_db`. Das Fenster zeigt sie unter
den Reglern GATE und COMP: `open` oder `shut -40`, und die dB des Kompressors, hervorgehoben, solange er arbeitet.

Der Kanalzug eines Eingangs steht im Zustand als `strip`: `{"polarity": false, "hpf_hz": 20, "lpf_hz": 20000,
"delay_ms": 0, "width": 100}` (so ist er neutral).

`<n>` zählt ab 1 wie `[1]` in der Eingabezeile. Eine Datei lässt sich nicht benennen: `save` schreibt nur in das Profil der
Route.

Ein Client liefert Ziel und Dauer, keine laufenden Einzelwerte: Die Rampe läuft im Mischer, Bild für Bild und auch dann
sauber, wenn der Client gerade nichts tut. Ohne `ramp_ms` dauert eine Änderung 5 ms, damit sie nicht knackt.

| Antwort | Bedeutung |
|---|---|
| `200 {"lines": ["A1: gain -6.0 dB"]}` | ausgeführt; dieselbe Zeile steht als `remote: …` in der Konsole bzw. im Fenster |
| `422 refused` | passt nicht zum Pult; `message` nennt den Grund (`… would feed no bus; mute it instead`) |
| `503 busy` | zu viele Operationen warten |
| `504 no_answer` | die Route hat nicht binnen 2 s geantwortet; die Operation kann trotzdem noch ausgeführt werden |

Fehler haben die Form `{"error": {"code": "…", "message": "…"}}`. Operationen sind wie alle Änderungen begrenzt (Kit:
`429`).

## Die Seite

`GET /` liefert ohne Anmeldung eine kleine Fernbedienung, Dateien aus dem Programm, nur mit der strengen Richtlinie des Kits
und ohne Daten: Die kommen über die Routen oben, mit einer Sitzung. Ein Link von `pult open --print` hat die Form
`http://127.0.0.1:<port>/#pair=<code>`. Der Code steht im Fragment, geht also an keinen Server und in keinen Referrer, und die
Seite nimmt ihn als Erstes aus der Adressleiste.

## Zum Ausprobieren

`tools/control-check.py` fährt das alles einmal gegen einen echten Lauf durch (nur auf den stummen Ausgängen „Voicemeeter
In 4/5“, mit eigenem Konfigurationsordner), wechselt auch Szenen unter Last und zeigt in Python, wie ein Client aussieht. `tools/page-check.mjs` prüft die Seite in einem
echten Chrome gegen das Demo-Pult (`cargo run -p pult-hub --example demo-hub`), mit axe hell und dunkel, auch mit
Hover, Fokus und Problem-Hinweis; mit `PULT_PAGE_SHOTS=<Ordner>` legt es je Theme ein Bildschirmfoto dorthin. Es läuft
in der CI; von Hand steht der Aufruf im Kopf der Datei, Playwright und axe kommen in festen Versionen aus
`tools/page-check`. Dasselbe zum Mitschauen ist das Testblatt `TESTS/sheets/remote-control.sheet.lua` (testing.nvim): `testing sheets step remote-control --headed` geht es Schritt für Schritt in einem sichtbaren Chrome durch, die CI lässt es mit `testing sheets run --all` durchlaufen. Die Farben der Seite kommen aus `crates/pult-hub/ui/palette.css`, die das Icon-Werkzeug des tasks-vault aus dem Icon
erzeugt (nie von Hand ändern). Für Rust-Clients liegen die
Nachrichten als Typen in `pult_core::protocol`, ohne Geräte-Code.
