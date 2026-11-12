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
            argv = ["check-lock-age.py", "--lock", path, "--min-days", "7", "--cache", os.path.join(folder, "c.json")]
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


class Cache(unittest.TestCase):
    def run_twice(self, first, second):
        """Two runs on one cache file; what each asked crates.io for."""
        with tempfile.TemporaryDirectory() as folder:
            lock = os.path.join(folder, "Cargo.lock")
            with open(lock, "w", encoding="utf-8") as handle:
                handle.write(LOCK)
            argv = ["x", "--lock", lock, "--cache", os.path.join(folder, "sub", "c.json")]
            results = []
            for published in (first, second):
                asked = []

                def ask(name, version, published=published, asked=asked):
                    asked.append(name)
                    return published(name, version)

                with mock.patch.object(sys, "argv", argv), mock.patch.object(age_check, "published", side_effect=ask),                         mock.patch.object(age_check.time, "sleep"), mock.patch("builtins.print"):
                    results.append((age_check.main(), asked))
            return results

    def test_a_known_date_is_not_asked_again(self):
        old = lambda name, version: datetime.now(timezone.utc) - timedelta(days=30)  # noqa: E731
        never = lambda name, version: (_ for _ in ()).throw(AssertionError("asked again"))  # noqa: E731
        (first, asked_first), (second, asked_second) = self.run_twice(old, never)
        self.assertEqual((first, asked_first), (0, ["serde"]))
        self.assertEqual((second, asked_second), (0, []))

    def test_a_cached_young_version_is_still_too_young(self):
        young = lambda name, version: datetime.now(timezone.utc) - timedelta(days=1)  # noqa: E731
        never = lambda name, version: (_ for _ in ()).throw(AssertionError("asked again"))  # noqa: E731
        (first, _), (second, asked) = self.run_twice(young, never)
        self.assertEqual((first, second, asked), (1, 1, []))

    def test_a_broken_cache_file_is_an_empty_cache(self):
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "c.json")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write("{not json")
            self.assertEqual(age_check.load_cache(path), {})
            with open(path, "w", encoding="utf-8") as handle:
                handle.write('["a list"]')
            self.assertEqual(age_check.load_cache(path), {})


def too_many(retry_after=None):
    headers = {"Retry-After": retry_after} if retry_after is not None else {}
    return age_check.urllib.error.HTTPError("https://crates.io", 429, "Too Many Requests", headers, None)


def unavailable():
    return age_check.urllib.error.HTTPError("https://crates.io", 503, "Service Unavailable", {}, None)


def answer():
    body = b'{"version": {"created_at": "2026-10-10T02:25:27Z"}}'
    response = mock.MagicMock()
    response.__enter__.return_value = response
    response.read.return_value = body
    return response


class TooManyRequests(unittest.TestCase):
    def test_a_429_is_waited_out_with_the_pause_crates_io_names(self):
        pauses = []
        with mock.patch.object(age_check.urllib.request, "urlopen", side_effect=[too_many("7"), answer()]):
            moment = age_check.published("x11-dl", "2.21.0", sleep=pauses.append)
        self.assertEqual(pauses, [7])
        self.assertEqual(moment.year, 2026)

    def test_without_a_named_pause_the_backoff_grows(self):
        pauses = []
        failures = [too_many(), too_many("soon"), too_many()]
        with mock.patch.object(age_check.urllib.request, "urlopen", side_effect=[*failures, answer()]):
            age_check.published("x11-dl", "2.21.0", sleep=pauses.append)
        self.assertEqual(pauses, list(age_check.BACKOFF[:3]))

    def test_a_long_named_pause_is_capped(self):
        pauses = []
        with mock.patch.object(age_check.urllib.request, "urlopen", side_effect=[too_many("3600"), answer()]):
            age_check.published("x11-dl", "2.21.0", sleep=pauses.append)
        self.assertEqual(pauses, [age_check.MAX_RETRY_AFTER])

    def test_a_503_is_waited_out_like_a_429(self):
        pauses = []
        with mock.patch.object(age_check.urllib.request, "urlopen", side_effect=[unavailable(), answer()]):
            moment = age_check.published("x11-dl", "2.21.0", sleep=pauses.append)
        self.assertEqual(pauses, [age_check.BACKOFF[0]])
        self.assertEqual(moment.year, 2026)

    def test_another_error_is_raised_at_once(self):
        pauses = []
        gone = age_check.urllib.error.HTTPError("https://crates.io", 404, "Not Found", {}, None)
        with mock.patch.object(age_check.urllib.request, "urlopen", side_effect=[gone, answer()]):
            with self.assertRaises(age_check.urllib.error.HTTPError):
                age_check.published("x11-dl", "2.21.0", sleep=pauses.append)
        self.assertEqual(pauses, [])

    def test_after_the_last_pause_the_429_is_raised(self):
        pauses = []
        failures = [too_many() for _ in range(len(age_check.BACKOFF) + 1)]
        with mock.patch.object(age_check.urllib.request, "urlopen", side_effect=failures):
            with self.assertRaises(age_check.urllib.error.HTTPError):
                age_check.published("x11-dl", "2.21.0", sleep=pauses.append)
        self.assertEqual(len(pauses), len(age_check.BACKOFF))


class CacheTrust(unittest.TestCase):
    def test_a_date_without_a_time_zone_is_asked_again(self):
        cache = {"x11-dl 2.21.0": "2020-01-01T00:00:00"}
        asked = []

        def ask(name, version):
            asked.append(name)
            return datetime(2026, 10, 10, tzinfo=timezone.utc)

        moment, fresh = age_check.published_cached("x11-dl", "2.21.0", cache, ask=ask)
        self.assertEqual(asked, ["x11-dl"])
        self.assertTrue(fresh)
        self.assertEqual(moment.year, 2026)
        self.assertEqual(cache["x11-dl 2.21.0"], "2026-10-10T00:00:00+00:00")

    def test_the_default_cache_is_the_workspace_s_not_the_account_s(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("LOCK_AGE_CACHE", None)
            with tempfile.TemporaryDirectory() as folder:
                lock = os.path.join(folder, "Cargo.lock")
                path = age_check.default_cache(lock)
                self.assertEqual(path, os.path.join(folder, "target", "lock-age-cache.json"))
                self.assertFalse(path.startswith(os.path.expanduser("~") + os.sep + ".cache"))

    def test_lock_age_cache_still_names_another_file(self):
        with mock.patch.dict(os.environ, {"LOCK_AGE_CACHE": "elsewhere.json"}):
            self.assertEqual(age_check.default_cache("Cargo.lock"), "elsewhere.json")


if __name__ == "__main__":
    unittest.main()
