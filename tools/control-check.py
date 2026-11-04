#!/usr/bin/env python3
"""One real run driven through the control interface, on the silent outputs "Voicemeeter In 4"
and "In 5" only, with a config folder of its own.

usage: python tools/control-check.py <pult.exe> <out dir> [seconds of steady reading] [seconds of scenes]

Pairs like a local program, reads the state, sets a gain, gets a refusal, then keeps reading
and holds an event stream open. Then it changes scenes every half second with every core busy,
and drives the route with `pult <command>` as a person would.
Ends with a non-zero status when anything is not as it should be, among it a dropout or a
glitch during the reading or the scenes. The first seconds are shown but do not count: a start on
a busy machine can cost one resync (pult/start-unter-last-ein-aussetzer).
"""

import http.client
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path


def call(port, method, path, token=None, body=None, origin=False):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if origin:
        headers["Origin"] = f"http://127.0.0.1:{port}"
    if body is not None:
        headers["Content-Type"] = "application/json"
    connection.request(method, path, body=body, headers=headers)
    reply = connection.getresponse()
    text = reply.read().decode()
    connection.close()
    return reply.status, text


# How long a start may take to settle, on a busy machine too.
START_SECONDS = 10

PROFILE = """# The route of the check.
[[bus]]
name = "A1"
device = "Voicemeeter In 5"

[[bus]]
name = "A2"
device = "Voicemeeter In 4"
mute = true

[[input]]
process = "pulttone"
buses = ["A1"]

# Both buses, one side, the second output up.
[[scene]]
name = "Leise"
blend_ms = 300

[[scene.input]]
input = 1
gain_db = -12
pan = -0.5
buses = ["A1", "A2"]

[[scene.bus]]
bus = "A2"
gain_db = -6
mute = false

# As it started.
[[scene]]
name = "Normal"
blend_ms = 100

[[scene.input]]
input = 1
gain_db = 0
pan = 0
buses = ["A1"]

[[scene.bus]]
bus = "A2"
mute = true
"""


def wait_for(path):
    for _ in range(100):
        if path.exists():
            return
        time.sleep(0.1)
    raise SystemExit(f"{path} did not appear")


def session(port, secret):
    status, text = call(port, "POST", "/api/control/pair", secret, "{}")
    assert status == 200, text
    code = json.loads(text)["code"]
    status, text = call(port, "POST", "/auth/exchange", None, json.dumps({"code": code}), True)
    assert status == 200, text
    return json.loads(text)["token"]


def state(port, token):
    for _ in range(50):
        status, text = call(port, "GET", "/api/pult/state", token)
        if status == 200:
            return json.loads(text)
        assert status == 503, text
        time.sleep(0.1)
    raise SystemExit("no state came")


def troubles(now):
    """Dropouts so far, as events, and glitches so far."""
    return round(now["dropouts_per_hour"] * now["elapsed_s"] / 3600), now["glitches"]


def shows(port, token, wanted, seconds=5):
    """Whether a state fulfils `wanted` soon: one comes with the owner's next report, which a
    busy machine delays."""
    until = time.time() + seconds
    while time.time() < until:
        if wanted(state(port, token)):
            return True
        time.sleep(0.01)
    return False


