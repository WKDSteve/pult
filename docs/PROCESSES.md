# Programs pult starts

Every program pult starts, with why, how long it lives and which way of
[appkit's `docs/PROCESSES.md`](https://github.com/WKDSteve/appkit/blob/main/docs/PROCESSES.md)
(rule `RS-08`) starts it. **A process that is not in this list is a bug.** The list is taken
from the code (`tools/process-starts.allow`, checked by `tools/check-process-starts.py` in the CI, and the `launch`
and `browser::open` calls)
and checked against a measurement with `app-proc-watch`.

pult does its work in its own process: the window (`pult-app`) mixes the devices, runs the
control interface (`pult-hub`, loopback only) and draws its icon in the notification area, all
in one process. The sound of a single program (process loopback) is read through the audio API
of Windows; no program is started for it.

| Program | Started by | Why | How long | Way |
| --- | --- | --- | --- | --- |
| `pult-app` (the window) | `pult start [--open]` (the command line, the tile of hub-desktop) when no route runs | it is the app: it runs until you close it, also after `pult` ended | launch (`appkit_hub::launch::spawn`, the program beside `pult`, nothing inherited) |
| the default browser | `pult open`, `pult start --open` (the command line) | the remote control page, with a pairing link | yours | system (`appkit_hub::browser::open`: `ShellExecuteW` on Windows, no process of pult; `/usr/bin/open` or `xdg-open` through `appkit_core::launch` elsewhere) |
| `pult-plughost host …` | `pult-plughost bench` (a measurement of the plug-in spike, by hand) | the second process of the measured insert | until the bench ends or kills it (`--kill-at`) | started by the bench itself; not part of the release and never started by `pult-app` or `pult` |

Apart from these, `pult-app` and `pult` start no program: no shell, no console helper, no `reg.exe` (the
login entry goes through the registry API). The login entry starts `pult-app --tray`, a window
program, so no console window opens at the login.

`pult-plughost` is a spike outside the release (see the README); its bench is the one place that
starts a process by hand (`crates/pult-plughost/src/bench.rs`). It is the one entry of
`tools/process-starts.allow`; `tools/check-process-starts.py` (in the CI) refuses any other start by hand.
