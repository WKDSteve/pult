// A remote control for the running pult, not a second mixer: it shows what the hub says and asks
// it for changes. Everything comes through the same interface other clients use (docs/CONTROL.md).
//
// Text from the mixer (device names, labels, messages) only ever goes into `textContent`. Nothing
// is shown as done before the hub said so: the next state brings the change.

const API = "/api/pult";
const TOKEN_KEY = "pult.session";
// The stream says something at least every 15 s; three silences mean it is dead.
const IDLE_MS = 45_000;
const MAX_BUFFER = 1024 * 1024;
// A hub that went away usually comes back within seconds (pult restarted, a profile reloaded), so the
// first retry is quick. A hub that stays away gets asked less and less often, but never rarer than every
// 30 s, so a page left open finds a pult that was started later within about that time. Up to a fifth is
// added at random, so tabs of one browser do not ask in step.
const RETRY_MIN_MS = 1000;
const RETRY_MAX_MS = 30_000;
const RETRY_SPREAD = 0.2;
const GAIN_MIN = -96;
const GAIN_MAX = 24;
const METER_FLOOR = -60;

const element = (id) => document.getElementById(id);

// The pairing code leaves the address before anything else happens.
function takePairingCode() {
  const hash = location.hash;
  if (hash === "") {
    return null;
  }
  history.replaceState(null, "", location.pathname + location.search);
  const match = /^#pair=([0-9a-f]{32,128})$/.exec(hash);
  return match ? match[1] : null;
}
const pairingCode = takePairingCode();

