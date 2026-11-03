#!/usr/bin/env python3
"""One run under build load, on the silent outputs "Voicemeeter In 4" and "In 5" only.

usage: python tools/load-run.py <pult.exe> <label> <out dir>

A tone plays to In 4, `pult run` captures that program and routes it to In 5, and after 30 s
a fresh release build of the workspace takes every core, followed by 30 s of quiet. Writes
<out dir>/<label>.csv; read it with tools/log-summary.py. Use a debug build, so that the
allocation guard is on, and run it once per version to compare them.
"""

import shutil
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
QUIET = 30  # seconds before and after the load


def main():
    if len(sys.argv) != 4:
        print(__doc__)
        return 2
    pult, label, out = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3])
    out.mkdir(parents=True, exist_ok=True)
    # The copy gets its own program name, so the capture finds the tone and nothing else.
    tone_exe = out / "pulttone.exe"
    shutil.copy2(pult, tone_exe)
    load_dir = out / f"load-{label}"
    shutil.rmtree(load_dir, ignore_errors=True)

    tone = subprocess.Popen(
        [str(tone_exe), "tone", "--to", "Voicemeeter In 4"],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.STDOUT,
    )
    time.sleep(2)
    with open(out / f"{label}.txt", "w", encoding="utf-8") as console:
        run = subprocess.Popen(
            [str(pult), "run", "--from", "app:pulttone", "--to", "Voicemeeter In 5",
             "--log", str(out / f"{label}.csv")],
            stdin=subprocess.PIPE,
            stdout=console,
            stderr=subprocess.STDOUT,
        )
        start = time.time()
        time.sleep(QUIET)
        load_start = time.time() - start
        build = subprocess.run(
            ["cargo", "build", "--release", "--workspace", "--target-dir", str(load_dir)],
            cwd=REPO,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        load_end = time.time() - start
        time.sleep(QUIET)
        run.communicate(b"\n", timeout=30)
        tone.communicate(b"\n", timeout=30)
    shutil.rmtree(load_dir, ignore_errors=True)

    text = (out / f"{label}.txt").read_text(encoding="utf-8", errors="replace")
    print(f"{label}: load {load_start:.0f}-{load_end:.0f} s (build exit {build.returncode})")
    print("\n".join(line for line in text.splitlines() if line.startswith("ran ")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
