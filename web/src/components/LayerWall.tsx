import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ComponentType,
  type CSSProperties,
} from "react";

import {
  LogoClaude,
  LogoClerk,
  LogoCodex,
  LogoFly,
  LogoNeon,
  LogoPi,
  LogoRender,
  LogoStripe,
  LogoSupabase,
  LogoVercel,
} from "@/components/logos";

type Props = {
  reduceMotion: boolean;
};

type LogoComponent = ComponentType<{
  className?: string;
  title?: string;
  color?: string;
}>;

/*
 * Layout: one CSS grid, one row per lane. Agent card, wall port, hub port,
 * and env card all share a row, so every edge is a straight horizontal line
 * by construction. The stackless wall and the Stripe Projects bar are
 * vertical slabs spanning all three lane rows.
 *
 * Motion: one shared 9s clock. SMIL pulses (animateMotion) travel the
 * measured edges while CSS keyframes (offset per lane via --ed) drive card
 * state, progress, and results. Lanes overlap; nothing snaps globally.
 */
const CYCLE_S = 9;

// Fractions of the cycle, kept in sync with the keyframes in demo.css.
const WIN = {
  req: { from: 0, to: 0.1 },
  x: { from: 0.089, to: 0.189 },
  dep: { from: 0.178, to: 0.3 },
} as const;

type Lane = {
  id: string;
  agent: string;
  Logo: LogoComponent;
  /** Brand fill for the agent mark (pi.dev / Claude / OpenAI). */
  logoColor: string;
  spec: string;
  cmd: string;
  env: string;
  host: { label: string; Logo: LogoComponent };
  integrations: { label: string; Logo: LogoComponent }[];
  result: string;
  offset: number;
  row: number;
};

const LANES: Lane[] = [
  {
    id: "a",
    agent: "pi",
    Logo: LogoPi,
    // pi.dev accent (site CSS --accent); mark itself is mono black/white.
    logoColor: "#6A9FCC",
    spec: "checkout.spec.ts",
    cmd: "stackless up --name e2e-checkout",
    env: "e2e-checkout",
    host: { label: "vercel", Logo: LogoVercel },
    integrations: [
      { label: "clerk", Logo: LogoClerk },
      { label: "neon", Logo: LogoNeon },
    ],
    result: "18 passed · 11.2s",
    offset: 0,
    row: 2,
  },
  {
    id: "b",
    agent: "claude",
    Logo: LogoClaude,
    logoColor: "#D97757", // Simple Icons Claude
    spec: "auth.spec.ts",
    cmd: "stackless up --name e2e-auth",
    env: "e2e-auth",
    host: { label: "render", Logo: LogoRender },
    integrations: [
      { label: "supabase", Logo: LogoSupabase },
      { label: "neon", Logo: LogoNeon },
    ],
    result: "24 passed · 9.6s",
    offset: 1.6,
    row: 3,
  },
  {
    id: "c",
    agent: "codex",
    Logo: LogoCodex,
    logoColor: "#10A37F", // OpenAI / ChatGPT green
    spec: "billing.spec.ts",
    cmd: "stackless up --name e2e-billing",
    env: "e2e-billing",
    host: { label: "fly.io", Logo: LogoFly },
    integrations: [
      { label: "stripe", Logo: LogoStripe },
      { label: "neon", Logo: LogoNeon },
    ],
    result: "12 passed · 13.4s",
    offset: 3.2,
    row: 4,
  },
];

type EdgeDef = {
  id: string;
  from: string;
  to: string;
  fromSide: "left" | "right" | "center";
  toSide: "left" | "right" | "center";
  kind: "req" | "x" | "dep";
  offset: number;
};

const EDGES: EdgeDef[] = LANES.flatMap((l) => [
  {
    id: `req-${l.id}`,
    from: `agent-${l.id}`,
    to: `wall-in-${l.id}`,
    fromSide: "right",
    toSide: "center",
    kind: "req",
    offset: l.offset,
  },
  {
    id: `x-${l.id}`,
    from: `wall-out-${l.id}`,
    to: `hub-in-${l.id}`,
    fromSide: "center",
    toSide: "center",
    kind: "x",
    offset: l.offset,
  },
  {
    id: `dep-${l.id}`,
    from: `hub-out-${l.id}`,
    to: `env-${l.id}`,
    fromSide: "center",
    toSide: "left",
    kind: "dep",
    offset: l.offset,
  },
]);