// Only in the storage of this tab; a browser that refuses it just asks to pair again on reload.
function storedToken() {
  try {
    return sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

function storeToken(value) {
  try {
    if (value === null) {
      sessionStorage.removeItem(TOKEN_KEY);
    } else {
      sessionStorage.setItem(TOKEN_KEY, value);
    }
  } catch {
    // It holds for this page then.
  }
}

let token = storedToken();

function notice(text, problem = false) {
  const box = element("notice");
  box.textContent = text;
  box.classList.toggle("problem", problem);
}

function needsPairing(why) {
  token = null;
  storeToken(null);
  element("mixer").hidden = true;
  notice(why, true);
  // The command is not a word of the page's language: a translator leaves it alone.
  const command = make("span", null, "pult open");
  command.translate = false;
  element("notice").append(' Run "', command, '" to get a new link to this page.');
}

function authorized(extra = {}) {
  return { Authorization: `Bearer ${token}`, ...extra };
}

async function exchange(code) {
  const reply = await fetch("/auth/exchange", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!reply.ok) {
    return false;
  }
  const answer = await reply.json();
  if (typeof answer.token !== "string") {
    return false;
  }
  token = answer.token;
  storeToken(token);
  return true;
}

// The colour of the browser's own bar (a phone's address bar) is the page's background, in the scheme
// the system is in. The tokens hold it as light-dark(), which a meta tag cannot read, so the resolved
// colour of the body is copied; the meta tag is made here, since its content cannot name a token.
function followThemeColor() {
  const meta = document.querySelector('meta[name="theme-color"]') ?? make("meta");
  meta.name = "theme-color";
  const copy = () => {
    meta.content = getComputedStyle(document.body).backgroundColor;
  };
  copy();
  document.head.append(meta);
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", copy);
}
followThemeColor();

// ── asking ───────────────────────────────────────────────────────────────────────────────────

let writing = Promise.resolve();

// One operation at a time, in the order they were asked for.
function ask(op) {
  writing = writing.then(() => send(op));
  return writing;
}

async function send(op) {
  let reply;
  try {
    reply = await fetch(`${API}/ops`, {
      method: "POST",
      headers: authorized({ "Content-Type": "application/json" }),
      body: JSON.stringify(op),
    });
  } catch {
    notice("pult did not answer; the change may not have happened.", true);
    return;
  }
  if (reply.status === 401) {
    needsPairing("This page has lost its session.");
    return;
  }
  const answer = await reply.json().catch(() => ({}));
  if (reply.ok) {
    notice((answer.lines ?? []).join("; "));
  } else {
    notice(answer.error?.message ?? `pult answered ${reply.status}.`, true);
  }
}

// ── showing ──────────────────────────────────────────────────────────────────────────────────

function make(tag, className, text) {
  const node = document.createElement(tag);
  if (className) {
    node.className = className;
  }
  if (text !== undefined) {
    node.textContent = text;
  }
  return node;
}

// Sends what `slider` is let go at. The result says whether a new state has to leave it alone: while
// it is in the hand, and until the last answer is there. A browser fires no `change` when the slider
// is let go where it started, so letting go is an event of its own; and every drag counts by itself,
// so an old answer does not take a new drag out of the hand.
function holdSlider(slider, send) {
  let inHand = false;
  let waiting = 0;
  const letGo = () => {
    inHand = false;
  };
  slider.addEventListener("pointerdown", () => {
    inHand = true;
    window.addEventListener("pointerup", letGo, { once: true });
    window.addEventListener("pointercancel", letGo, { once: true });
  });
  slider.addEventListener("input", () => {
    inHand = true;
  });
  slider.addEventListener("keyup", letGo);
  slider.addEventListener("blur", letGo);
  slider.addEventListener("change", () => {
    inHand = false;
    waiting += 1;
    send(Number(slider.value)).finally(() => {
      waiting -= 1;
    });
  });
  return () => inHand || waiting > 0;
}

// A small slider for the one knob of a gate or a compressor, 0 (off) to 10. Its `update` leaves it
// alone while it is in the hand.
function makeKnob(address, word, what) {
  const label = make("label", "knob");
  const name = make("span", "level", word);
  const slider = make("input");
  slider.type = "range";
  slider.min = 0;
  slider.max = 10;
  slider.step = 0.5;
  const value = make("span", "level");
  label.append(name, slider, value);
  const says = (amount) => (amount > 0 ? amount.toFixed(1) : "off");

  slider.addEventListener("input", () => {
    value.textContent = says(Number(slider.value));
  });
  const held = holdSlider(slider, (set) => ask({ op: "set", address, value: set }));

  return {
    node: label,
    update(amount, called) {
      slider.setAttribute("aria-label", `${what} of ${called}`);
      if (!held()) {
        slider.value = amount;
        value.textContent = says(amount);
      }
    },
  };
}

// A strip of an input or a bus; `update` fills it from each new state, so focus and a slider in
// the hand survive.
function makeStrip(address, title, isInput) {
  const li = make("li", "strip");
  const name = make("span", "name");
  // Names are the mixer's, not words to translate.
  name.translate = false;
  const mute = make("button");
  mute.type = "button";
  mute.textContent = "Mute";
  const detail = make("span", "detail");
  detail.translate = false;
  const meter = make("meter");
  meter.min = METER_FLOOR;
  meter.max = 0;
  const level = make("span", "level");
  const slider = make("input");
  slider.type = "range";
  slider.min = GAIN_MIN;
  slider.max = GAIN_MAX;
  slider.step = 0.5;
  const gain = make("span", "level");
  li.append(name, mute, detail, meter, level, slider, gain);

  // What an input has before its fader, and a bus behind its sum.
  const gate = isInput ? makeKnob(`${address}/gate`, "Gate", "Gate") : null;
  const comp = isInput ? makeKnob(`${address}/comp`, "Comp", "Compressor") : null;
  const eq = isInput ? null : make("button");
  const limit = isInput ? null : make("button");
  if (isInput) {
    li.append(gate.node, comp.node);
  } else {
    eq.type = "button";
    eq.className = "eq";
    eq.textContent = "EQ";
    limit.type = "button";
    limit.className = "limit";
    limit.textContent = "LIM";
    li.append(eq, limit);
    eq.addEventListener("click", () => {
      ask({ op: "set", address: `${address}/eq`, value: !shown.eq_on });
    });
    limit.addEventListener("click", () => {
      ask({ op: "set", address: `${address}/limit`, value: !shown.limit_on });
    });
  }

  let shown = { muted: false, gain_db: 0, eq_on: false, limit_on: false };
  mute.addEventListener("click", () => {
    ask({ op: "set", address: `${address}/mute`, value: !shown.muted });
  });
  slider.addEventListener("input", () => {
    gain.textContent = `${Number(slider.value).toFixed(1)} dB`;
  });
  const held = holdSlider(slider, (value) =>
    ask({ op: "set", address: `${address}/gain`, value }),
  );

  return {
    node: li,
    update(state) {
      shown = state;
      const called = title(state);
      name.textContent = called;
      detail.textContent = state.detail;
      mute.setAttribute("aria-pressed", String(state.muted));
      mute.setAttribute("aria-label", `Mute ${called}`);
      meter.setAttribute("aria-label", `Level of ${called}`);
      slider.setAttribute("aria-label", `Gain of ${called}`);
      if (isInput) {
        // A pult older than these says nothing of them: they read as off.
        gate.update(state.gate ?? 0, called);
        comp.update(state.comp ?? 0, called);
      } else {
        eq.setAttribute("aria-pressed", String(Boolean(state.eq_on)));
        eq.setAttribute("aria-label", `EQ of ${called}`);
        limit.setAttribute("aria-pressed", String(Boolean(state.limit_on)));
        limit.setAttribute("aria-label", `LIM of ${called}`);
      }
      meter.value = state.peak_db === null ? METER_FLOOR : state.peak_db;
      level.textContent = state.peak_db === null ? "silence" : `${state.peak_db.toFixed(1)} dB`;
      if (!held()) {
        slider.value = state.gain_db;
        gain.textContent = `${state.gain_db.toFixed(1)} dB`;
      }
    },
  };
}

// Keeps one strip per key: a new key is a new strip, a key that went takes its strip along.
function showStrips(list, strips, wanted) {
  element(`${list.id}-empty`).hidden = wanted.length > 0;
  const keys = wanted.map((entry) => entry.key);
  const same = strips.size === keys.length && keys.every((key) => strips.has(key));
  if (!same) {
    strips.clear();
    list.replaceChildren();
    for (const entry of wanted) {
      const strip = makeStrip(entry.address, (state) => state.title, entry.isInput);
      strips.set(entry.key, strip);
      list.append(strip.node);
    }
  }
  for (const entry of wanted) {
    strips.get(entry.key).update(entry.state);
  }
}

const inputStrips = new Map();
const busStrips = new Map();
let shownScenes = null;

function showScenes(scenes) {
  const text = JSON.stringify(scenes);
  if (text === shownScenes) {
    return;
  }
  shownScenes = text;
  element("scenes-section").hidden = scenes.length === 0;
  const row = element("scenes");
  row.replaceChildren();
  for (const name of scenes) {
    const button = make("button", null, name);
    button.type = "button";
    button.translate = false;
    button.addEventListener("click", () => {
      ask({ op: "scene", name });
    });
    row.append(button);
  }
}

function clock(seconds) {
  const whole = Math.floor(seconds);
  const part = (value) => String(value).padStart(2, "0");
  return `${Math.floor(whole / 3600)}:${part(Math.floor(whole / 60) % 60)}:${part(whole % 60)}`;
}

function show(state) {
  element("mixer").hidden = false;
  const route = element("route");
  if (state.profile === null || state.profile === undefined) {
    route.textContent = "Profile: none (from the command line)";
  } else {
    const profile = make("span", null, state.profile);
    profile.translate = false;
    route.replaceChildren("Profile: ", profile);
  }
  element("health").textContent =
    `Running for ${clock(state.elapsed_s)}, ` +
    `${state.dropouts_per_hour.toFixed(1)} dropouts per hour, ${state.glitches} glitches`;
  showScenes(state.scenes ?? []);
  showStrips(
    element("inputs"),
    inputStrips,
    state.inputs.map((input) => ({
      key: `${input.number}/${input.name}`,
      address: `input/${input.number}`,
      isInput: true,
      state: {
        ...input,
        title: input.label ?? input.name,
        detail:
          `${input.label === null ? "" : `${input.name} `}` +
          `to ${input.buses.join(", ")}${input.present ? "" : " (gone)"}`,
      },
    })),
  );
  showStrips(
    element("buses"),
    busStrips,
    state.buses.map((bus) => ({
      key: bus.name,
      address: `bus/${bus.name}`,
      isInput: false,
      state: {
        ...bus,
        title: bus.label ?? bus.name,
        detail: `${bus.device}${bus.present ? "" : " (gone)"}`,
      },
    })),
  );
}

// ── following ────────────────────────────────────────────────────────────────────────────────

let openStream = null;
let wakeUp = null;

// Waits `ms` (for ever when it is not finite), or until the page is shown or hidden.
function nap(ms) {
  return new Promise((done) => {
    const timer = Number.isFinite(ms) ? setTimeout(finish, ms) : null;
    function finish() {
      clearTimeout(timer);
      wakeUp = null;
      done();
    }
    wakeUp = finish;
  });
}

function retryDelay(failures) {
  const base = Math.min(RETRY_MAX_MS, RETRY_MIN_MS * 2 ** Math.min(failures, 16));
  return base * (1 + RETRY_SPREAD * Math.random());
}

// A page nobody sees needs no state: its stream (one of the four a session has, docs/CONTROL.md) is
// closed, and a new one is opened at once when the page is seen again. The levels come with the stream,
// so they stop with it.
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    openStream?.abort();
  }
  wakeUp?.();
});

