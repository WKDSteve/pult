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
      "present": true,
      "peak_db": -12.1,
      "clipped": 0
    }
  ],
  "events": [
    { "seq": 1, "elapsed_s": 5, "type": "lost", "text": "A1 Focusrite is gone (…)" }
  ]
}
```

- `peak_db` ist die höchste Spitze seit dem vorigen Zustand, `null` bei Stille. Ein neuer Zustand entsteht höchstens
  zehnmal pro Sekunde; `pult run` berichtet alle 200 ms.
- `events` sind die letzten 32 Ereignisse; `seq` zählt hoch und zeigt, welche ein Client schon kennt. `type` ist `lost`,
  `back`, `refused` oder `slow`.
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
| `{"op": "meter_point", "value": "pre"}` | Eingangs-Meter vor (`pre`) oder hinter (`post`) den Fader legen |
| `{"op": "save"}` | den Stand in das Profil speichern, aus dem die Route kam |

| Adresse | Wert |
|---|---|
| `input/<n>/gain`, `bus/<name>/gain` | Zahl, -96 bis 24 dB; außerhalb wird abgelehnt, nicht begrenzt |
| `input/<n>/pan` | Zahl, -1 bis 1 |
| `input/<n>/mute`, `input/<n>/solo`, `bus/<name>/mute` | `true` oder `false` |
| `matrix/<n>/<bus>` | `true` oder `false` |
| `input/<n>/meter`, `bus/<name>/meter` | nur lesbar, im Zustand als `peak_db` |

`<n>` zählt ab 1 wie `[1]` in der Eingabezeile. Eine Datei lässt sich nicht benennen: `save` schreibt nur in das Profil der
Route.

| Antwort | Bedeutung |
|---|---|
| `200 {"lines": ["A1: gain -6.0 dB"]}` | ausgeführt; dieselbe Zeile steht als `remote: …` in der Konsole bzw. im Fenster |
| `422 refused` | passt nicht zum Pult; `message` nennt den Grund (`… would feed no bus; mute it instead`) |
| `503 busy` | zu viele Operationen warten |
| `504 no_answer` | die Route hat nicht binnen 2 s geantwortet; die Operation kann trotzdem noch ausgeführt werden |

Fehler haben die Form `{"error": {"code": "…", "message": "…"}}`. Operationen sind wie alle Änderungen begrenzt (Kit:
`429`).

## Zum Ausprobieren

`tools/control-check.py` fährt das alles einmal gegen einen echten Lauf durch (nur auf den stummen Ausgängen „Voicemeeter
In 4/5“, mit eigenem Konfigurationsordner) und zeigt in Python, wie ein Client aussieht. Für Rust-Clients liegen die
Nachrichten als Typen in `pult_core::protocol`, ohne Geräte-Code.
