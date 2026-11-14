"""Tests of tools/click-run.py: `python -m unittest discover tools`. What needs no audio device."""

import importlib.util
import pathlib
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "click_run", pathlib.Path(__file__).with_name("click-run.py")
)
click_run = importlib.util.module_from_spec(spec)
spec.loader.exec_module(click_run)


def wav(frames, channels=2, rate=48_000, value=0.5):
    """A WAV file in 32-bit float as `pult record` writes it: 44 bytes of header, then samples."""
    data = struct.pack("<f", value) * (frames * channels)
    header = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt "
    header += struct.pack("<IHHIIHH", 16, 3, channels, rate, rate * channels * 4, channels * 4, 32)
    return header + b"data" + struct.pack("<I", len(data)) + data


class Profile(unittest.TestCase):
    def test_a_program_or_a_device_feeds_the_recorded_bus(self):
        text = click_run.profile_text("Voicemeeter In 5", None, "app:pulttone")
        self.assertIn('device = "Voicemeeter In 5"', text)
        self.assertIn('process = "pulttone"', text)
        self.assertIn('buses = ["A1"]', text)
        self.assertNotIn("A2", text)
        self.assertEqual(text.count("[[scene]]"), 2)
        text = click_run.profile_text("Focusrite", None, "CABLE Output")
        self.assertIn('device = "CABLE Output"', text)
        self.assertNotIn("process", text)

    def test_a_side_bus_keeps_the_input_for_the_matrix_button(self):
        text = click_run.profile_text("Voicemeeter In 5", "Voicemeeter In 4", "app:pulttone")
        self.assertIn('name = "A2"\ndevice = "Voicemeeter In 4"', text)
        self.assertIn('buses = ["A1", "A2"]', text)
        # The side bus comes before the input, so the input can name it.
        self.assertLess(text.index('name = "A2"'), text.index("[[input]]"))


class Sequence(unittest.TestCase):
    def test_every_change_is_a_command_and_the_matrix_takes_the_recorded_bus(self):
        for words in click_run.SEQUENCE + click_run.MATRIX + click_run.LAST:
            self.assertRegex(words, r"^[a-z]+ \S")
        self.assertEqual(click_run.MATRIX, ["unroute 1 A1", "route 1 A1"])
        # The recording ends loud, so its last seconds can be judged.
        self.assertEqual(click_run.LAST, ["scene laut"])


class Probe(unittest.TestCase):
    def test_the_cut_silences_five_milliseconds_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as folder:
            source = pathlib.Path(folder) / "a.wav"
            target = pathlib.Path(folder) / "b.wav"
            source.write_bytes(wav(48_000))
            click_run.cut_a_piece(source, target, 0.5)
            before, after = source.read_bytes(), target.read_bytes()
            self.assertEqual(len(before), len(after))
            self.assertEqual(before[:44], after[:44])
            start = 44 + 24_000 * 8
            length = 240 * 8
            self.assertEqual(after[start:start + length], bytes(length))
            self.assertEqual(after[:start], before[:start])
            self.assertEqual(after[start + length:], before[start + length:])

    def test_a_recording_too_short_for_the_cut_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            source = pathlib.Path(folder) / "a.wav"
            source.write_bytes(wav(100))
            with self.assertRaises(ValueError):
                click_run.cut_a_piece(source, pathlib.Path(folder) / "b.wav", 0.5)


class Floor(unittest.TestCase):
    LINE = ("21.00 s at 48000 Hz, 2 ch, judged from 6.56 s to 19.41 s: peak {} dBFS, "
            "floor {} dBFS (from the quiet start), steepest step 0.00 of what a 103 Hz tone can do")

    def test_a_floor_near_the_tone_cannot_be_judged(self):
        # What a forgotten tone on the recorded device did to a run: "no clicks", blindly.
        self.assertEqual(click_run.loud_floor(self.LINE.format("-5.0", "-13.8")), (-5.0, -13.8))
        self.assertEqual(click_run.loud_floor(self.LINE.format("-5.0", "-34.9")), (-5.0, -34.9))

    def test_a_quiet_start_is_fine(self):
        self.assertIsNone(click_run.loud_floor(self.LINE.format("-5.0", "-80.0")))
        self.assertIsNone(click_run.loud_floor(self.LINE.format("-5.0", "-inf")))
        self.assertIsNone(click_run.loud_floor(self.LINE.format("-5.0", "-35.0")))

    def test_a_line_without_levels_says_nothing(self):
        self.assertIsNone(click_run.loud_floor("no clicks"))
        self.assertIsNone(click_run.loud_floor(""))


class Port(unittest.TestCase):
    def test_a_free_port_is_a_port(self):
        self.assertTrue(1024 <= click_run.free_port() <= 65_535)


if __name__ == "__main__":
    unittest.main()