// Reads server-sent events until the stream ends, with limits a dead or endless stream cannot pass.
// `fetch`, since an EventSource can send no header. The result says whether it showed a state.
async function followStream() {
  const stop = new AbortController();
  openStream = stop;
  let delivered = false;
  let idle = setTimeout(() => stop.abort(), IDLE_MS);
  try {
    const reply = await fetch(`${API}/stream`, { headers: authorized(), signal: stop.signal });
    if (reply.status === 401) {
      needsPairing("This page has no session (any more).");
      return false;
    }
    if (!reply.ok || !reply.body) {
      throw new Error(`stream: ${reply.status}`);
    }
    if (element("notice").textContent.startsWith("pult does not answer")) {
      notice("");
    }
    const reader = reply.body.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) {
        return delivered;
      }
      clearTimeout(idle);
      idle = setTimeout(() => stop.abort(), IDLE_MS);
      buffer += value;
      if (buffer.length > MAX_BUFFER) {
        throw new Error("stream: a message without an end");
      }
      for (let end = buffer.indexOf("\n\n"); end >= 0; end = buffer.indexOf("\n\n")) {
        const message = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        const data = message
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (data !== "") {
          show(JSON.parse(data));
          delivered = true;
        }
      }
    }
  } catch (error) {
    // Whatever was shown before the end counts for the pace of the retries.
    error.delivered = delivered;
    throw error;
  } finally {
    clearTimeout(idle);
    stop.abort();
    openStream = null;
  }
}