def op(port, token, **fields):
    return call(port, "POST", "/api/pult/ops", token, json.dumps(fields))


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    pult, out = Path(sys.argv[1]), Path(sys.argv[2])
    seconds = int(sys.argv[3]) if len(sys.argv) > 3 else 20
    scene_seconds = int(sys.argv[4]) if len(sys.argv) > 4 else 30
    config = out / "config"
    shutil.rmtree(config, ignore_errors=True)
    config.mkdir(parents=True)
    tone_exe = out / "pulttone.exe"
    shutil.copy2(pult, tone_exe)
    env = {**os.environ, "PULT_CONFIG_DIR": str(config), "PULT_PORT": "0"}
    profile = out / "control-check.toml"
    profile.write_text(PROFILE, encoding="utf-8", newline="\n")
    (out / "control.csv").unlink(missing_ok=True)
    load = []
    tone = subprocess.Popen(
        [str(tone_exe), "tone", "--to", "Voicemeeter In 4"],
        stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT, env=env,
    )
    run = None
    failed = []
    try:
        time.sleep(2)
        with open(out / "control.txt", "w", encoding="utf-8") as console:
            run = subprocess.Popen(
                [str(pult), "run", "--profile", str(profile), "--log", str(out / "control.csv")],
                stdin=subprocess.PIPE, stdout=console, stderr=subprocess.STDOUT, env=env,
            )
            wait_for(config / "hub.json")
            record = json.loads((config / "hub.json").read_text(encoding="utf-8"))
            port = record["port"]
            secret = (config / "control.secret").read_text(encoding="utf-8").strip()

            status, _ = call(port, "GET", "/api/pult/state")
            if status != 401:
                failed.append(f"state without a session: {status}")
            token = session(port, secret)
            first = state(port, token)
            print("state:", first["inputs"][0]["name"], "->", first["buses"][0]["name"])

            status, text = op(port, token, op="set", address="bus/A1/gain", value=-6)
            print("set:", status, text)
            if status != 200:
                failed.append(f"set: {status} {text}")
            if not shows(port, token, lambda now: now["buses"][0]["gain_db"] == -6):
                failed.append("the gain did not change")

            status, text = op(port, token, op="set", address="bus/A1/gain", value=0, ramp_ms=200)
            print("ramp:", status, text)
            if status != 200 or "over 200 ms" not in text:
                failed.append(f"ramp: {status} {text}")
            status, text = op(port, token, op="set", address="bus/A1/mute", value=True, ramp_ms=200)
            if status != 422:
                failed.append(f"a ramp on a mute: {status} {text}")

            status, text = op(port, token, op="set", address="matrix/1/A1", value=False)
            print("refused:", status, text)
            if status != 422:
                failed.append(f"refusal: {status} {text}")

            stream = socket.create_connection(("127.0.0.1", port), timeout=5)
            stream.sendall(
                f"GET /api/pult/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                f"Authorization: Bearer {token}\r\n\r\n".encode()
            )
            said = stream.recv(65536).decode(errors="replace")
            if "text/event-stream" not in said:
                failed.append(f"no stream: {said[:200]}")

            # What the start cost; from here on nothing may come on top.
            time.sleep(START_SECONDS)
            at_start = troubles(state(port, token))
            print(f"dropouts and glitches of the start: {at_start}")

            reads = 0
            until = time.time() + seconds
            while time.time() < until:
                last = state(port, token)
                reads += 1
                time.sleep(0.02)
            print(f"read the state {reads} times in {seconds} s, a stream open")
            if troubles(last) != at_start:
                failed.append(f"during the reading: {at_start} became {troubles(last)}")
            if last["inputs"][0]["peak_db"] is None:
                failed.append("no peak of the tone in the state")

            if last["scenes"] != ["Leise", "Normal"]:
                failed.append(f"scenes: {last['scenes']}")
            status, text = op(port, token, op="scene", name="nichts")
            if status != 422 or "scenes: Leise, Normal" not in text:
                failed.append(f"an unknown scene: {status} {text}")

            load = [
                subprocess.Popen([sys.executable, "-c", "while True: pass"])
                for _ in range(os.cpu_count() or 4)
            ]
            changes = 0
            slowest = 0
            until = time.time() + scene_seconds
            while time.time() < until:
                name = "leise" if changes % 2 == 0 else "normal"
                status, text = op(port, token, op="scene", name=name)
                if status != 200:
                    failed.append(f"scene {name}: {status} {text}")
                    break
                changes += 1
                asked = time.time()
                want = (-12, -0.5, ["A1", "A2"]) if name == "leise" else (0, 0, ["A1"])
                arrived = shows(
                    port,
                    token,
                    lambda now: (
                        now["inputs"][0]["gain_db"],
                        now["inputs"][0]["pan"],
                        now["inputs"][0]["buses"],
                    )
                    == want,
                )
                slowest = max(slowest, time.time() - asked)
                if not arrived:
                    failed.append(f"scene {name} does not show")
                    break
                time.sleep(0.5)
                if state(port, token)["inputs"][0]["peak_db"] is None:
                    failed.append(f"scene {name}: no sound")
                    break
            for process in load:
                process.kill()
                process.wait()
            load = []
            last = state(port, token)
            print(f"changed scenes {changes} times in {scene_seconds} s, all cores busy")
            print(f"the state showed it after {slowest:.2f} s at most")
            if troubles(last) != at_start:
                failed.append(f"during the scenes: {at_start} became {troubles(last)}")

            # The same through the program itself: status, output, and how long it takes.
            def command(*words):
                begun = time.time()
                done = subprocess.run(
                    [str(pult), *words], capture_output=True, text=True, env=env, timeout=30
                )
                return done.returncode, done.stdout.strip(), done.stderr.strip(), time.time() - begun

            slowest_call = 0
            for words, wanted_status, wanted_text in [
                (("scene", "leise"), 0, "scene Leise over 300 ms"),
                (("gain", "A1", "-3", "over", "100"), 0, "over 100 ms"),
                (("mute", "pulttone"), 0, "muted"),
                (("unmute", "1"), 0, "unmuted"),
                (("scenes",), 0, "Leise\nNormal"),
                (("scene", "nichts"), 1, "no scene"),
                (("gain", "A1", "60"), 1, "is outside"),
                (("mute", "nichts"), 2, "no input or bus matches"),
            ]:
                status, out_text, err_text, took = command(*words)
                slowest_call = max(slowest_call, took)
                print(f"pult {' '.join(words)}: {status} in {took:.2f} s: {out_text or err_text}")
                if status != wanted_status or wanted_text not in (out_text + err_text):
                    failed.append(f"pult {' '.join(words)}: {status} {out_text} {err_text}")
            print(f"the slowest command took {slowest_call:.2f} s")
            status, out_text, err_text, _ = command("show")
            if status != 0 or "gain -3.0 dB" not in out_text:
                failed.append(f"pult show: {status} {out_text} {err_text}")

            # Into the profile, with the scenes and the comments left as they were.
            status, text = op(port, token, op="scene", name="leise")
            status, text = op(port, token, op="save")
            if status != 200:
                failed.append(f"save: {status} {text}")
            saved = profile.read_text(encoding="utf-8")
            scenes_at = "# Both buses, one side"
            if saved[saved.index(scenes_at):] != PROFILE[PROFILE.index(scenes_at):]:
                failed.append("saving changed the scenes")
            if not saved.startswith("# The route of the check.") or 'buses = ["A1", "A2"]' not in saved:
                failed.append("saving did not keep the file or the state")

            begun = time.time()
            run.communicate(b"\n", timeout=30)
            print(f"ended in {time.time() - begun:.1f} s with a stream open")
            status, out_text, err_text, _ = command("show")
            if status != 1 or "no pult is running" not in err_text:
                failed.append(f"pult show after the end: {status} {out_text} {err_text}")
            if (config / "cli").exists():
                failed.append("the session of the command line stayed")
            stream.close()
            if (config / "hub.json").exists() or (config / "control.secret").exists():
                failed.append("the files of the hub stayed")
    finally:
        for process in load:
            process.kill()
        if run and run.poll() is None:
            run.kill()
        tone.communicate(b"\n", timeout=30)
    text = (out / "control.txt").read_text(encoding="utf-8", errors="replace")
    remote = [line for line in text.splitlines() if "remote:" in line]
    print("console:", remote[:3])
    print("\n".join(line for line in text.splitlines() if line.startswith(("ran ", "Control", "note"))))
    if len(remote) < 2:
        failed.append("the console did not show what clients did")
    for why in failed:
        print("FAILED:", why)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
