// The remote control page in a real Chrome against the demo hub (crates/pult-hub/examples): pairing,
// the link out of the address, the live state, mute, gain, scenes, refusals, a hostile name, no
// violation of the policy, and axe in light and dark. pult itself has no Node dependency: Playwright
// and axe come at fixed versions from the folder tools/page-check, which nothing else reads.
//
//   pnpm --dir tools/page-check install --frozen-lockfile
//   cargo build -p pult-hub --example demo-hub --locked
//   node tools/page-check.mjs tools/page-check/node_modules target/debug/examples/demo-hub.exe
//
// usage: node tools/page-check.mjs <node_modules with @playwright/test and @axe-core/playwright> <demo-hub.exe>

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [modules, hubProgram] = process.argv.slice(2);
if (!modules || !hubProgram) {
  console.error("usage: node tools/page-check.mjs <node_modules> <demo-hub.exe>");
  process.exit(2);
}
const require = createRequire(join(resolve(modules), "x.js"));
const { chromium } = require("@playwright/test");
const { AxeBuilder } = require("@axe-core/playwright");

// PULT_PAGE_SHOTS=<folder>: also a screenshot of each theme there, to look at the colours by hand.
async function shot(name) {
  if (process.env.PULT_PAGE_SHOTS) {
    await page.screenshot({ path: join(process.env.PULT_PAGE_SHOTS, `pult-${name}.png`), fullPage: true });
  }
}

const failed = [];
function check(what, ok, detail = "") {
  console.log(`${ok ? "ok    " : "FAILED"} ${what}${ok ? "" : ` ${detail}`}`);
  if (!ok) {
    failed.push(what);
  }
}

const config = mkdtempSync(join(tmpdir(), "pult-page-check-"));
const hub = spawn(hubProgram, [], {
  env: { ...process.env, PULT_CONFIG_DIR: config, PULT_PORT: "0" },
  stdio: ["pipe", "pipe", "inherit"],
});
const link = await new Promise((done, fail) => {
  let out = "";
  hub.stdout.on("data", (chunk) => {
    out += chunk;
    const line = out.split("\n").find((text) => text.startsWith("http"));
    if (line) {
      done(line.trim());
    }
  });
  hub.on("exit", (code) => fail(new Error(`the hub ended: ${code}`)));
});
const origin = new URL(link).origin;

// PULT_CHROME=<path>: a Chrome or Chromium binary of its own (a Linux box without the Chrome channel).
const browser = await chromium.launch(
  process.env.PULT_CHROME
    ? { executablePath: process.env.PULT_CHROME, headless: true, args: ["--no-sandbox"] }
    : { channel: "chrome", headless: true },
);
const context = await browser.newContext();
const page = await context.newPage();
const problems = [];
page.on("console", (message) => {
  if (message.type() === "error") {
    problems.push(message.text());
  }
});
page.on("pageerror", (error) => problems.push(String(error)));
const away = [];
page.on("request", (request) => {
  if (!request.url().startsWith(origin) && !request.url().startsWith("data:")) {
    away.push(request.url());
  }
});