type PulseDef = {
  id: string;
  edge: string;
  begin: number;
  from: number;
  to: number;
};

const PULSES: PulseDef[] = LANES.flatMap((l) => [
  {
    id: `p-req-${l.id}`,
    edge: `req-${l.id}`,
    begin: l.offset,
    from: WIN.req.from,
    to: WIN.req.to,
  },
  {
    id: `p-x-${l.id}`,
    edge: `x-${l.id}`,
    begin: l.offset + 0.8,
    from: WIN.x.from,
    to: WIN.x.to,
  },
  {
    id: `p-dep-${l.id}`,
    edge: `dep-${l.id}`,
    begin: l.offset + 1.6,
    from: WIN.dep.from,
    to: WIN.dep.to,
  },
]);

type Pt = { x: number; y: number };

function anchorPoint(
  el: HTMLElement,
  root: DOMRect,
  side: EdgeDef["fromSide"],
): Pt {
  const r = el.getBoundingClientRect();
  const x0 = r.left - root.left;
  const y0 = r.top - root.top;
  switch (side) {
    case "left":
      return { x: x0, y: y0 + r.height / 2 };
    case "right":
      return { x: x0 + r.width, y: y0 + r.height / 2 };
    case "center":
      return { x: x0 + r.width / 2, y: y0 + r.height / 2 };
  }
}

type DrawnEdge = EdgeDef & { d: string };

const f = (n: number) => n.toFixed(3);

function laneDelay(lane: Lane): CSSProperties {
  return { "--ed": `${lane.offset}s` } as CSSProperties;
}

