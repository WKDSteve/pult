-- The looks of a filled button of the remote control (the LIM switch of the first bus, switched on in the demo): at
-- rest, under the pointer and with the keyboard focus, light and dark. The colours themselves are computed and checked
-- by pult-hub's tests; this sheet leaves the shots for the judgement whether one sees the difference.
--   testing sheets step button-looks --headed
return {
  id = "pult.button-looks",
  title = "pult: a filled button at rest, under the pointer and with focus, light and dark",
  task = "pult/live-check-knoepfe-der-fernbedienung-hell-und-dunkel-in-ruhe",
  engine = "browser",
  app = "demo-hub",
  sizes = { { cols = 1000, rows = 1000 } },
  steps = {
    { scheme = "light" },
    { open = "${link}", note = "the link out of the terminal" },
    {
      wait = "document.querySelectorAll('#buses .strip').length === 2",
      timeout = 10000,
      note = "paired: two buses",
    },
    {
      expect = "document.querySelector('#buses .strip button.limit').getAttribute('aria-pressed') === 'true'",
      note = "the limiter of the first bus is on: a filled button",
    },

    { hover = "#buses-title", note = "the pointer away from the buttons, the buses in view" },
    { expect = "!document.querySelector('#buses .strip button.limit').matches(':hover')" },
    { shot = "filled-rest-light" },
    { hover = "#buses .strip button.limit" },
    { expect = "document.querySelector('#buses .strip button.limit').matches(':hover')" },
    { shot = "filled-hover-light" },
    { hover = "#buses-title" },
    { focus = "#buses .strip button.limit" },
    {
      expect = "document.querySelector('#buses .strip button.limit').matches(':focus-visible')",
      note = "the focus ring shows",
    },
    { shot = "filled-focus-light" },
    {
      expect = "(document.activeElement.blur(), document.activeElement === document.body)",
      note = "the focus off again",
    },

    { scheme = "dark" },
    { expect = "matchMedia('(prefers-color-scheme: dark)').matches", note = "the page sees the dark scheme" },
    { shot = "filled-rest-dark" },
    { hover = "#buses .strip button.limit" },
    { expect = "document.querySelector('#buses .strip button.limit').matches(':hover')" },
    { shot = "filled-hover-dark" },
    { hover = "#buses-title" },
    { focus = "#buses .strip button.limit" },
    {
      expect = "document.querySelector('#buses .strip button.limit').matches(':focus-visible')",
      note = "the focus ring shows",
    },
    { shot = "filled-focus-dark" },
  },
  judge = {
    "Under the pointer the filled LIM button is clearly lighter or darker than at rest, light and dark",
    "With the focus a ring around it is plainly visible, light and dark",
    "Its text stays readable in every state",
  },
}
