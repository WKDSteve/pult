-- The remote control in a real browser against the demo hub, step by step:
--   testing sheets step remote-control --headed
-- tools/page-check.mjs goes further (refusals, the policy, axe); this one is the walk a person can watch.
return {
  id = "pult.remote-control",
  title = "pult: the remote control against the demo hub",
  engine = "browser",
  app = "demo-hub",
  sizes = { { cols = 1000, rows = 800 } },
  steps = {
    { open = "${link}", note = "the link out of the terminal" },
    {
      wait = "document.querySelectorAll('#inputs .strip').length === 3",
      timeout = 10000,
      note = "paired: three inputs",
    },
    { expect = "document.querySelectorAll('#buses .strip').length === 2", note = "two buses" },
    { expect = "!location.href.includes('pair')", note = "the code has left the address" },
    { shot = "mixer" },
    { click = "role=button[name=/^Mute CABLE/]", note = "mute the first input" },
    {
      wait = "document.querySelector('#inputs .strip button').getAttribute('aria-pressed') === 'true'",
      note = "the mute comes back through the state",
    },
    { shot = "muted" },
    { click = "role=button[name=/^Mute CABLE/]", note = "and off again" },
    {
      wait = "document.querySelector('#inputs .strip button').getAttribute('aria-pressed') === 'false'",
    },
    { click = 'role=button[name="Abend"]', note = "a scene" },
    {
      wait = "document.getElementById('notice').textContent.includes('scene Abend')",
      note = "the scene is confirmed",
    },
    { shot = "scene" },
  },
}