export function LayerWall({ reduceMotion }: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [edges, setEdges] = useState<DrawnEdge[]>([]);

  const measure = useCallback(() => {
    const root = rootRef.current;
    if (!root) return;
    const rootRect = root.getBoundingClientRect();
    setSize({ w: rootRect.width, h: rootRect.height });

    const next: DrawnEdge[] = [];
    for (const edge of EDGES) {
      const fromEl = root.querySelector<HTMLElement>(
        `[data-anchor="${edge.from}"]`,
      );
      const toEl = root.querySelector<HTMLElement>(
        `[data-anchor="${edge.to}"]`,
      );
      if (!fromEl || !toEl) continue;
      const a = anchorPoint(fromEl, rootRect, edge.fromSide);
      const b = anchorPoint(toEl, rootRect, edge.toSide);
      next.push({ ...edge, d: `M ${a.x} ${a.y} L ${b.x} ${b.y}` });
    }
    setEdges(next);
  }, []);

  useEffect(() => {
    measure();
    const id = requestAnimationFrame(measure);
    const root = rootRef.current;
    if (!root) return () => cancelAnimationFrame(id);

    const ro = new ResizeObserver(() => measure());
    ro.observe(root);
    window.addEventListener("resize", measure);
    return () => {
      cancelAnimationFrame(id);
      ro.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [measure]);

  const drawn = new Map(edges.map((e) => [e.id, e]));

  return (
    <div
      ref={rootRef}
      className="layer-wall"
      data-static={reduceMotion ? "" : undefined}
      aria-label="Animation: three agents each call stackless up in parallel; each request travels a straight line through the stackless wall and the Stripe Projects bar into its own isolated environment, which runs its end-to-end spec and passes."
    >
      <svg
        className="layer-edges"
        width={size.w}
        height={size.h}
        viewBox={`0 0 ${size.w || 1} ${size.h || 1}`}
        aria-hidden="true"
      >
        {edges.map((edge) => (
          <path
            key={edge.id}
            id={`layer-edge-${edge.id}`}
            className={`layer-edge layer-edge-${edge.kind}`}
            d={edge.d}
            style={{ "--ed": `${edge.offset}s` } as CSSProperties}
          />
        ))}
        {!reduceMotion &&
          PULSES.map((p) => {
            if (!drawn.has(p.edge)) return null;
            const from = Math.max(p.from, 0.001);
            return (
              <circle
                key={p.id}
                className="layer-pulse"
                r="3.2"
                opacity="0"
              >
                <animateMotion
                  dur={`${CYCLE_S}s`}
                  begin={`${f(p.begin)}s`}
                  repeatCount="indefinite"
                  calcMode="linear"
                  keyPoints="0;0;1;1"
                  keyTimes={`0;${f(from)};${f(p.to)};1`}
                >
                  <mpath href={`#layer-edge-${p.edge}`} />
                </animateMotion>
                <animate
                  attributeName="opacity"
                  dur={`${CYCLE_S}s`}
                  begin={`${f(p.begin)}s`}
                  repeatCount="indefinite"
                  values="0;0;1;1;0;0"
                  keyTimes={`0;${f(from)};${f(from + 0.02)};${f(p.to - 0.03)};${f(p.to)};1`}
                />
              </circle>
            );
          })}
      </svg>

      <p className="layer-col-label layer-label-agents">E2E fleet</p>
      <p className="layer-col-label layer-label-envs">Isolated instances</p>

      <div className="layer-agents">
        {LANES.map((lane) => (
          <div
            key={lane.id}
            className="layer-agent"
            data-anchor={`agent-${lane.id}`}
            style={laneDelay(lane)}
          >
            <div className="layer-agent-top">
              <span className="layer-agent-dot" />
              <lane.Logo
                className="layer-agent-logo"
                title=""
                color={lane.logoColor}
              />
              <span className="layer-agent-who">{lane.agent}</span>
              <span className="layer-agent-spec">{lane.spec}</span>
            </div>
            <code className="layer-agent-cmd">{lane.cmd}</code>
          </div>
        ))}
      </div>

      <div className="layer-wall-slab" data-anchor="wall">
        <span className="layer-wall-word">stackless</span>
        <span className="layer-wall-sub">lifecycle layer</span>
      </div>

      {LANES.map((lane) => (
        <span
          key={`wall-in-${lane.id}`}
          className="layer-port layer-port-wall layer-port-in"
          data-anchor={`wall-in-${lane.id}`}
          style={
            {
              "--row": lane.row,
              "--ed": `${lane.offset}s`,
            } as CSSProperties
          }
        />
      ))}
      {LANES.map((lane) => (
        <span
          key={`wall-out-${lane.id}`}
          className="layer-port layer-port-wall layer-port-out"
          data-anchor={`wall-out-${lane.id}`}
          style={{ "--row": lane.row } as CSSProperties}
        />
      ))}

      <div className="layer-hub" data-anchor="hub">
        <LogoStripe className="layer-hub-logo" />
        <span className="layer-hub-title">Stripe Projects</span>
        <span className="layer-hub-meta">provisioning plane</span>
      </div>

      {LANES.map((lane) => (
        <span
          key={`hub-in-${lane.id}`}
          className="layer-port layer-port-hub layer-port-in"
          data-anchor={`hub-in-${lane.id}`}
          style={
            {
              "--row": lane.row,
              "--ed": `${lane.offset}s`,
            } as CSSProperties
          }
        />
      ))}
      {LANES.map((lane) => (
        <span
          key={`hub-out-${lane.id}`}
          className="layer-port layer-port-hub layer-port-out"
          data-anchor={`hub-out-${lane.id}`}
          style={{ "--row": lane.row } as CSSProperties}
        />
      ))}

      <ul className="layer-envs">
        {LANES.map((lane) => (
          <li
            key={lane.id}
            className="layer-env"
            data-anchor={`env-${lane.id}`}
          >
            <div className="layer-env-card" style={laneDelay(lane)}>
              <div className="layer-env-top">
                <code className="layer-env-name">{lane.env}</code>
                <span className="layer-env-badge" style={laneDelay(lane)}>
                  <span className="layer-st layer-st-prov">
                    provisioning…
                  </span>
                  <span className="layer-st layer-st-run">
                    running {lane.spec}
                  </span>
                  <span className="layer-st layer-st-pass">
                    ✓ {lane.result}
                  </span>
                </span>
              </div>
              <div className="layer-env-chips">
                <span className="layer-chip layer-chip-host">
                  <lane.host.Logo className="layer-chip-logo" />@
                  {lane.host.label}
                </span>
                {lane.integrations.map((int) => (
                  <span key={int.label} className="layer-chip">
                    <int.Logo className="layer-chip-logo" />
                    {int.label}
                  </span>
                ))}
              </div>
              <div className="layer-env-bar">
                <span className="layer-env-fill" style={laneDelay(lane)} />
              </div>
            </div>
          </li>
        ))}
      </ul>

      <p className="layer-summary">
        <span>
          <strong>3</strong> agents
        </span>
        <span>
          <strong>3</strong> isolated instances
        </span>
        <span>
          <strong>0</strong> shared state
        </span>
      </p>
    </div>
  );
}
