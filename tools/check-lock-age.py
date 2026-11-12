#!/usr/bin/env python3
"""Fail when a locked crate version is younger than N days.

A freshly published version is the window in which a hijacked crate does
its harm (concept risk T7: a bad release was online for 86 minutes). Cargo
has no "minimum release age" setting, so this reads Cargo.lock and asks
crates.io when each registry version was published.

    python tools/check-lock-age.py [--min-days 7] [--lock Cargo.lock]

Exit 0: every version is old enough. Exit 1: the young ones are listed;
take an older version with `cargo update -p <crate> --precise <version>`.
Exit 2: the check itself could not run (network, API); that is not a pass.

crates.io asks for one request per second and an identifying User-Agent. When a publication date has been read
once, it is kept in a cache file (it never changes): `--cache`, else `$LOCK_AGE_CACHE`, else
`~/.cache/lock-age/crates-io.json`. A run then only asks for the versions it has not seen, which keeps a runner that
checks several repositories below the rate limit of crates.io.
"""

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone

USER_AGENT = "pult-lock-age-check (https://github.com/WKDSteve/pult)"


def locked_registry_packages(lock_text):
    """Yield (name, version) of every package that comes from a registry."""
    for block in lock_text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        version = re.search(r'^version = "([^"]+)"', block, re.M)
        source = re.search(r'^source = "registry\+', block, re.M)
        if name and version and source:
            yield name.group(1), version.group(1)


# Waits after an answer 429 (too many requests), in seconds, when crates.io names none: about four minutes in all.
# A run asks for every crate of the lock file, one a second, and crates.io can still ask for a pause minutes into it.
BACKOFF = (10, 20, 40, 60, 60, 60)
# The longest pause taken from a Retry-After header.
MAX_RETRY_AFTER = 120


def retry_after(err, default):
    """The pause crates.io asks for (Retry-After, in seconds) within MAX_RETRY_AFTER, else `default`."""
    value = (err.headers or {}).get("Retry-After") if hasattr(err, "headers") else None
    try:
        seconds = int(str(value).strip())
    except (TypeError, ValueError):
        return default
    return max(1, min(seconds, MAX_RETRY_AFTER))


def published(name, version, sleep=time.sleep):
    """The moment a version was published (UTC), from the crates.io API. An answer 429 is waited out (its
    Retry-After, else BACKOFF) and asked again; anything else, or 429 after the last pause, is raised."""
    url = f"https://crates.io/api/v1/crates/{name}/{version}"
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    for attempt in range(len(BACKOFF) + 1):
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                text = json.load(response)["version"]["created_at"]
            return datetime.fromisoformat(text.replace("Z", "+00:00"))
        except urllib.error.HTTPError as err:
            if err.code == 429 and attempt < len(BACKOFF):
                sleep(retry_after(err, BACKOFF[attempt]))
                continue
            raise
    raise RuntimeError("unreachable")


def default_cache():
    """Where the publication dates are kept between runs."""
    return os.environ.get("LOCK_AGE_CACHE") or os.path.join(os.path.expanduser("~"), ".cache", "lock-age", "crates-io.json")


def load_cache(path):
    """The cache as {"name version": "ISO time"}; empty when missing or unreadable (it is only a cache)."""
    try:
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
        return {k: v for k, v in data.items() if isinstance(k, str) and isinstance(v, str)}
    except (OSError, ValueError, AttributeError):
        return {}


def save_cache(path, cache):
    """Writes the cache through a temporary file, so a run that is cut off leaves the old one."""
    try:
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        temp = f"{path}.{os.getpid()}.tmp"
        with open(temp, "w", encoding="utf-8") as handle:
            json.dump(cache, handle, sort_keys=True)
        os.replace(temp, path)
    except OSError as err:
        print(f"cache not written ({path}): {err}", file=sys.stderr)


def published_cached(name, version, cache, ask=None):
    """The publication date from the cache, or asked (and then kept)."""
    key = f"{name} {version}"
    if key in cache:
        try:
            return datetime.fromisoformat(cache[key]), False
        except ValueError:
            pass
    moment = (ask or published)(name, version)
    cache[key] = moment.isoformat()
    return moment, True


def too_young(age, min_days):
    """Is a version of this age (the time since it was published) younger than `min_days` days?

    Elapsed time, not calendar days: a version published late on one day is not a day older at midnight.
    """
    return age < timedelta(days=min_days)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--min-days", type=int, default=7)
    parser.add_argument("--lock", default="Cargo.lock")
    parser.add_argument("--cache", default=None, help="file of known publication dates")
    args = parser.parse_args()
    cache_path = args.cache or default_cache()
    cache = load_cache(cache_path)

    with open(args.lock, encoding="utf-8") as handle:
        packages = sorted(set(locked_registry_packages(handle.read())))
    now = datetime.now(timezone.utc)

    young = []
    asked = 0
    for name, version in packages:
        try:
            moment, fresh = published_cached(name, version, cache)
        except Exception as err:  # noqa: BLE001 - any failure means "not checked"
            save_cache(cache_path, cache)
            print(f"cannot check {name} {version}: {err}", file=sys.stderr)
            return 2
        age = now - moment
        if too_young(age, args.min_days):
            young.append((age.total_seconds() / 86400, name, version))
        if fresh:
            asked += 1
            time.sleep(1.05)
    save_cache(cache_path, cache)

    print(f"{len(packages)} registry crates checked ({asked} asked, the rest from the cache), minimum age {args.min_days} days")
    for age, name, version in sorted(young):
        print(f"  too young: {name} {version} ({age:.1f} days)")
    return 1 if young else 0


if __name__ == "__main__":
    sys.exit(main())
