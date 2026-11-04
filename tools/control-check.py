#!/usr/bin/env python3
"""One real run driven through the control interface, on the silent outputs "Voicemeeter In 4"
and "In 5" only, with a config folder of its own.

usage: python tools/control-check.py <pult.exe> <out dir> [seconds of steady reading]

Pairs like a local program, reads the state, sets a gain, gets a refusal, then keeps reading
and holds an event stream open. Ends with a non-zero status when anything is not as it should
be, among it a dropout or a glitch during the reading.
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


def op(port, token, **fields):
    return call(port, "POST", "/api/pult/ops", token, json.dumps(fields))


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    pult, out = Path(sys.argv[1]), Path(sys.argv[2])
    seconds = int(sys.argv[3]) if len(sys.argv) > 3 else 20
    config = out / "config"
    shutil.rmtree(config, ignore_errors=True)
    config.mkdir(parents=True)
    tone_exe = out / "pulttone.exe"
    shutil.copy2(pult, tone_exe)
    env = {**os.environ, "PULT_CONFIG_DIR": str(config), "PULT_PORT": "0"}

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
                [str(pult), "run", "--from", "app:pulttone", "--to", "Voicemeeter In 5"],
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
            time.sleep(0.5)
            if state(port, token)["buses"][0]["gain_db"] != -6:
                failed.append("the gain did not change")

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

            reads = 0
            until = time.time() + seconds
            while time.time() < until:
                last = state(port, token)
                reads += 1
                time.sleep(0.02)
            print(f"read the state {reads} times in {seconds} s, a stream open")
            print("dropouts/h", last["dropouts_per_hour"], "glitches", last["glitches"])
            if last["dropouts_per_hour"] != 0 or last["glitches"] != 0:
                failed.append("dropouts or glitches during the reading")
            if last["inputs"][0]["peak_db"] is None:
                failed.append("no peak of the tone in the state")

            begun = time.time()
            run.communicate(b"\n", timeout=30)
            print(f"ended in {time.time() - begun:.1f} s with a stream open")
            stream.close()
            if (config / "hub.json").exists() or (config / "control.secret").exists():
                failed.append("the files of the hub stayed")
    finally:
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
