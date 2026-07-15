const INSTALL_CMD =
  "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/latest/download/stackless-installer.sh | sh";

const NODE_ORDER = ["clerk", "web", "db"];
const HERO_MS = 10000;
const FLEET_MS = 6000;
const FLEET_OFFSET = 2000;

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const copyButtons = Array.from(document.querySelectorAll("[data-copy]"));

async function copyText(text, button) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    document.execCommand("copy");
    document.body.removeChild(area);
  }

  const label = button.querySelector("[data-copy-label]");
  const previous = label?.textContent;
  const previousAria = button.getAttribute("aria-label");
  button.classList.add("is-copied");
  if (label) label.textContent = "Copied";
  if (previousAria) button.setAttribute("aria-label", "Copied");
  const check = button.querySelector(".copy-icon-check");
  if (check) check.hidden = false;
  window.setTimeout(() => {
    if (label && previous != null) label.textContent = previous;
    if (previousAria) button.setAttribute("aria-label", previousAria);
    button.classList.remove("is-copied");
    if (check) check.hidden = true;
  }, 1600);
}

function bindCopy() {
  for (const button of copyButtons) {
    button.addEventListener("click", () => {
      const source = button.getAttribute("data-copy");
      const text =
        source === "install"
          ? INSTALL_CMD
          : document.querySelector(source)?.textContent?.trim() || INSTALL_CMD;
      copyText(text, button);
    });
  }
}

/* —— shared conductor —— */

const Conductor = {
  t0: 0,
  paused: false,
  pauseAccum: 0,
  pauseStarted: 0,
  raf: 0,
  listeners: [],

  now() {
    if (this.paused) {
      return this.pauseStarted - this.t0 - this.pauseAccum;
    }
    return performance.now() - this.t0 - this.pauseAccum;
  },

  start() {
    this.t0 = performance.now();
    this.pauseAccum = 0;
    this.paused = false;
    const tick = () => {
      const t = this.now();
      for (const fn of this.listeners) fn(t);
      this.raf = requestAnimationFrame(tick);
    };
    this.raf = requestAnimationFrame(tick);
  },

  pause() {
    if (this.paused) return;
    this.paused = true;
    this.pauseStarted = performance.now();
  },

  resume() {
    if (!this.paused) return;
    this.pauseAccum += performance.now() - this.pauseStarted;
    this.paused = false;
  },

  on(fn) {
    this.listeners.push(fn);
  },
};

function applyCues(schedule, localT, fired, ctx) {
  for (let i = 0; i < schedule.length; i += 1) {
    const cue = schedule[i];
    if (localT >= cue.at && !fired.has(i)) {
      fired.add(i);
      cue.run(ctx);
    }
  }
}

/* —— hero demo —— */

