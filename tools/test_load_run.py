"""Tests of tools/load-run.py: `python -m unittest discover tools`. Only the arguments, no audio device."""

import contextlib
import importlib.util
import io
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "load_run", pathlib.Path(__file__).with_name("load-run.py")
)
load_run = importlib.util.module_from_spec(spec)
spec.loader.exec_module(load_run)


def refused(argv):
    """Whether parsing `argv` ends with status 2, as argparse does on a wrong argument."""
    with contextlib.redirect_stderr(io.StringIO()):
        try:
            load_run.parse(argv)
        except SystemExit as end:
            return end.code == 2
    return False


class Arguments(unittest.TestCase):
    def test_without_options_pult_keeps_its_own_latency(self):
        args = load_run.parse(["pult.exe", "a", "out"])
        self.assertIsNone(args.latency_ms)
        self.assertEqual(args.quiet, load_run.QUIET)
        command = load_run.run_command(args.pult, "out/a.csv", args.latency_ms)
        self.assertNotIn("--latency-ms", command)
        self.assertEqual(command[1:3], ["run", "--from"])

    def test_the_latency_reaches_pult_run(self):
        args = load_run.parse(["pult.exe", "l20", "out", "--latency-ms", "20", "--quiet", "60"])
        self.assertEqual(args.quiet, 60)
        command = load_run.run_command(args.pult, "out/l20.csv", args.latency_ms)
        self.assertEqual(command[-2:], ["--latency-ms", "20"])
        self.assertIn("Voicemeeter In 5", command)

    def test_wrong_arguments_are_refused(self):
        self.assertTrue(refused(["pult.exe", "a"]))
        self.assertTrue(refused(["pult.exe", "a", "out", "--latency-ms", "0"]))
        self.assertTrue(refused(["pult.exe", "a", "out", "--latency-ms", "fast"]))
        self.assertTrue(refused(["pult.exe", "a", "out", "--quiet", "-1"]))


class Isolation(unittest.TestCase):
    def test_the_run_has_its_own_config_folder_and_port(self):
        env = load_run.run_env(pathlib.Path("out/config-a"))
        self.assertEqual(env["PULT_CONFIG_DIR"], str(pathlib.Path("out/config-a")))
        self.assertTrue(1 <= int(env["PULT_PORT"]) <= 65535)


if __name__ == "__main__":
    unittest.main()
