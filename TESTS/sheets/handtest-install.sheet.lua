-- The open hand test of slices 3 and 4 of stufe-3-ui: the rolling zip on the real machine. A person does it; the sheet asks.
return {
  id = "pult.handtest-install",
  title = "pult: install, start with Windows, update, remove",
  task = "pult/stufe-3-ui",
  engine = "manual",
  steps = {
    { ask = "gh release download latest -R WKDSteve/pult, unpacked, install.ps1: did it install?" },
    { ask = "The start menu opens the window?" },
    { ask = "The tray icon is there, and closing the window keeps the sound?" },
    { ask = "pult reopens the last profile without an argument?" },
    { ask = "Options, Start with Windows on, Start in the background on: after signing in again pult is in the tray?" },
    { ask = "Start in the background off: after signing in again the window is open?" },
    { ask = "Quit, start again: Options still show both choices as you left them?" },
    { ask = "Start with Windows off: after signing in again pult does not start?" },
    { ask = "An update while pult runs: the sound goes on?" },
    { ask = "The tile in the hub starts pult?" },
    { ask = "Settings > Apps > pult removes everything?" },
    { ask = "After installing again, start with Windows is off?" },
  },
}
