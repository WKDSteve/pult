#!/usr/bin/env python3
"""Reads a `pult run --log` CSV: when each counter rose, and how the run ended.

usage: python tools/log-summary.py <FILE.csv>

Two runs on one file cannot happen any more (pult refuses the second), but a log cut off by
a closed window can end in half a row, which is skipped.
"""

import csv
import re
import sys

# The running totals; the peak and drift columns are readings at that moment.
COUNTERS = re.compile(r"_(dropouts|lost|resyncs|outages|clipped)$|^(glitches|dropouts)$")


def clock(seconds):
    """Seconds as the h:mm:ss of the status line."""
    whole = int(float(seconds))
    return f"{whole // 3600}:{whole // 60 % 60:02}:{whole % 60:02}"


def complete(rows):
    """The rows with a value in every column."""
    return [row for row in rows if all(value not in (None, "") for value in row.values())]


def changes(rows):
    """Each row in which a counter rose: (elapsed_s, {column: rise}). A fall is a new base."""
    found = []
    last = {}
    for row in rows:
        rises = {}
        for column, value in row.items():
            if COUNTERS.search(column):
                now = float(value)
                if now > last.get(column, 0.0):
                    rises[column] = now - last.get(column, 0.0)
                last[column] = now
        if rises:
            found.append((row["elapsed_s"], rises))
    return found


def report(rows):
    """The lines to print for the rows of one log."""
    rows = complete(rows)
    if not rows:
        return ["no complete rows"]
    lines = [
        f"{clock(at)} " + " ".join(f"{column}+{rise:g}" for column, rise in rises.items())
        for at, rises in changes(rows)
    ]
    end = rows[-1]
    lines.append(
        f"ran {clock(end['elapsed_s'])}: {end['dropouts']} dropouts "
        f"({end['dropouts_per_hour']} per hour), {end['glitches']} glitches"
    )
    return lines


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    with open(sys.argv[1], encoding="utf-8", newline="") as file:
        print("\n".join(report(list(csv.DictReader(file)))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