function createHero(demo) {
  const cmdUp = demo.querySelector('[data-cmd="up"]');
  const cmdDown = demo.querySelector('[data-cmd="down"]');
  const cursor = demo.querySelector(".demo-cursor");
  const tomlBody = demo.querySelector(".demo-toml-body");
  const fired = new Set();
  let lastCycle = -1;

  function setPhase(phase) {
    demo.dataset.phase = phase;
  }

  function clearToml() {
    demo.querySelectorAll(".toml-line").forEach((el) => {
      el.classList.remove("is-lit", "was-lit");
    });
  }

  function pinToml(key, { keepOthers = false } = {}) {
    if (!keepOthers) {
      demo.querySelectorAll(".toml-line.is-lit").forEach((el) => {
        el.classList.remove("is-lit");
        el.classList.add("was-lit");
      });
    }
    const lines = demo.querySelectorAll(`.toml-line[data-hl="${key}"]`);
    lines.forEach((el) => {
      el.classList.remove("was-lit");
      el.classList.add("is-lit");
    });
    const anchor = demo.querySelector(`[data-toml-anchor="${key}"]`);
    if (anchor && tomlBody) {
      // Never scroll the page; only nudge the toml pane while the hero is on-screen.
      const hero = demo.closest(".hero");
      const heroRect = hero?.getBoundingClientRect();
      const heroVisible =
        heroRect &&
        heroRect.bottom > 80 &&
        heroRect.top < window.innerHeight * 0.85;
      if (!heroVisible) return;

      const bodyRect = tomlBody.getBoundingClientRect();
      const lineRect = anchor.getBoundingClientRect();
      if (lineRect.top < bodyRect.top || lineRect.bottom > bodyRect.bottom) {
        const nextTop =
          tomlBody.scrollTop +
          (lineRect.top - bodyRect.top) -
          bodyRect.height * 0.35;
        tomlBody.scrollTo({ top: Math.max(0, nextTop), behavior: "smooth" });
      }
    }
  }

  function lightAll() {
    NODE_ORDER.forEach((key) => {
      demo.querySelectorAll(`.toml-line[data-hl="${key}"]`).forEach((el) => {
        el.classList.remove("was-lit");
        el.classList.add("is-lit");
      });
    });
  }

  function setNode(key, state) {
    const node = demo.querySelector(`.node[data-node="${key}"]`);
    if (!node) return;
    node.classList.remove("is-on", "is-out", "is-active");
    if (state === "on") {
      node.classList.add("is-on", "is-active");
      demo.querySelectorAll(".node").forEach((el) => {
        if (el !== node) el.classList.remove("is-active");
      });
    }
    if (state === "out") node.classList.add("is-out");
  }

  function clearActive() {
    demo.querySelectorAll(".node.is-active").forEach((el) => {
      el.classList.remove("is-active");
    });
  }

  function hideCommands() {
    cmdUp.hidden = true;
    cmdDown.hidden = true;
    cmdUp.classList.remove("is-revealing");
    cmdDown.classList.remove("is-revealing");
    cursor.classList.remove("is-on");
  }

  function revealCommand(which) {
    hideCommands();
    const el = which === "up" ? cmdUp : cmdDown;
    el.hidden = false;
    // reflow so chunk transitions replay
    void el.offsetWidth;
    el.classList.add("is-revealing");
    cursor.classList.add("is-on");
  }

  function resetAll() {
    setPhase("idle");
    hideCommands();
    clearToml();
    clearActive();
    NODE_ORDER.forEach((key) => setNode(key, "off"));
  }

  function showStaticAlive() {
    setPhase("alive");
    hideCommands();
    cmdUp.hidden = false;
    cmdUp.classList.add("is-revealing");
    cursor.classList.remove("is-on");
    NODE_ORDER.forEach((key) => setNode(key, "on"));
    clearActive();
    lightAll();
  }

  const schedule = [
    { at: 0, run: () => resetAll() },
    {
      at: 420,
      run: () => {
        setPhase("paste-up");
        revealCommand("up");
      },
    },
    {
      at: 980,
      run: () => {
        setPhase("spawn");
        pinToml("clerk");
        setNode("clerk", "on");
      },
    },
    {
      at: 1580,
      run: () => {
        pinToml("web");
        setNode("web", "on");
      },
    },
    {
      at: 2180,
      run: () => {
        pinToml("db");
        setNode("db", "on");
      },
    },
    {
      at: 2780,
      run: () => {
        setPhase("lease");
        lightAll();
        clearActive();
        cursor.classList.remove("is-on");
      },
    },
    {
      at: 3280,
      run: () => {
        setPhase("hold");
      },
    },
    {
      at: 5600,
      run: () => {
        setPhase("paste-down");
        revealCommand("down");
      },
    },
    {
      at: 6000,
      run: () => {
        setPhase("teardown");
        cursor.classList.remove("is-on");
        clearToml();
        pinToml("db");
        setNode("db", "out");
      },
    },
    {
      at: 6400,
      run: () => {
        pinToml("web");
        setNode("web", "out");
      },
    },
    {
      at: 6800,
      run: () => {
        pinToml("clerk");
        setNode("clerk", "out");
      },
    },
    {
      at: 7300,
      run: () => {
        setPhase("cool");
        hideCommands();
        clearToml();
        clearActive();
        NODE_ORDER.forEach((key) => setNode(key, "off"));
      },
    },
  ];

  return {
    showStaticAlive,
    tick(t) {
      const cycle = Math.floor(t / HERO_MS);
      const local = t % HERO_MS;
      if (cycle !== lastCycle) {
        fired.clear();
        lastCycle = cycle;
      }
      applyCues(schedule, local, fired, null);
    },
  };
}

/* —— fleet —— */

function createFleet(root) {
  const lanes = Array.from(root.querySelectorAll("[data-fleet-lane]"));
  const fired = lanes.map(() => new Set());
  const prevLocal = lanes.map(() => -1);

  const laneSchedule = [
    { at: 0, run: (lane) => { lane.dataset.state = "idle"; } },
    { at: 480, run: (lane) => { lane.dataset.state = "up"; } },
    { at: 1400, run: (lane) => { lane.dataset.state = "alive"; } },
    { at: 3600, run: (lane) => { lane.dataset.state = "down"; } },
    { at: 4800, run: (lane) => { lane.dataset.state = "idle"; } },
  ];

  return {
    showStaticAlive() {
      lanes.forEach((lane) => {
        lane.dataset.state = "alive";
      });
    },
    tick(t) {
      lanes.forEach((lane, i) => {
        const phase = Number(lane.dataset.phase || i);
        const local = (t + phase * FLEET_OFFSET) % FLEET_MS;
        if (local < prevLocal[i]) {
          fired[i].clear();
        }
        prevLocal[i] = local;
        applyCues(laneSchedule, local, fired[i], lane);
      });
    },
  };
}

/* —— boot —— */

function boot() {
  bindCopy();

  const demo = document.getElementById("demo");
  const fleetRoot = document.getElementById("fleet");
  const hero = demo ? createHero(demo) : null;
  const fleet = fleetRoot ? createFleet(fleetRoot) : null;

  if (reduceMotion) {
    hero?.showStaticAlive();
    fleet?.showStaticAlive();
    return;
  }

  if (hero) Conductor.on((t) => hero.tick(t));
  if (fleet) Conductor.on((t) => fleet.tick(t));

  if (demo) {
    demo.addEventListener("mouseenter", () => {
      // Only freeze during the product hold so mid-spawn never stalls.
      if (demo.dataset.phase === "hold" || demo.dataset.phase === "lease") {
        Conductor.pause();
        demo.classList.add("is-paused");
      }
    });
    demo.addEventListener("mouseleave", () => {
      Conductor.resume();
      demo.classList.remove("is-paused");
    });
  }

  Conductor.start();
}

boot();
