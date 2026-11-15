#!/usr/bin/env python3
"""One run under build load, on the silent outputs "Voicemeeter In 4" and "In 5" only.

usage: python tools/load-run.py <pult.exe> <label> <out dir> [--latency-ms MS] [--quiet S]

A tone plays to In 4, `pult run` captures that program and routes it to In 5, and after 30 s
a fresh release build of the workspace takes every core, followed by 30 s of quiet. Writes
<out dir>/<label>.csv; read it with tools/log-summary.py. Use a debug build, so that the
allocation guard is on, and run it once per version to compare them.

--latency-ms is handed to `pult run` (its default when left out); --quiet sets the seconds
before and after the load. With a short latency, give --quiet well over STEER_SECONDS (15 s of
pult-core's mixer), so that the ring has settled before the load and the count says something.

The pult of the run has a config folder and a port of its own (PULT_CONFIG_DIR, PULT_PORT), so a
pult the user has running is neither reached nor in the way.
"""

import argparse
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
QUIET = 30  # seconds before and after the load


def parse(argv):
    """The arguments; exits with status 2 and the usage on a wrong one."""
    parser = argparse.ArgumentParser(usage=__doc__.splitlines()[2].removeprefix("usage: "))
    parser.add_argument("pult", type=Path)
    parser.add_argument("label")
    parser.add_argument("out", type=Path)
    parser.add_argument("--latency-ms", type=int)
    parser.add_argument("--quiet", type=int, default=QUIET)
    args = parser.parse_args(argv)
    if args.latency_ms is not None and args.latency_ms <= 0:
        parser.error("--latency-ms must be above 0")
    if args.quiet < 0:
        parser.error("--quiet must not be negative")
    return args


def run_command(pult, csv, latency_ms=None):
    """The `pult run` of the route from the tone to In 5."""
    command = [str(pult), "run", "--from", "app:pulttone", "--to", "Voicemeeter In 5",
               "--log", str(csv)]
    if latency_ms is not None:
        command += ["--latency-ms", str(latency_ms)]
    return command


def free_port():
    """A port nothing listens on right now."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def run_env(config):
    """The environment of the tone and the route: their own config folder and port."""
    return dict(os.environ, PULT_CONFIG_DIR=str(config), PULT_PORT=str(free_port()))


def main(argv=None):
    args = parse(sys.argv[1:] if argv is None else argv)
    pult, label, out = args.pult, args.label, args.out
    out.mkdir(parents=True, exist_ok=True)
    # The copy gets its own program name, so the capture finds the tone and nothing else.
    tone_exe = out / "pulttone.exe"
    shutil.copy2(pult, tone_exe)
    load_dir = out / f"load-{label}"
    shutil.rmtree(load_dir, ignore_errors=True)
    config = out / f"config-{label}"
    shutil.rmtree(config, ignore_errors=True)
    config.mkdir()
    env = run_env(config)

    tone = subprocess.Popen(
        [str(tone_exe), "tone", "--to", "Voicemeeter In 4"],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.STDOUT,
        env=env,
    )
    time.sleep(2)
    with open(out / f"{label}.txt", "w", encoding="utf-8") as console:
        run = subprocess.Popen(
            run_command(pult, out / f"{label}.csv", args.latency_ms),
            stdin=subprocess.PIPE,
            stdout=console,
            stderr=subprocess.STDOUT,
            env=env,
        )
        start = time.time()
        time.sleep(args.quiet)
        load_start = time.time() - start
        build = subprocess.run(
            ["cargo", "build", "--release", "--workspace", "--target-dir", str(load_dir)],
            cwd=REPO,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        load_end = time.time() - start
        time.sleep(args.quiet)
        run.communicate(b"\n", timeout=30)
        tone.communicate(b"\n", timeout=30)
    shutil.rmtree(load_dir, ignore_errors=True)
    shutil.rmtree(config, ignore_errors=True)

    text = (out / f"{label}.txt").read_text(encoding="utf-8", errors="replace")
    print(f"{label}: load {load_start:.0f}-{load_end:.0f} s (build exit {build.returncode})")
    print("\n".join(line for line in text.splitlines() if line.startswith("ran ")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
