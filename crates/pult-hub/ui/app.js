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
const RETRY_MS = 2000;
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
  notice(`${why} Run "pult open" to get a new link to this page.`, true);
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

// A strip of an input or a bus; `update` fills it from each new state, so focus and a slider in
// the hand survive.
function makeStrip(address, title) {
  const li = make("li", "strip");
  const name = make("span", "name");
  const mute = make("button");
  mute.type = "button";
  mute.textContent = "Mute";
  const detail = make("span", "detail");
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

  let shown = { muted: false, gain_db: 0 };
  let held = false;
  mute.addEventListener("click", () => {
    ask({ op: "set", address: `${address}/mute`, value: !shown.muted });
  });
  slider.addEventListener("input", () => {
    held = true;
    gain.textContent = `${Number(slider.value).toFixed(1)} dB`;
  });
  slider.addEventListener("change", () => {
    const value = Number(slider.value);
    ask({ op: "set", address: `${address}/gain`, value }).finally(() => {
      held = false;
    });
  });

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
      meter.value = state.peak_db === null ? METER_FLOOR : state.peak_db;
      level.textContent = state.peak_db === null ? "silence" : `${state.peak_db.toFixed(1)} dB`;
      if (!held) {
        slider.value = state.gain_db;
        gain.textContent = `${state.gain_db.toFixed(1)} dB`;
      }
    },
  };
}

// Keeps one strip per key: a new key is a new strip, a key that went takes its strip along.
function showStrips(list, strips, wanted) {
  const keys = wanted.map((entry) => entry.key);
  const same = strips.size === keys.length && keys.every((key) => strips.has(key));
  if (!same) {
    strips.clear();
    list.replaceChildren();
    for (const entry of wanted) {
      const strip = makeStrip(entry.address, (state) => state.title);
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
  element("route").textContent = `Profile: ${state.profile ?? "none (from the command line)"}`;
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
      state: {
        ...bus,
        title: bus.label ?? bus.name,
        detail: `${bus.device}${bus.present ? "" : " (gone)"}`,
      },
    })),
  );
}

// ── following ────────────────────────────────────────────────────────────────────────────────

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

// Reads server-sent events until the stream ends, with limits a dead or endless stream cannot pass.
// `fetch`, since an EventSource can send no header.
async function followStream() {
  const stop = new AbortController();
  let idle = setTimeout(() => stop.abort(), IDLE_MS);
  try {
    const reply = await fetch(`${API}/stream`, { headers: authorized(), signal: stop.signal });
    if (reply.status === 401) {
      needsPairing("This page has no session (any more).");
      return;
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
        return;
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
        }
      }
    }
  } finally {
    clearTimeout(idle);
    stop.abort();
  }
}

let following = false;

// Follows the mixer while there is a session; only one of these runs at a time.
async function follow() {
  if (following) {
    return;
  }
  following = true;
  while (token !== null) {
    try {
      await followStream();
    } catch {
      // Told below, and tried again.
    }
    if (token !== null) {
      notice("pult does not answer: the route has ended, or is about to start.", true);
      await sleep(RETRY_MS);
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
