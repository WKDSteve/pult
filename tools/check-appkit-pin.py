#!/usr/bin/env python3
"""Fail when the appkit pin of Cargo.lock is not the commit its tag names now.

pult depends on https://github.com/WKDSteve/appkit by a tag (appkit-core, appkit-hub, appkit-testkit) and its
release workflows take actions of the kit's release-kit by a full commit. Cargo.lock pins the commit the tag named
when the lock was written, and a build with --locked takes that commit whatever the tag says later. A tag that was
moved (by mistake, or by someone who got write access) would go unnoticed. This reads every appkit package of
Cargo.lock and asks the repository where its tag points now. pult has no page built from appkit-ui, so unlike the
check of the apps with one (hub-desktop) there is no pnpm lock to compare.

    python tools/check-appkit-pin.py [--lock Cargo.lock] [--workflows .github/workflows]

Exit 0: every appkit package is pinned by a tag, all by the same tag and commit, every action of the kit a workflow
takes (`uses: WKDSteve/appkit/release-kit/...@<sha>`) is pinned by that same commit, and the tag names that commit.
Exit 1: what is wrong is listed. Exit 2: the check itself could not run (git, network); that is not a pass.

appkit's tools/bump-apps.py moves pult to a new tag (Cargo.toml, the workflows, Cargo.lock) and runs this after it.
"""

import argparse
import os
import re
import subprocess
import sys

APPKIT = "https://github.com/WKDSteve/appkit"
SOURCE = re.compile(r"^git\+(?P<url>[^?#]+)(?:\?(?P<query>[^#]*))?#(?P<sha>[0-9a-f]{40})$")

# A step of a workflow that takes an action of the kit: `uses: WKDSteve/appkit/release-kit/publish@<ref>`, also
# quoted, in another case (GitHub does not tell owner and repository names apart by case) or from the root of the
# repository (`WKDSteve/appkit@<ref>`).
WORKFLOW_USES = re.compile(
    r"""^\s*-?\s*uses:\s*["']?WKDSteve/appkit(?:/(?P<path>[^@\s"']+))?@(?P<ref>[^\s"'#]+)""",
    re.M | re.I,
)


def appkit_packages(lock_text):
    """Yield (name, source) of every package whose source is the appkit repository."""
    for block in lock_text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        source = re.search(r'^source = "([^"]+)"', block, re.M)
        if name and source and source.group(1).startswith(f"git+{APPKIT}"):
            yield name.group(1), source.group(1)


def pinned(packages):
    """The set of (tag, sha) of the packages, and the list of what is wrong."""
    problems = []
    pins = set()
    for name, source in packages:
        match = SOURCE.match(source)
        if not match or match.group("url") != APPKIT:
            problems.append(f"{name}: not a source of {APPKIT}: {source}")
            continue
        query = match.group("query") or ""
        tag = re.fullmatch(r"tag=([^&]+)", query)
        if not tag:
            problems.append(f"{name}: pinned by '{query or 'the default branch'}', not by a tag")
            continue
        pins.add((tag.group(1), match.group("sha")))
    if len(pins) > 1:
        problems.append(f"the appkit packages are pinned to different commits: {sorted(pins)}")
    return pins, problems


def workflow_pins(texts, sha):
    """What is wrong with the actions of the kit the workflows take: each must be pinned by the full commit the
    crates are pinned to, so that the release-kit of a release is the one of pult's tag. `texts` maps a file name to
    its text."""
    problems = []
    for name, text in sorted(texts.items()):
        for match in WORKFLOW_USES.finditer(text):
            ref = match.group("ref")
            where = f"{name}: WKDSteve/appkit/{match.group('path') or ''}@{ref}"
            if not re.fullmatch(r"[0-9a-f]{40}", ref):
                problems.append(f"{where}: not pinned by a full commit")
            elif ref != sha:
                problems.append(f"{where}: Cargo.lock pins appkit at {sha}, one commit for both")
    return problems


def read_workflows(folder):
    """The text of every workflow file in the folder (none when it is not there)."""
    texts = {}
    if os.path.isdir(folder):
        for entry in sorted(os.listdir(folder)):
            if entry.endswith((".yml", ".yaml")):
                with open(os.path.join(folder, entry), encoding="utf-8") as f:
                    texts[entry] = f.read()
    return texts


def remote_commit(tag):
    """The commit the tag names now: the peeled commit of an annotated tag."""
    refs = [f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}"]
    result = subprocess.run(
        ["git", "ls-remote", APPKIT, *refs], capture_output=True, text=True, timeout=60, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(f"git ls-remote failed: {result.stderr.strip()}")
    found = {}
    for line in result.stdout.splitlines():
        sha, ref = line.split("\t", 1)
        found[ref] = sha
    return found.get(refs[1]) or found.get(refs[0])


def check(lock_text, workflows, remote):
    """The exit code and the lines to print; `remote` maps a tag to the commit it names now (raises when it cannot
    tell)."""
    packages = list(appkit_packages(lock_text))
    if not packages:
        return 1, [f"no package of {APPKIT} in Cargo.lock"]
    pins, problems = pinned(packages)
    steps = sum(len(WORKFLOW_USES.findall(text)) for text in workflows.values())
    if len(pins) == 1:
        problems.extend(workflow_pins(workflows, next(iter(pins))[1]))
    if problems:
        return 1, problems
    (tag, sha) = pins.pop()
    try:
        now = remote(tag)
    except (OSError, RuntimeError, subprocess.SubprocessError) as err:
        return 2, [f"the check could not run: {err}"]
    if now is None:
        return 1, [f"the tag {tag} is not in {APPKIT}"]
    if now != sha:
        return 1, [f"the tag {tag} names {now} now, Cargo.lock pins {sha}: the tag was moved"]
    names = ", ".join(sorted(name for name, _ in packages))
    # Says how many steps of the workflows it found: none would be a pin that was not looked at.
    return 0, [f"ok: {names} and {steps} step(s) of the workflows at {tag} = {sha[:12]}"]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--lock", default="Cargo.lock")
    parser.add_argument("--workflows", default=".github/workflows")
    args = parser.parse_args()
    with open(args.lock, encoding="utf-8") as f:
        lock_text = f.read()
    code, lines = check(lock_text, read_workflows(args.workflows), remote_commit)
    for line in lines:
        print(line)
    return code


if __name__ == "__main__":
    sys.exit(main())
