"""Tests of tools/check-appkit-pin.py: python -m unittest discover tools"""

import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location("check_appkit_pin", os.path.join(HERE, "check-appkit-pin.py"))
pin = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pin)

SHA = "3" * 40
OTHER = "4" * 40


def package(name, source):
    return f'[[package]]\nname = "{name}"\nversion = "0.1.0"\nsource = "{source}"\n'


def lock(*sources):
    """A Cargo.lock with one appkit package per source and a crate of crates.io."""
    names = ["appkit-core", "appkit-hub", "appkit-testkit"]
    text = "version = 4\n\n" + package("serde", "registry+https://github.com/rust-lang/crates.io-index")
    for name, source in zip(names, sources):
        text += package(name, source)
    return text


def tagged(tag="v0.44.0", sha=SHA):
    return f"git+https://github.com/WKDSteve/appkit?tag={tag}#{sha}"


def workflow(ref):
    return f"jobs:\n  a:\n    steps:\n      - uses: WKDSteve/appkit/release-kit/publish@{ref} # v0.44.0\n"


def remote(commits):
    def ask(tag):
        if commits is None:
            raise RuntimeError("no network")
        return commits.get(tag)

    return ask


class CheckTest(unittest.TestCase):
    def test_one_tag_for_the_crates_and_the_workflows_that_the_tag_still_names_passes(self):
        code, lines = pin.check(
            lock(tagged(), tagged(), tagged()), {"rolling.yml": workflow(SHA)}, remote({"v0.44.0": SHA})
        )
        self.assertEqual(code, 0, lines)
        self.assertIn("1 step(s)", lines[0])

    def test_a_moved_tag_fails(self):
        code, lines = pin.check(lock(tagged(), tagged()), {}, remote({"v0.44.0": OTHER}))
        self.assertEqual(code, 1)
        self.assertIn("the tag was moved", lines[0])

    def test_two_tags_or_a_branch_fail(self):
        code, lines = pin.check(lock(tagged(), tagged("v0.43.0", OTHER)), {}, remote({}))
        self.assertEqual(code, 1)
        self.assertIn("different commits", lines[-1])
        branch = f"git+https://github.com/WKDSteve/appkit?branch=main#{SHA}"
        code, lines = pin.check(lock(tagged(), branch), {}, remote({"v0.44.0": SHA}))
        self.assertEqual(code, 1)
        self.assertIn("not by a tag", lines[0])

    def test_a_workflow_on_another_commit_or_a_tag_fails(self):
        for ref, said in ((OTHER, "one commit for both"), ("v0.44.0", "not pinned by a full commit")):
            code, lines = pin.check(lock(tagged()), {"release.yml": workflow(ref)}, remote({"v0.44.0": SHA}))
            self.assertEqual(code, 1, ref)
            self.assertIn(said, lines[0])

    def test_no_appkit_or_no_network_is_no_pass(self):
        self.assertEqual(pin.check("version = 4\n", {}, remote({}))[0], 1)
        self.assertEqual(pin.check(lock(tagged()), {}, remote(None))[0], 2)

    def test_the_lock_of_the_repository_is_read_as_pinned_by_one_tag(self):
        with open(os.path.join(HERE, "..", "Cargo.lock"), encoding="utf-8") as f:
            pins, problems = pin.pinned(list(pin.appkit_packages(f.read())))
        self.assertEqual(problems, [])
        self.assertEqual(len(pins), 1)


if __name__ == "__main__":
    unittest.main()
