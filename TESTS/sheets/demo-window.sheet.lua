-- The window of pult with the demo desk, by the names a screen reader hears:
--   testing sheets step demo-window
-- It needs sheet-driver of appkit ($TESTING_DESKTOP_DRIVER), else it is a skip; not part of the CI (a window on the
-- runner is still an open question).
return {
  id = "pult.demo-window",
  title = "pult: the window with the demo desk",
  engine = "desktop",
  app = "demo-window",
  sizes = { { cols = 1000, rows = 800 } },
  steps = {
    { wait = { name = "Spotify.exe mute", state = "off" }, timeout = 15000, note = "the desk is there" },
    { toggle = "Spotify.exe mute", note = "mute one input" },
    { expect = { name = "Spotify.exe mute", state = "on" }, note = "it is muted" },
    { set = "Spotify.exe gain dB", value = -12, note = "pull its fader down" },
    { expect = { name = "Spotify.exe gain dB", value = -12 }, note = "the fader follows" },
    { set = "Spotify.exe pan", value = -0.5, note = "turn its pan to the left" },
    { expect = { name = "Spotify.exe pan", value = -0.5 }, note = "the knob follows" },
    { shot = "desk" },
    { click = "Matrix", note = "the other tab" },
    { wait = { name = "Spotify.exe to A1" }, note = "the matrix is there" },
    { shot = "matrix" },
  },
  judge = { "the mute of Spotify.exe is red and its fader stands visibly lower, in the picture desk" },
}
