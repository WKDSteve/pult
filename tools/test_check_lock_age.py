"""Tests of check-lock-age.py: the age is the time since publication, not the number of calendar days, and a
check that cannot ask crates.io is no pass. crates.io is never asked: `published` is replaced.

    python -m unittest discover tools
"""

import importlib.util
import os
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location("check_lock_age", Path(__file__).with_name("check-lock-age.py"))
age_check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(age_check)

LOCK = (
    'version = 4\n\n[[package]]\nname = "serde"\nversion = "1.0.0"\n'
    'source = "registry+https://github.com/rust-lang/crates.io-index"\n\n'
    '[[package]]\nname = "appkit-hub"\nversion = "0.0.0"\n'
    'source = "git+https://github.com/WKDSteve/appkit?tag=v0.4.0#1cb7df1e150be297b1dd0ff5112f51d97efb745d"\n'
)


class TooYoung(unittest.TestCase):
    def test_a_day_is_elapsed_time_not_a_calendar_day(self):
        # Published at 02:25 on the 10th, checked at 02:14 on the 17th: seven calendar days, not seven days.
        self.assertTrue(age_check.too_young(timedelta(days=6, hours=23, minutes=49), 7))
        self.assertFalse(age_check.too_young(timedelta(days=7), 7))
        self.assertFalse(age_check.too_young(timedelta(days=30), 7))


class Main(unittest.TestCase):
    def run_main(self, published):
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "Cargo.lock")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(LOCK)
            argv = ["check-lock-age.py", "--lock", path, "--min-days", "7"]
            with mock.patch.object(sys, "argv", argv), mock.patch.object(
                age_check, "published", side_effect=published
            ), mock.patch.object(age_check.time, "sleep"), mock.patch("builtins.print"):
                return age_check.main()

    def test_a_version_a_few_minutes_short_of_a_week_fails(self):
        just_short = lambda name, version: datetime.now(timezone.utc) - timedelta(days=6, hours=23, minutes=50)  # noqa: E731
        self.assertEqual(self.run_main(just_short), 1)

    def test_a_version_older_than_a_week_passes(self):
        old = lambda name, version: datetime.now(timezone.utc) - timedelta(days=7, minutes=5)  # noqa: E731
        self.assertEqual(self.run_main(old), 0)

    def test_only_registry_crates_are_asked(self):
        asked = []

        def old(name, version):
            asked.append(name)
            return datetime.now(timezone.utc) - timedelta(days=30)

        self.run_main(old)
        self.assertEqual(asked, ["serde"])

    def test_a_check_that_cannot_ask_is_no_pass(self):
        self.assertEqual(self.run_main(RuntimeError("no network")), 2)


class Published(unittest.TestCase):
    def test_the_timestamp_of_crates_io_keeps_its_time_of_day(self):
        body = b'{"version": {"created_at": "2026-10-10T02:25:27.123456Z"}}'
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.read.return_value = body
        with mock.patch.object(age_check.urllib.request, "urlopen", return_value=response):
            moment = age_check.published("smallvec", "1.16.3")
        self.assertEqual(moment, datetime(2026, 10, 10, 2, 25, 27, 123456, tzinfo=timezone.utc))


if __name__ == "__main__":
    unittest.main()
