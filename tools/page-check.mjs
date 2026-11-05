// The remote control page in a real Chrome against the demo hub (crates/pult-hub/examples): pairing,
// the link out of the address, the live state, mute, gain, scenes, refusals, a hostile name, no
// violation of the policy, and axe in light and dark. pult itself has no Node dependency, so
// Playwright and axe come from another checkout that has them.
//
// usage: node tools/page-check.mjs <node_modules with @playwright/test and @axe-core/playwright> <demo-hub.exe>

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [modules, hubProgram] = process.argv.slice(2);
if (!modules || !hubProgram) {
  console.error("usage: node tools/page-check.mjs <node_modules> <demo-hub.exe>");
  process.exit(2);
}
const require = createRequire(join(modules, "x.js"));
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

const browser = await chromium.launch({ channel: "chrome", headless: true });
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