let following = false;

// Follows the mixer while there is a session; only one of these runs at a time.
async function follow() {
  if (following) {
    return;
  }
  following = true;
  let failures = 0;
  while (token !== null) {
    if (document.hidden) {
      await nap(Infinity);
      failures = 0;
      continue;
    }
    let delivered = false;
    try {
      delivered = await followStream();
    } catch (error) {
      // Told below, and tried again.
      delivered = error?.delivered === true;
    }
    if (document.hidden) {
      continue; // Closed on purpose, not a route that ended.
    }
    if (token !== null) {
      notice("pult does not answer: the route has ended, or is about to start.", true);
      // A stream that showed something and then ended starts the pace again; one that failed slows it.
      const wait = retryDelay(delivered ? 0 : failures);
      failures = delivered ? 1 : failures + 1;
      await nap(wait);
    }
  }
  following = false;
}

async function start(code) {
  if (code !== null) {
    let paired = false;
    try {
      paired = await exchange(code);
    } catch {
      paired = false;
    }
    // A used or old link does not take away a session that still lives.
    if (!paired && token === null) {
      needsPairing("This link is no longer valid.");
      return;
    }
    if (paired) {
      notice("");
    }
  }
  if (token === null) {
    needsPairing("This page has no session yet.");
    return;
  }
  await follow();
}

// A link opened in the tab that shows this page only changes the fragment, and loads nothing.
window.addEventListener("hashchange", () => {
  const code = takePairingCode();
  if (code !== null) {
    start(code);
  }
});

start(pairingCode);
