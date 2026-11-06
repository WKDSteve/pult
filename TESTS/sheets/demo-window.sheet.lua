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
    { set = "Spotify.exe gate", value = 3, note = "a gate on it" },
    { expect = { name = "Spotify.exe gate", value = 3 }, note = "the dial follows" },
    { toggle = "A1 equalizer", note = "the equalizer of a bus off" },
    { expect = { name = "A1 equalizer", state = "off" }, note = "it is off" },
    { shot = "desk" },
    { click = "EQ", note = "the bands" },
    { set = "A2 equalizer band 2 gain", value = 6, note = "raise a band" },
    { expect = { name = "A2 equalizer band 2 gain", value = 6 }, note = "the dial follows" },
    { shot = "equalizer" },
    { click = "Matrix", note = "the other tab" },
    { wait = { name = "Spotify.exe to A1" }, note = "the matrix is there" },
    { shot = "matrix" },
    -- Only looked at: the app's APPDATA is a folder of the run, but the start with the system is the real one of this
    -- user, so a sheet never switches these two.
    { click = "Options", note = "the options" },
    { wait = { name = "Start in the background" }, note = "both choices are there" },
    { shot = "options" },
    { click = "Close", note = "the options go away again" },
    { wait = { name = "Save profile" }, note = "back at the desk" },
  },
  judge = { "the mute of Spotify.exe is red and its fader stands visibly lower, in the picture desk" },
}
