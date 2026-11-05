-- The hand test on the real route: what no driver can hear. A person does it; the sheet asks.
return {
  id = "pult.handtest-listen",
  title = "pult: listening, MIDI and the real window",
  task = "pult/handtest-hoeren-und-echtes-fenster",
  engine = "manual",
  steps = {
    { ask = "Music on a real route: ramps and scene changes without a click?" },
    { ask = "The APC's faders on a real route work?" },
    { ask = "The one offset correction about 2 s after routing in (aligned) cannot be heard?" },
    { ask = "The Scene menu works by hand in the real window?" },
    { ask = "MIDI in the real window: learning, faders, keys, lights (yellow is value 96), lights off on quit?" },
    { ask = "Remote control and MIDI together, a click on the hub page with real sound?" },
    { ask = ":My pult works against the real window?" },
    { ask = "pult start --open opens a real browser tab, and the tile in the running hub-desktop is right?" },
  },
}
