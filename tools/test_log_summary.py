"""Tests of tools/log-summary.py: `python -m unittest discover tools`."""

import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "log_summary", pathlib.Path(__file__).with_name("log-summary.py")
)
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


def row(elapsed, resyncs="0", glitches="0", dropouts="0", per_hour="0.00", drift="1.5"):
    return {
        "elapsed_s": elapsed,
        "in1_peak_db": "-6.0",
        "in1_drift_ppm": drift,
        "in1_resyncs": resyncs,
        "glitches": glitches,
        "dropouts": dropouts,
        "dropouts_per_hour": per_hour,
    }


class Changes(unittest.TestCase):
    def test_only_rises_of_counters_count(self):
        rows = [row("10.0"), row("20.0", drift="90.0"), row("30.0", resyncs="2", dropouts="1")]
        self.assertEqual(
            summary.changes(rows), [("30.0", {"in1_resyncs": 2.0, "dropouts": 1.0})]
        )

    def test_a_fall_is_the_new_base(self):
        rows = [row("10.0", glitches="5"), row("20.0", glitches="1"), row("30.0", glitches="2")]
        self.assertEqual(
            summary.changes(rows), [("10.0", {"glitches": 5.0}), ("30.0", {"glitches": 1.0})]
        )


class Report(unittest.TestCase):
    def test_ends_with_the_totals_of_the_last_row(self):
        rows = [row("3661.0", glitches="2", dropouts="1", per_hour="0.98")]
        self.assertEqual(
            summary.report(rows),
            ["1:01:01 glitches+2 dropouts+1", "ran 1:01:01: 1 dropouts (0.98 per hour), 2 glitches"],
        )

    def test_half_a_row_at_the_end_is_skipped(self):
        cut = row("20.0")
        cut["dropouts"] = None
        self.assertEqual(
            summary.report([row("10.0"), cut]), ["ran 0:00:10: 0 dropouts (0.00 per hour), 0 glitches"]
        )

    def test_a_log_without_rows_says_so(self):
        self.assertEqual(summary.report([]), ["no complete rows"])


if __name__ == "__main__":
    unittest.main()