try {
  // Without a link there is nothing but the way to one.
  await page.goto(`${origin}/`);
  await page.getByText('Run "pult open"').waitFor();
  check("without a link it says how to get one", await page.locator("#mixer").isHidden());
  check("the command is not for a translator",
    (await page.locator("#notice span").evaluate((node) => node.translate)) === false);
  problems.length = 0; // The refused stream is not looked for, but a browser may log it.

  await page.goto(link);
  await page.locator("#inputs .strip").first().waitFor();
  check("the code leaves the address", !page.url().includes("pair"), page.url());
  const stored = await page.evaluate(() => [
    sessionStorage.length,
    localStorage.length,
    document.cookie,
  ]);
  check("the token is in this tab only", stored[0] === 1 && stored[1] === 0 && stored[2] === "");
  check("three inputs, two buses", (await page.locator("#inputs .strip").count()) === 3 &&
    (await page.locator("#buses .strip").count()) === 2);
  const route = await page.locator("#route").textContent();
  check("the route is named", route.includes("pult.toml"), route);

  // Names of devices, profiles and scenes, the command and the mark are not for a translator.
  const untranslated = await page.evaluate(() => ({
    mark: document.querySelector("h1").translate,
    name: document.querySelector(".strip .name").translate,
    detail: document.querySelector(".strip .detail").translate,
    profile: document.querySelector("#route span").translate,
    scene: document.querySelector("#scenes button").translate,
    words: document.querySelector("#route").translate,
  }));
  check("names stay as they are for a translator",
    untranslated.mark === false && untranslated.name === false && untranslated.detail === false &&
    untranslated.profile === false && untranslated.scene === false && untranslated.words === true,
    JSON.stringify(untranslated));

  // The status line changes every second: its digits keep one width, and the headings wrap evenly.
  const looks = await page.evaluate(() => [
    getComputedStyle(document.getElementById("health")).fontVariantNumeric,
    getComputedStyle(document.querySelector("h2:not([hidden])")).textWrapStyle,
  ]);
  check("the status line has digits of one width", looks[0].includes("tabular-nums"), looks[0]);
  check("headings wrap evenly", looks[1] === "balance", looks[1]);

  // The bar of the browser takes the background of the page, in either scheme; a tap is not waited for.
  const bar = async () =>
    page.evaluate(() => [
      document.querySelector('meta[name="theme-color"]')?.content,
      getComputedStyle(document.body).backgroundColor,
      getComputedStyle(document.body).touchAction,
      getComputedStyle(document.body).webkitTapHighlightColor,
    ]);
  const seenBars = [];
  for (const colorScheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme });
    await page.waitForTimeout(50);
    const [content, background, touch, flash] = await bar();
    check(`${colorScheme}: the bar of the browser follows the page`,
      Boolean(content) && content === background, `${content} / ${background}`);
    check(`${colorScheme}: a tap is not waited for`, touch === "manipulation", touch);
    check(`${colorScheme}: the browser's flash is off`, flash === "rgba(0, 0, 0, 0)", flash);
    seenBars.push(content);
  }
  check("the two schemes have two bars", seenBars[0] !== seenBars[1], seenBars.join(" | "));
  await page.emulateMedia({ colorScheme: "light" });

  // With the flash gone, a press shows itself: the background of a button changes while it is down.
  const scene = page.getByRole("button", { name: "Laut", exact: true });
  const before = await scene.evaluate((node) => getComputedStyle(node).backgroundColor);
  const spot = await scene.boundingBox();
  await page.mouse.move(spot.x + spot.width / 2, spot.y + spot.height / 2);
  await page.mouse.down();
  const during = await scene.evaluate((node) => getComputedStyle(node).backgroundColor);
  await page.mouse.up();
  await page.mouse.move(0, 0);
  check("a press shows itself", before !== during, `${before} / ${during}`);

  // What the mixer calls things stays text.
  const hostile = page.locator("#inputs .strip").nth(2).locator(".name");
  check("a hostile name is text", (await hostile.textContent()).startsWith("<img"));
  check("and nothing of it ran", (await page.locator("img").count()) === 0 &&
    (await page.title()) === "pult");

  // Live: the meters of the demo keep moving.
  const level = page.locator("#inputs .strip").first().locator(".level").first();
  const seen = new Set();
  for (let i = 0; i < 10; i += 1) {
    seen.add(await level.textContent());
    await page.waitForTimeout(150);
  }
  check("the levels are live", seen.size > 2, [...seen].join(" | "));

  // A mute comes back through the state, not from the click.
  const mute = page.getByRole("button", { name: "Mute CABLE" });
  check("starts unmuted", (await mute.getAttribute("aria-pressed")) === "false");
  await mute.click();
  await page.waitForFunction(
    () => document.querySelector("#inputs .strip button").getAttribute("aria-pressed") === "true",
  );
  check("mute arrives", true);
  await mute.click();
  await page.waitForFunction(
    () => document.querySelector("#inputs .strip button").getAttribute("aria-pressed") === "false",
  );

  // Gain, through the slider.
  const slider = page.getByRole("slider", { name: "Gain of A1" });
  await slider.fill("-12");
  await page.waitForFunction(() =>
    document.getElementById("notice").textContent.includes("-12.0 dB"),
  );
  check("gain arrives", true);

  // A page nobody sees closes its stream and opens it again when seen: the levels stop and go on.
  const streams = [];
  const streamEnds = [];
  page.on("request", (request) => {
    if (request.url().endsWith("/api/pult/stream")) {
      streams.push(Date.now());
    }
  });
  page.on("requestfailed", (request) => {
    if (request.url().endsWith("/api/pult/stream")) {
      streamEnds.push(Date.now());
    }
  });
  const show = (hidden) =>
    page.evaluate((hidden) => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
      document.dispatchEvent(new Event("visibilitychange"));
    }, hidden);
  const meter = page.locator("#inputs .strip").first().locator(".level").first();
  const opened = streams.length;
  await show(true);
  await page.waitForTimeout(300);
  check("a hidden page closes its stream", streamEnds.length === 1, String(streamEnds.length));
  const frozen = await meter.textContent();
  await page.waitForTimeout(3500); // Longer than the first two pauses of a retry.
  check("and does not ask again while hidden", streams.length === opened, `${streams.length - opened} new`);
  check("and the levels hold still", (await meter.textContent()) === frozen);
  check("and does not call a closed stream an error",
    !(await page.locator("#notice").textContent()).includes("does not answer"));
  await show(false);
  await page.waitForTimeout(600);
  check("a page seen again opens it at once", streams.length === opened + 1, String(streams.length - opened));
  const live = new Set();
  for (let i = 0; i < 10; i += 1) {
    live.add(await meter.textContent());
    await page.waitForTimeout(150);
  }
  check("and the levels move again", live.size > 2, [...live].join(" | "));

  // Asks past the page, like another client: what a slider then shows came through the state.
  const setOutside = (address, value) =>
    page.evaluate(
      async ({ address, value }) => {
        const token = sessionStorage.getItem("pult.session");
        const reply = await fetch("/api/pult/ops", {
          method: "POST",
          headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
          body: JSON.stringify({ op: "set", address, value }),
        });
        return reply.status;
      },
      { address, value },
    );
  const follows = async (slider, value) => {
    for (let tries = 0; tries < 50; tries += 1) {
      if ((await slider.inputValue()) === value) {
        return true;
      }
      await new Promise((done) => setTimeout(done, 100));
    }
    return false;
  };

  // The one knob of gate and compressor, each to its own address and back through the state.
  for (const [what, word, address] of [
    ["Gate", "gate", "input/1/gate"],
    ["Compressor", "comp", "input/1/comp"],
  ]) {
    const knob = page.getByRole("slider", { name: `${what} of CABLE` });
    check(`the ${word} starts off`, (await knob.inputValue()) === "0");
    await knob.fill("4");
    await page.waitForFunction(
      (text) => document.getElementById("notice").textContent.includes(text),
      `${word} 4.0`,
    );
    check(`the ${word} arrives`, true);
    const status = await setOutside(address, 7);
    check(`the ${word} follows another client`, status === 200 && (await follows(knob, "7")));
    await setOutside(address, 0);
    check(`the ${word} goes off again`, await follows(knob, "0"));
  }

  // Let go where it started: a browser fires no change then, and the slider follows all the same.
  const knob = page.getByRole("slider", { name: "Compressor of CABLE" });
  await knob.scrollIntoViewIfNeeded();
  const box = await knob.boundingBox();
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + 8, y);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, y, { steps: 4 });
  await page.mouse.move(box.x + 8, y, { steps: 4 });
  await page.mouse.up();
  await setOutside("input/1/comp", 3);
  check("a slider let go where it started still follows", await follows(knob, "3"));
  await setOutside("input/1/comp", 0);
  await follows(knob, "0");

  // The switch of a limiter comes back through the state too.
  const limit = page.getByRole("button", { name: "LIM of A1" });
  check("the limiter starts on", (await limit.getAttribute("aria-pressed")) === "true");
  await limit.click();
  await page.waitForFunction(
    () => document.querySelector("#buses .strip .limit").getAttribute("aria-pressed") === "false",
  );
  check("the limiter switches", true);
  await limit.click();
  await page.waitForFunction(
    () => document.querySelector("#buses .strip .limit").getAttribute("aria-pressed") === "true",
  );

  // The switch of an equalizer comes back through the state too.
  const eq = page.getByRole("button", { name: "EQ of A1" });
  check("the equalizer starts on", (await eq.getAttribute("aria-pressed")) === "true");
  await eq.click();
  await page.waitForFunction(
    () => document.querySelector("#buses .strip .eq").getAttribute("aria-pressed") === "false",
  );
  check("the equalizer switches", true);
  await eq.click();
  await page.waitForFunction(
    () => document.querySelector("#buses .strip .eq").getAttribute("aria-pressed") === "true",
  );

  // A scene.
  await page.getByRole("button", { name: "Abend", exact: true }).click();
  await page.waitForFunction(() =>
    document.getElementById("notice").textContent.includes("scene Abend"),
  );
  await page.waitForFunction(
    () => document.querySelector("#inputs .strip input").value === "-12",
  );
  check("a scene arrives and moves the fader", true);

  // A refusal is shown as the mixer said it: taking an input off its only bus.
  const refused = await page.evaluate(async () => {
    const token = sessionStorage.getItem("pult.session");
    const reply = await fetch("/api/pult/ops", {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ op: "set", address: "matrix/1/A1", value: false }),
    });
    return reply.status;
  });
  check("a refusal is a refusal", refused === 422, String(refused));

  // Another use of the link changes nothing: the session stays.
  await page.goto(link);
  await page.locator("#inputs .strip").first().waitFor();
  check("a used link leaves a living session", (await page.locator("#mixer").isVisible()));
  problems.length = 0; // The refused exchange of that link.

  // A reload keeps the session, and the policy holds.
  await page.reload();
  await page.locator("#inputs .strip").first().waitFor();
  const policy = (await page.request.get(`${origin}/`)).headers()["content-security-policy"];
  check("the page has the strict policy", policy.includes("script-src 'self'") &&
    policy.includes("require-trusted-types-for 'script'"), policy);
  const blocked = await page.evaluate(() => {
    try {
      document.body.insertAdjacentHTML("beforeend", "<b>x</b>");
      return false;
    } catch {
      return true;
    }
  });
  check("a string as markup is refused", blocked);
  problems.length = 0; // The refusal just provoked.

  // A state of the stub below: the stream answered with what the test says, in a narrow window.
  const longName = "x".repeat(90);
  const stub = async (state) => {
    await page.route("**/api/pult/stream", (route) =>
      route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: `event: state\ndata: ${JSON.stringify(state)}\n\n`,
      }),
    );
    await page.reload();
  };
  const bare = { elapsed_s: 3725, dropouts_per_hour: 0, glitches: 0, scenes: [], inputs: [], buses: [] };
  await page.setViewportSize({ width: 320, height: 640 });
  await stub({ ...bare, profile: `${longName}.toml`, scenes: [longName] });
  await page.locator("#route").waitFor();
  await page.waitForFunction(() => !document.getElementById("mixer").hidden);
  check("no inputs say so", await page.locator("#inputs-empty").isVisible());
  check("no buses say so", await page.locator("#buses-empty").isVisible());
  check("long names of a profile and a scene wrap", await page.evaluate(
    () => document.documentElement.scrollWidth <= document.documentElement.clientWidth));
  await page.unroute("**/api/pult/stream");
  await stub({
    ...bare,
    inputs: [{ number: 1, name: longName, label: null, present: true, buses: [longName], peak_db: null,
      gain_db: 0, muted: false }],
    buses: [{ name: "A1", label: longName, device: longName, present: true, peak_db: null, gain_db: 0,
      muted: false, eq_on: false, limit_on: false }],
  });
  await page.locator("#inputs .strip").first().waitFor();
  check("a strip with nothing to say hides the empty text", await page.locator("#inputs-empty").isHidden());
  check("long names of a device and a bus wrap", await page.evaluate(
    () => document.documentElement.scrollWidth <= document.documentElement.clientWidth));
  await page.unroute("**/api/pult/stream");
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.reload();
  await page.locator("#inputs .strip").first().waitFor();
  problems.length = 0;

  // A hub that does not answer is asked less and less often: gaps of about 1, 2, 4 s.
  const asked = [];
  await page.route("**/api/pult/stream", (route) => {
    asked.push(Date.now());
    return route.fulfill({ status: 503, body: "" });
  });
  await page.reload();
  await page.waitForFunction(() => document.getElementById("notice").textContent.includes("does not answer"));
  while (asked.length < 4) {
    await page.waitForTimeout(250);
  }
  const gaps = asked.slice(1).map((time, i) => time - asked[i]);
  check("the first retry is quick", gaps[0] >= 900 && gaps[0] < 1800, gaps.join(" "));
  check("a hub that stays away is asked less often", gaps[1] >= gaps[0] * 1.6 && gaps[2] >= gaps[1] * 1.6,
    gaps.join(" "));
  await page.unroute("**/api/pult/stream");
  await page.reload();
  await page.locator("#inputs .strip").first().waitFor();
  problems.length = 0; // The refusals above.

  // axe in both themes: as the page is, with a button under the pointer, and with one focused from the keyboard
  // (the colours of pult come from palette.css, generated from its icon: the accent, the hover and the focus ring
  // are all its own).
  const axe = async (what) => {
    const { violations } = await new AxeBuilder({ page }).analyze();
    check(`axe, ${what}`, violations.length === 0,
      JSON.stringify(violations.map((v) => [v.id, v.nodes.map((n) => n.target)])));
  };
  for (const colorScheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme });
    await page.getByRole("button", { name: "Laut", exact: true }).click();
    await page.waitForTimeout(200);
    await axe(colorScheme);
    await page.locator("#inputs .strip button").first().hover();
    await axe(`${colorScheme}, hover`);
    // Axe is content with a button that does not react, so the colours are compared here.
    await page.mouse.move(0, 0);
    const plain = page.getByRole("button", { name: "Laut", exact: true });
    const filled = page.getByRole("button", { name: "EQ of A1" });
    const look = (button, property) =>
      button.evaluate((node, name) => getComputedStyle(node)[name], property);
    const plainBorder = await look(plain, "borderTopColor");
    const filledFill = await look(filled, "backgroundColor");
    await plain.hover();
    check(`${colorScheme}: a button shows the pointer`,
      (await look(plain, "borderTopColor")) !== plainBorder);
    await filled.hover();
    check(`${colorScheme}: a pressed button shows the pointer`,
      (await look(filled, "backgroundColor")) !== filledFill);
    await axe(`${colorScheme}, hover on a pressed button`);
    await filled.focus();
    await axe(`${colorScheme}, hover and focus on a pressed button`);
    await page.mouse.move(0, 0);
    await page.locator("#inputs .strip button").first().focus();
    await page.keyboard.press("Shift+Tab");
    await page.keyboard.press("Tab");
    await axe(`${colorScheme}, focus`);
    await shot(colorScheme);
  }

  // The hub goes: the page says so.
  hub.stdin.end("\n");
  await page.getByText("pult does not answer").waitFor({ timeout: 10_000 });
  check("an ended route is told", true);
  problems.length = 0; // Connection errors of the retries are what ending looks like.
  for (const colorScheme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme });
    await axe(`${colorScheme}, problem notice`);
    await shot(`${colorScheme}-problem`);
  }
  problems.length = 0;

  check("no error in the page", problems.length === 0, problems.join(" | "));
  check("nothing leaves this origin", away.length === 0, away.join(" | "));
} catch (error) {
  check("the run", false, String(error));
} finally {
  await browser.close();
  hub.kill();
  rmSync(config, { recursive: true, force: true });
}
console.log(failed.length === 0 ? "all fine" : `${failed.length} failed`);
process.exit(failed.length === 0 ? 0 : 1);
