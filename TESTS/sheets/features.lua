-- TESTS/sheets/features.lua -- the functionalities of pult as its user meets them; `testing sheets report` says which
-- ones have a feature sheet (`feature = "<id>"`) and in which state, and K16 warns about a sheet that names an id not
-- listed here. Data: read with an empty environment like a sheet (testing.nvim docs/SHEETS.md).
-- The list follows the table "Was geht schon" of README.md, docs/USAGE.md and docs/CONTROL.md. pult has no command
-- surface for `testing surface`, so no entry names one.
return {
  {
    id = "pult.devices",
    title = "pult devices lists the audio devices, inputs and outputs",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.tone",
    title = "pult tone plays a test tone on a device to check the chain",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.mix",
    title = "Inputs are mixed and played on an output (pult run --from ... --to ...), with a gain per input",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.profiles",
    title = "Several buses with a routing matrix, gain, mute and pan per input, kept in a profile file",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.scenes",
    title = "Scenes in the profile switch gain, pan, mute, matrix and the effects without a dropout, crossfaded",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.programs",
    title = "The sound of one program as an input (--from app:Spotify), picked up again when it restarts",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.live-controls",
    title = "Controls while running: gain, mute, pan, show and save in an input line under the status line",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.meters",
    title = "Levels, dropouts, corrections, clipping and device glitches live, meters before or after the fader",
    docs = "README.md",
  },
  {
    id = "pult.drift",
    title = "Clock drift between devices and mixed sample rates are evened out by a resampler, without a click",
    docs = "README.md",
  },
  {
    id = "pult.hotplug",
    title = "A device unplugged while running: the rest plays on, and the device comes back by itself",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.dynamics",
    title = "Gate and compressor per input and a limiter per bus, each one control, in the profile and live",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.eq",
    title = "A parametric equalizer per bus with four bands, on the EQ tab of the window",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.window",
    title = "The window: the desk with faders, meters, solo, the matrix and the tabs, named for a screen reader",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.remote",
    title = "The remote control in the browser (pult open) drives the running pult",
    docs = "docs/CONTROL.md",
  },
  {
    id = "pult.button-states",
    title = "A button of the remote control shows rest, pointer and keyboard focus apart, light and dark",
    docs = "docs/CONTROL.md",
  },
  {
    id = "pult.commands",
    title = "pult <command> from outside, for example pult mute mikro, against the running pult",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.hub-tile",
    title = "The tile in hub-desktop shows pult and opens it",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.midi",
    title = "A MIDI controller learns its faders and keys (pult learn) and lights them",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.neovim",
    title = ":My pult in Neovim controls the running pult",
    docs = "docs/USAGE.md",
  },
  {
    id = "pult.install",
    title = "Install, start with Windows at sign-in, update and remove",
    docs = "docs/USAGE.md",
  },
}
