import { useEffect, useRef } from "react";

import {
  applyCues,
  HERO_MS,
  NODE_ORDER,
  type Conductor,
  type Cue,
} from "@/lib/conductor";

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function HeroDemo({ reduceMotion, conductor }: Props) {
  const demoRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const demo = demoRef.current;
    if (!demo) return;

    const cmdUpEl = demo.querySelector<HTMLElement>('[data-cmd="up"]');
    const cmdDownEl = demo.querySelector<HTMLElement>('[data-cmd="down"]');
    const cursorEl = demo.querySelector<HTMLElement>(".demo-cursor");
    const tomlBody = demo.querySelector<HTMLElement>(".demo-toml-body");
    if (!cmdUpEl || !cmdDownEl || !cursorEl) return;
    const cmdUp = cmdUpEl;
    const cmdDown = cmdDownEl;
    const cursor = cursorEl;
    const root = demo;

    const fired = new Set<number>();
    let lastCycle = -1;

    function setPhase(phase: string) {
      root.dataset.phase = phase;
    }

    function clearToml() {
      root.querySelectorAll(".toml-line").forEach((el) => {
        el.classList.remove("is-lit", "was-lit");
      });
    }

    function pinToml(key: string, { keepOthers = false } = {}) {
      if (!keepOthers) {
        root.querySelectorAll(".toml-line.is-lit").forEach((el) => {
          el.classList.remove("is-lit");
          el.classList.add("was-lit");
        });
      }
      root.querySelectorAll(`.toml-line[data-hl="${key}"]`).forEach((el) => {
        el.classList.remove("was-lit");
        el.classList.add("is-lit");
      });
      const anchor = root.querySelector(`[data-toml-anchor="${key}"]`);
      if (anchor && tomlBody) {
        const hero = root.closest(".hero");
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
        root.querySelectorAll(`.toml-line[data-hl="${key}"]`).forEach((el) => {
          el.classList.remove("was-lit");
          el.classList.add("is-lit");
        });
      });
    }

    function setNode(key: string, state: "on" | "out" | "off") {
      const node = root.querySelector(`.node[data-node="${key}"]`);
      if (!node) return;
      node.classList.remove("is-on", "is-out", "is-active");
      if (state === "on") {
        node.classList.add("is-on", "is-active");
        root.querySelectorAll(".node").forEach((el) => {
          if (el !== node) el.classList.remove("is-active");
        });
      }
      if (state === "out") node.classList.add("is-out");
    }

    function clearActive() {
      root.querySelectorAll(".node.is-active").forEach((el) => {
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

    function revealCommand(which: "up" | "down") {
      hideCommands();
      const el = which === "up" ? cmdUp : cmdDown;
      el.hidden = false;
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

    if (reduceMotion) {
      showStaticAlive();
      return;
    }

    const schedule: Cue<null>[] = [
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
      { at: 3280, run: () => setPhase("hold") },
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

    const onEnter = () => {
      if (demo.dataset.phase === "hold" || demo.dataset.phase === "lease") {
        conductor.pause();
        demo.classList.add("is-paused");
      }
    };
    const onLeave = () => {
      conductor.resume();
      demo.classList.remove("is-paused");
    };

    demo.addEventListener("mouseenter", onEnter);
    demo.addEventListener("mouseleave", onLeave);

    const off = conductor.on((t) => {
      const cycle = Math.floor(t / HERO_MS);
      const local = t % HERO_MS;
      if (cycle !== lastCycle) {
        fired.clear();
        lastCycle = cycle;
      }
      applyCues(schedule, local, fired, null);
    });

    return () => {
      demo.removeEventListener("mouseenter", onEnter);
      demo.removeEventListener("mouseleave", onLeave);
      off();
    };
  }, [conductor, reduceMotion]);

  return (
    <div
      ref={demoRef}
      className="demo"
      id="demo"
      data-phase="idle"
      aria-label="Animated demo: stackless.toml defines a stack, up spawns nodes, down tears them down"
    >
      <div className="demo-toml" id="demo-toml">
        <div className="demo-toml-head">
          <span>stackless.toml</span>
        </div>
        <pre className="demo-toml-body" tabIndex={0}>
          <code>
            <span className="toml-line" data-hl="stack">
              <span className="t-header">[stack]</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="stack">
              <span className="t-key">name</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;demo&quot;</span>
            </span>
            {"\n\n"}
            <span className="toml-line" data-hl="clerk" data-toml-anchor="clerk">
              <span className="t-header">[integrations.clerk]</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="clerk">
              <span className="t-key">provider</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;clerk&quot;</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="clerk">
              <span className="t-key">app_name</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">
                &quot;
                <span className="t-interp">${"{"}stack.name{"}"}</span>-
                <span className="t-interp">${"{"}instance.name{"}"}</span>
                &quot;
              </span>
            </span>
            {"\n\n"}
            <span className="toml-line" data-hl="web" data-toml-anchor="web">
              <span className="t-header">[services.web]</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-key">source</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-punct">{"{"}</span>{" "}
              <span className="t-key">repo</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">
                &quot;https://github.com/you/app&quot;
              </span>
              <span className="t-punct">,</span>{" "}
              <span className="t-key">ref</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;main&quot;</span>{" "}
              <span className="t-punct">{"}"}</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-key">root_origin</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-bool">true</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-key">health</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-punct">{"{"}</span>{" "}
              <span className="t-key">path</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;/&quot;</span>
              <span className="t-punct">,</span>{" "}
              <span className="t-key">contains</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;ok&quot;</span>{" "}
              <span className="t-punct">{"}"}</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-key">env</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-punct">{"{"}</span>{" "}
              <span className="t-key">CLERK_SECRET_KEY</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">
                &quot;
                <span className="t-interp">
                  ${"{"}integrations.clerk.secret_key{"}"}
                </span>
                &quot;
              </span>{" "}
              <span className="t-punct">{"}"}</span>
            </span>
            {"\n\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-header">{"  "}[services.web.vercel]</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-key">{"  "}framework</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;vite&quot;</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="web">
              <span className="t-key">{"  "}build</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;npm run build&quot;</span>
            </span>
            {"\n\n"}
            <span className="toml-line" data-hl="db" data-toml-anchor="db">
              <span className="t-header">[services.db]</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="db">
              <span className="t-key">source</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-punct">{"{"}</span>{" "}
              <span className="t-key">repo</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">
                &quot;https://github.com/you/app&quot;
              </span>
              <span className="t-punct">,</span>{" "}
              <span className="t-key">ref</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;main&quot;</span>{" "}
              <span className="t-punct">{"}"}</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="db">
              <span className="t-key">health</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-punct">{"{"}</span>{" "}
              <span className="t-key">path</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;/health&quot;</span>
              <span className="t-punct">,</span>{" "}
              <span className="t-key">contains</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">&quot;ready&quot;</span>{" "}
              <span className="t-punct">{"}"}</span>
            </span>
            {"\n\n"}
            <span className="toml-line" data-hl="db">
              <span className="t-header">{"  "}[services.db.local]</span>
            </span>
            {"\n"}
            <span className="toml-line" data-hl="db">
              <span className="t-key">{"  "}run</span>{" "}
              <span className="t-punct">=</span>{" "}
              <span className="t-str">
                &quot;docker run --rm -p $PORT:5432 postgres:16&quot;
              </span>
            </span>
          </code>
        </pre>
      </div>

      <div className="demo-stage">
        <div className="demo-cmd" id="demo-cmd" aria-live="polite">
          <span className="demo-prompt">$</span>
          <span className="demo-cmd-line">
            <span className="demo-cmd-text" data-cmd="up" hidden>
              <span className="cmd-chunk" style={{ ["--d" as string]: 0 }}>
                stackless
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 1 }}>
                up
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 2 }}>
                --name demo
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 3 }}>
                --on vercel
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 4 }}>
                --json
              </span>
            </span>
            <span className="demo-cmd-text" data-cmd="down" hidden>
              <span className="cmd-chunk" style={{ ["--d" as string]: 0 }}>
                stackless
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 1 }}>
                down
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 2 }}>
                demo
              </span>
              <span className="cmd-chunk" style={{ ["--d" as string]: 3 }}>
                --json
              </span>
            </span>
          </span>
          <span className="demo-cursor" aria-hidden="true" />
        </div>

        <div className="demo-graph" id="demo-graph">
          <div className="demo-hub" aria-hidden="true" />
          <div className="demo-spine" aria-hidden="true">
            <span className="demo-spine-fill" />
          </div>
          <ul className="demo-nodes">
            <li className="node" data-node="clerk" style={{ ["--i" as string]: 0 }}>
              <span className="wire" aria-hidden="true">
                <span className="wire-fill" />
              </span>
              <span className="node-mark" />
              <span className="node-label">clerk</span>
              <span className="node-meta">auth</span>
            </li>
            <li className="node" data-node="web" style={{ ["--i" as string]: 1 }}>
              <span className="wire" aria-hidden="true">
                <span className="wire-fill" />
              </span>
              <span className="node-mark" />
              <span className="node-label">web</span>
              <span className="node-meta">vercel</span>
            </li>
            <li className="node" data-node="db" style={{ ["--i" as string]: 2 }}>
              <span className="wire" aria-hidden="true">
                <span className="wire-fill" />
              </span>
              <span className="node-mark" />
              <span className="node-label">db</span>
              <span className="node-meta">postgres</span>
            </li>
          </ul>
        </div>
      </div>
    </div>
  );
}
