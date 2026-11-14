#!/usr/bin/env python3
"""The click test over real devices: a tone through a running pult, recorded where it comes out.

usage: python tools/click-run.py <pult.exe> <out dir> [--tone-to DEVICE] [--from INPUT]
                                 [--bus DEVICE] [--side-bus DEVICE] [--record SOURCE]
                                 [--tone-db DB]

What `cargo test` renders without devices (crates/pult-engine/tests/clicks.rs), here over WASAPI,
real callbacks and real clocks: a 103 Hz tone plays to a device, `pult run` routes it to a bus, and
`pult record` records what that bus plays while the changes of the sequence (ramps, scene changes,
mutes, the matrix, the equalizer's switch, the limiter's lookahead and switch) go to that pult
through its control interface. `pult clicks` then looks through the recording with the detector of
the offline test, the end of the route (`pult stop`, where every bus fades out) included. A probe
at the end cuts a piece out of a copy of the recording and must find it.

Without options nothing is heard and no cable is needed: the tone goes to "Voicemeeter In 4", pult
captures the tone's program and plays to "Voicemeeter In 5", and the recording is what In 5 plays
(a WASAPI loopback). Over a cable, name the devices, for example a virtual cable into pult and a
loopback cable from the interface's output to its input:

    python tools/click-run.py target/debug/pult.exe out --tone-to "CABLE Input"
        --from "CABLE Output" --bus "Focusrite" --record "Analogue 1 + 2"

The matrix button of the sequence (the input off the bus and on again) needs a second bus the input
stays on, since pult keeps every input on at least one: --side-bus names its device. In the silent
setup that is "Voicemeeter In 4"; with devices of one's own and no --side-bus those two changes
are left out, and the run says so.

An automated session uses the silent setup only ("Voicemeeter In 4" and "In 5"): every other
device, VB-CABLE included, may be part of the audio the user hears, so runs over them are the
user's own, by hand.

The pult of this run has a config folder and a port of its own (PULT_CONFIG_DIR, PULT_PORT), so
its commands never reach a pult the user has running. Use a debug build, so that the allocation
guard of the callbacks is on.

Writes <out dir>/click-run.wav, click-run-probe.wav, click-run.toml and click-run.txt.
Ends with 0 when the recording has no click and the probe found its cut, with 1 when it has
clicks, and with 2 when the run itself failed.
"""

import argparse
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

TONE_HZ = 103
# Seconds the recording runs before pult starts: the noise of the way alone, for the floor.
QUIET = 3.0
# Seconds after the start of the route that are not judged: the rings prime, and about 2 s in the
# one alignment of the start skips on purpose.
SETTLE = 4.0
# Seconds between two changes: each has room to settle, as in the offline test.
STEP = 0.5
# The devices of the silent setup (tools/load-run.py uses the same two).
# How far below the tone's peak the floor of the quiet start must lie for a judgement to count.
HEADROOM_DB = 30.0

SILENT_TONE = "Voicemeeter In 4"
SILENT_BUS = "Voicemeeter In 5"

BUS = """\
[[bus]]
name = "A1"
device = "{bus}"
limit_on = true
limit_db = -6
limit_lookahead_ms = 1

[[bus.eq]]
band = 1
kind = "bell"
freq_hz = 100
gain_db = 6
q = 1
"""

SIDE_BUS = """\
[[bus]]
name = "A2"
device = "{side_bus}"
"""

SCENES = """\
[[scene]]
name = "laut"
blend_ms = 300

[[scene.input]]
input = 1
gain_db = 4

[[scene]]
name = "leise"

[[scene.input]]
input = 1
gain_db = -20
pan = -0.5

[[scene.bus]]
bus = "A1"
gain_db = -2
"""

# The changes, as the words of pult's command line. Those of the offline test, on one input and
# one bus; the limiter works in "laut" (the tone, the bell and the scene's gain pass its ceiling).
SEQUENCE = [
    "gain 1 -6 over 500",
    "gain A1 -3",
    "pan 1 -0.7",
    "scene laut",
    "limit A1 look 0",
    "limit A1 look 5",
    "limit A1 off",
    "limit A1 on",
    "eq A1 off",
    "eq A1 on",
    "scene leise",
    "mute 1",
    "unmute 1",
    "mute A1",
    "unmute A1",
    "gain 1 0 over 300",
]

# The matrix button, where a second bus keeps the input; before the last change.
MATRIX = ["unroute 1 A1", "route 1 A1"]

# After everything: up again, so the end of the recording is loud enough to judge.
LAST = ["scene laut"]


def free_port():
    """A port nothing listens on right now."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def profile_text(bus, side_bus, source):
    """The profile of the run: one input on the bus that is recorded, and on a side bus if any."""
    if source.lower().startswith("app:"):
        source_line = f'process = "{source[4:]}"'
    else:
        source_line = f'device = "{source}"'
    buses = '"A1", "A2"' if side_bus else '"A1"'
    parts = ["latency_ms = 40\n", BUS.format(bus=bus)]
    if side_bus:
        parts.append(SIDE_BUS.format(side_bus=side_bus))
    parts.append(f"[[input]]\n{source_line}\nbuses = [{buses}]\n")
    parts.append(SCENES)
    return "\n".join(parts)


def loud_floor(judgement):
    """What `pult clicks` said about peak and floor when the floor is too near the peak to judge.

    The floor comes from the quiet start of the recording. When something else plays on the
    recorded device, that start is not quiet, the floor rises, and steps below it do not count:
    the judgement would say "no clicks" whatever happened. Returns (peak, floor) in dBFS when the
    floor is less than HEADROOM_DB below the peak, None when it is fine or not named.
    """
    found = re.search(r"peak (-?[0-9.]+|-inf) dBFS, floor (-?[0-9.]+|-inf) dBFS", judgement)
    if not found:
        return None
    peak, floor = (float(value) for value in found.groups())
    if floor > peak - HEADROOM_DB:
        return peak, floor
    return None


def cut_a_piece(source, target, at_seconds):
    """Copies the recording with 5 ms of silence at `at_seconds`: a jump at both of its ends."""
    data = bytearray(source.read_bytes())
    channels, rate = struct.unpack_from("<HI", data, 22)
    frame = 4 * channels
    start = 44 + int(at_seconds * rate) * frame
    length = int(0.005 * rate) * frame
    if start + length > len(data):
        raise ValueError("the recording is too short for the probe")
    data[start:start + length] = bytes(length)
    target.write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", maxsplit=1)[0])
    parser.add_argument("pult", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--tone-to", default=SILENT_TONE)
    parser.add_argument("--from", dest="source", default=None,
                        help="the input of pult: a device, or app:<program>; default the tone's program")
    parser.add_argument("--bus", default=SILENT_BUS)
    parser.add_argument("--side-bus", default=None,
                        help="a second bus the input stays on, for the matrix button")
    parser.add_argument("--record", default=None,
                        help="what to record: an input device, or out:<output>; default what the bus plays")
    parser.add_argument("--tone-db", type=float, default=-12.0)
    args = parser.parse_args()

    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    pult = args.pult.resolve()
    # The copy gets its own program name, so the capture finds the tone and nothing else.
    tone_exe = out / "pulttone.exe"
    shutil.copy2(pult, tone_exe)
    source = args.source or "app:pulttone"
    record = args.record or f"out:{args.bus}"
    silent_setup = args.source is None and args.tone_to == SILENT_TONE and args.bus == SILENT_BUS
    side_bus = args.side_bus or (SILENT_TONE if silent_setup else None)
    sequence = SEQUENCE + (MATRIX if side_bus else []) + LAST
    profile = out / "click-run.toml"
    profile.write_text(profile_text(args.bus, side_bus, source), encoding="utf-8")
    config = out / "config"
    shutil.rmtree(config, ignore_errors=True)
    config.mkdir()
    env = dict(os.environ, PULT_CONFIG_DIR=str(config), PULT_PORT=str(free_port()))
    wav = out / "click-run.wav"
    total = QUIET + SETTLE + STEP * (len(sequence) + 3) + 6.0
    report = []

    def say(line):
        print(line)
        report.append(line)

    def remote(words):
        done = subprocess.run([str(pult), *words.split()], env=env, capture_output=True,
                              text=True, encoding="utf-8", errors="replace", timeout=30,
                              check=False)
        if done.returncode != 0:
            raise RuntimeError(f"pult {words}: {done.stderr.strip() or done.stdout.strip()}")
        return done.stdout

    tone = subprocess.Popen(
        [str(tone_exe), "tone", "--to", args.tone_to, "--freq", str(TONE_HZ),
         "--gain-db", str(args.tone_db)],
        stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    recorder = run = None
    try:
        time.sleep(1.0)
        recorder = subprocess.Popen(
            [str(pult), "record", "--from", record, "--to", str(wav), "--seconds", str(total)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        started = time.time()
        time.sleep(QUIET)
        with open(out / "click-run-pult.txt", "w", encoding="utf-8") as console:
            run = subprocess.Popen([str(pult), "run", "--profile", str(profile)], env=env,
                                   stdin=subprocess.PIPE, stdout=console,
                                   stderr=subprocess.STDOUT)
            route_at = time.time() - started
            time.sleep(SETTLE)
            if run.poll() is not None:
                raise RuntimeError("pult run ended early, see click-run-pult.txt")
            before = json.loads(remote("show --json"))
            if not side_bus:
                say("no --side-bus: the matrix button is left out")
            for words in sequence:
                remote(words)
                say(f"{time.time() - started:6.2f} s  {words}")
                time.sleep(STEP)
            time.sleep(1.0)
            after = json.loads(remote("show --json"))
            stop_at = time.time() - started
            remote("stop")
            run.wait(timeout=30)
        recorded = recorder.communicate(timeout=total + 30)[0].decode("utf-8", "replace")
        say(recorded.strip())
        if recorder.returncode != 0:
            raise RuntimeError("pult record failed")
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError) as problem:
        print(f"click-run: {problem}", file=sys.stderr)
        for process in (run, recorder):
            if process is not None and process.poll() is None:
                process.kill()
        return 2
    finally:
        if tone.poll() is None:
            try:
                tone.communicate(b"\n", timeout=30)
            except subprocess.SubprocessError:
                tone.kill()

    dropouts = (sum(i["dropouts"] for i in after["inputs"])
                - sum(i["dropouts"] for i in before["inputs"]))
    glitches = after["glitches"] - before["glitches"]
    say(f"during the sequence pult counted {dropouts} dropouts and {glitches} device glitches")

    # The recorder needs a moment to start, so the times of this script run a little ahead of
    # those in the file: half of the quiet start is left out of the floor, and the judgement
    # ends well before the route does.
    skip = route_at + SETTLE - 0.5
    until = stop_at - 0.5
    judge = [str(pult), "clicks", str(wav), "--freq", str(TONE_HZ), "--quiet", str(QUIET / 2),
             "--skip", f"{skip:.2f}", "--until", f"{until:.2f}"]
    verdict = subprocess.run(judge, capture_output=True, text=True, encoding="utf-8",
                             errors="replace", timeout=120, check=False)
    say(verdict.stdout.strip())
    if verdict.returncode not in (0, 1):
        print(f"click-run: {verdict.stderr.strip()}", file=sys.stderr)
        return 2
    loud = loud_floor(verdict.stdout)
    if loud:
        print(f"click-run: the start of the recording was not quiet (floor {loud[1]:.1f} dBFS, "
              f"tone {loud[0]:.1f} dBFS): something else plays on the recorded device, and "
              "nothing can be judged", file=sys.stderr)
        return 2

    # The probe: the same judgement must find a piece cut out while the tone ran steadily,
    # before the first change.
    probe = out / "click-run-probe.wav"
    try:
        cut_a_piece(wav, probe, skip + 0.25)
    except (ValueError, struct.error) as problem:
        print(f"click-run: {problem}", file=sys.stderr)
        return 2
    judge[2] = str(probe)
    probed = subprocess.run(judge, capture_output=True, text=True, encoding="utf-8",
                            errors="replace", timeout=120, check=False)
    found = probed.returncode == 1
    say("probe: the cut of 5 ms in a copy " + ("was found" if found else "was NOT found"))

    # The end of the route (pult stop): every bus fades out before its stream falls.
    ending = subprocess.run(
        [str(pult), "clicks", str(wav), "--freq", str(TONE_HZ), "--quiet", str(QUIET / 2),
         "--skip", f"{until:.2f}"],
        capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120,
        check=False)
    say("the end of the route (pult stop): "
        + (ending.stdout.strip().splitlines() or ["not judged"])[-1])
    if ending.returncode not in (0, 1):
        print(f"click-run: {ending.stderr.strip()}", file=sys.stderr)
        return 2

    clicks = verdict.returncode == 1 or ending.returncode == 1
    if clicks and (dropouts or glitches):
        say("note: a dropout or glitch is a jump the sequence did not make; compare the times")
    say("RESULT: " + ("clicks" if clicks else "no clicks")
        + ("" if found else ", and the probe is blind"))
    (out / "click-run.txt").write_text("\n".join(report) + "\n", encoding="utf-8")
    if not found:
        return 2
    return 1 if clicks else 0


if __name__ == "__main__":
    sys.exit(main())
