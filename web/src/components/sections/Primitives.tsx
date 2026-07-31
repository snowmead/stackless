import { FileIcon } from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ComponentType,
  type CSSProperties,
} from "react";

import {
  LogoAlgolia,
  LogoAuth0,
  LogoClickHouse,
  LogoClerk,
  LogoCloudflare,
  LogoDatadog,
  LogoElevenLabs,
  LogoFly,
  LogoGitLab,
  LogoHuggingFace,
  LogoLaravel,
  LogoMixpanel,
  LogoNeon,
  LogoPlanetScale,
  LogoPostHog,
  LogoPrisma,
  LogoRailway,
  LogoRender,
  LogoSentry,
  LogoSupabase,
  LogoTurso,
  LogoTwilio,
  LogoUpstash,
  LogoWix,
  LogoWordPress,
} from "@/components/logos";
import { highlightCode } from "@/lib/highlight";
import { cn } from "@/lib/utils";

/** Pairs cycle in the toml selection and logo grid. */
const INTEGRATION_PAIRS: readonly [string, string][] = [
  ["clerk", "turso"],
  ["auth0", "neon"],
  ["clerk", "supabase"],
  ["auth0", "planetscale"],
  ["clerk", "upstash"],
  ["auth0", "clickhouse"],
];

const PAIR_HOLD_MS = 2800;
/** Per-character delete/type interval. */
const REWRITE_CHAR_MS = 26;
/** Stagger database tokens slightly after auth. */
const REWRITE_DB_STAGGER_MS = 36;

const REWRITE_TOKEN_IDS = [
  "env-auth",
  "env-db",
  "int-auth-header",
  "int-auth-provider",
  "int-db-header",
  "int-db-provider",
] as const;

type RewriteTokenId = (typeof REWRITE_TOKEN_IDS)[number];

type LogoComponent = ComponentType<{
  className?: string;
  title?: string;
  color?: string;
}>;

/** Simple Icons brand hex (Twilio removed from SI; official #F22F46). */
const INTEGRATION_PROVIDERS: {
  id: string;
  label: string;
  Logo: LogoComponent;
  color: string;
  /** Near-black brand marks: lighten in `.dark` so they stay visible. */
  ink?: boolean;
}[] = [
  { id: "clerk", label: "clerk", Logo: LogoClerk, color: "#6C47FF" },
  { id: "auth0", label: "auth0", Logo: LogoAuth0, color: "#EB5424" },
  { id: "neon", label: "neon", Logo: LogoNeon, color: "#34D59A" },
  { id: "supabase", label: "supabase", Logo: LogoSupabase, color: "#3FCF8E" },
  {
    id: "planetscale",
    label: "planetscale",
    Logo: LogoPlanetScale,
    color: "#000000",
    ink: true,
  },
  { id: "turso", label: "turso", Logo: LogoTurso, color: "#4FF8D2" },
  {
    id: "prisma",
    label: "prisma",
    Logo: LogoPrisma,
    color: "#2D3748",
    ink: true,
  },
  { id: "clickhouse", label: "clickhouse", Logo: LogoClickHouse, color: "#FFCC01" },
  { id: "upstash", label: "upstash", Logo: LogoUpstash, color: "#00E9A3" },
  {
    id: "railway",
    label: "railway",
    Logo: LogoRailway,
    color: "#0B0D0E",
    ink: true,
  },
  {
    id: "render",
    label: "render",
    Logo: LogoRender,
    color: "#000000",
    ink: true,
  },
  {
    id: "flyio",
    label: "fly.io",
    Logo: LogoFly,
    color: "#24175B",
    ink: true,
  },
  { id: "cloudflare", label: "cloudflare", Logo: LogoCloudflare, color: "#F38020" },
  { id: "gitlab", label: "gitlab", Logo: LogoGitLab, color: "#FC6D26" },
  { id: "laravel", label: "laravel", Logo: LogoLaravel, color: "#FF2D20" },
  { id: "wordpress", label: "wordpress", Logo: LogoWordPress, color: "#21759B" },
  { id: "wix", label: "wix", Logo: LogoWix, color: "#0C6EFC" },
  {
    id: "sentry",
    label: "sentry",
    Logo: LogoSentry,
    color: "#362D59",
    ink: true,
  },
  { id: "datadog", label: "datadog", Logo: LogoDatadog, color: "#632CA6" },
  {
    id: "posthog",
    label: "posthog",
    Logo: LogoPostHog,
    color: "#000000",
    ink: true,
  },
  { id: "mixpanel", label: "mixpanel", Logo: LogoMixpanel, color: "#7856FF" },
  { id: "algolia", label: "algolia", Logo: LogoAlgolia, color: "#003DFF" },
  { id: "twilio", label: "twilio", Logo: LogoTwilio, color: "#F22F46" },
  {
    id: "huggingface",
    label: "huggingface",
    Logo: LogoHuggingFace,
    color: "#FFD21E",
  },
  {
    id: "elevenlabs",
    label: "elevenlabs",
    Logo: LogoElevenLabs,
    color: "#000000",
    ink: true,
  },
];

function servicesTomlHead(): string {
  return `[services.web]
source = { repo = "https://github.com/acme/app", ref = "main" }
health = { http = { path = "/", expect = 200 } }

  [services.web.local]
  run = "python3 -m http.server $PORT --bind 127.0.0.1"

[services.api]
source = { repo = "https://github.com/acme/api", ref = "main" }
health = { http = { path = "/health", expect = 200 } }`;
}

function servicesTomlTail(): string {
  return `
  [services.api.local]
  run = "npm start -- --port $PORT"`;
}

function RewriteToken({
  to,
  className,
  reduceMotion,
  charMs = REWRITE_CHAR_MS,
  delayMs = 0,
  cycleGeneration,
  onSettled,
}: {
  to: string;
  className?: string;
  reduceMotion: boolean;
  charMs?: number;
  delayMs?: number;
  /** Non-zero when a pair cycle is active (skips settle on initial mount). */
  cycleGeneration: number;
  onSettled?: () => void;
}) {
  const [display, setDisplay] = useState(to);
  const displayRef = useRef(to);
  const timersRef = useRef<number[]>([]);
  const onSettledRef = useRef(onSettled);
  onSettledRef.current = onSettled;

  const clearTimers = useCallback(() => {
    for (const id of timersRef.current) {
      window.clearTimeout(id);
    }
    timersRef.current = [];
  }, []);

  useEffect(() => {
    if (cycleGeneration === 0) {
      return;
    }

    if (to === displayRef.current) {
      onSettledRef.current?.();
      return;
    }

    clearTimers();
    let cancelled = false;

    const finish = () => {
      if (cancelled) return;
      displayRef.current = to;
      setDisplay(to);
      onSettledRef.current?.();
    };

    const schedule = (fn: () => void, ms: number) => {
      const id = window.setTimeout(fn, ms);
      timersRef.current.push(id);
    };

    const run = () => {
      if (reduceMotion) {
        finish();
        return;
      }

      let current = displayRef.current;
      const target = to;

      const stepDelete = () => {
        if (cancelled) return;
        if (current.length === 0) {
          stepType("");
          return;
        }
        current = current.slice(0, -1);
        displayRef.current = current;
        setDisplay(current);
        schedule(stepDelete, charMs);
      };

      const stepType = (partial: string) => {
        if (cancelled) return;
        if (partial.length >= target.length) {
          finish();
          return;
        }
        const next = target.slice(0, partial.length + 1);
        displayRef.current = next;
        setDisplay(next);
        schedule(() => stepType(next), charMs);
      };

      stepDelete();
    };

    if (delayMs > 0) {
      schedule(run, delayMs);
    } else {
      run();
    }

    return () => {
      cancelled = true;
      clearTimers();
    };
  }, [to, cycleGeneration, reduceMotion, charMs, delayMs, clearTimers]);

  return <span className={className}>{display}</span>;
}

function ProviderLogoGrid({ selected }: { selected: readonly string[] }) {
  const selectedSet = new Set(selected);

  return (
    <div className="primitive-provider-shell">
      <ul
        className={cn("primitive-provider-grid", "is-filtering")}
        role="list"
        aria-label="Supported providers"
      >
        {INTEGRATION_PROVIDERS.map(({ id, label, Logo, color, ink }) => {
          const isSelected = selectedSet.has(id);
          return (
            <li key={id} aria-label={label} aria-current={isSelected || undefined}>
              <span
                className={cn(
                  "primitive-provider-cell",
                  isSelected && "is-selected",
                )}
                style={{ "--provider-tint": color } as CSSProperties}
              >
                <Logo
                  className={
                    ink
                      ? "primitive-provider-logo primitive-provider-logo--ink"
                      : "primitive-provider-logo"
                  }
                  title=""
                  color={color}
                />
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function PrimitiveToml({
  auth,
  db,
  reduceMotion,
  cycleGeneration,
  onTokenSettled,
}: {
  auth: string;
  db: string;
  reduceMotion: boolean;
  cycleGeneration: number;
  onTokenSettled?: (id: RewriteTokenId) => void;
}) {
  const report = (id: RewriteTokenId) =>
    onTokenSettled ? () => onTokenSettled(id) : undefined;

  return (
    <div className="hero-code code-panel">
      <div className="hero-code-chrome">
        <div className="hero-code-file">
          <FileIcon className="hero-code-icon" aria-hidden="true" />
          <span>stackless.toml</span>
        </div>
      </div>
      <pre
        className="code-block code-block-hl hero-code-body"
        tabIndex={0}
        data-lang="toml"
      >
        <code>
          {highlightCode(servicesTomlHead(), "toml")}
          <span className="toml-line">
            <span className="t-key">env</span>
            <span className="t-punct"> = </span>
            <span className="t-punct">{"{"}</span>
            <span className="t-key"> AUTH_SECRET </span>
            <span className="t-punct">= </span>
            <span className="t-str">{"\"${integrations."}</span>
            <RewriteToken
              className="t-interp"
              to={auth}
              reduceMotion={reduceMotion}
              cycleGeneration={cycleGeneration}
              onSettled={report("env-auth")}
            />
            <span className="t-interp">{'.secret_key}"'}</span>
            <span className="t-punct">, </span>
            <span className="t-key">DATABASE_URL </span>
            <span className="t-punct">= </span>
            <span className="t-str">{"\"${integrations."}</span>
            <RewriteToken
              className="t-interp"
              to={db}
              reduceMotion={reduceMotion}
              cycleGeneration={cycleGeneration}
              delayMs={REWRITE_DB_STAGGER_MS}
              onSettled={report("env-db")}
            />
            <span className="t-interp">{'.database_url}"'}</span>
            <span className="t-punct"> </span>
            <span className="t-punct">{"}"}</span>
          </span>
          {highlightCode(servicesTomlTail(), "toml")}
          <span className="toml-line toml-line-empty" />
          <span
            className="toml-selection"
            aria-label={`Selected integrations: ${auth}, ${db}`}
          >
            <span className="toml-line">
              <span className="t-header">[integrations.</span>
              <RewriteToken
                className="t-header"
                to={auth}
                reduceMotion={reduceMotion}
                cycleGeneration={cycleGeneration}
                onSettled={report("int-auth-header")}
              />
              <span className="t-header">]</span>
            </span>
            <span className="toml-line">
              <span className="t-key">provider</span>
              <span className="t-punct"> = </span>
              <span className="t-str">{"\""}</span>
              <RewriteToken
                className="t-str"
                to={auth}
                reduceMotion={reduceMotion}
                cycleGeneration={cycleGeneration}
                onSettled={report("int-auth-provider")}
              />
              <span className="t-str">{"\""}</span>
            </span>
            <span className="toml-line toml-line-empty" />
            <span className="toml-line">
              <span className="t-header">[integrations.</span>
              <RewriteToken
                className="t-header"
                to={db}
                reduceMotion={reduceMotion}
                cycleGeneration={cycleGeneration}
                delayMs={REWRITE_DB_STAGGER_MS}
                onSettled={report("int-db-header")}
              />
              <span className="t-header">]</span>
            </span>
            <span className="toml-line">
              <span className="t-key">provider</span>
              <span className="t-punct"> = </span>
              <span className="t-str">{"\""}</span>
              <RewriteToken
                className="t-str"
                to={db}
                reduceMotion={reduceMotion}
                cycleGeneration={cycleGeneration}
                delayMs={REWRITE_DB_STAGGER_MS}
                onSettled={report("int-db-provider")}
              />
              <span className="t-str">{"\""}</span>
            </span>
          </span>
        </code>
      </pre>
    </div>
  );
}

type Props = {
  reduceMotion: boolean;
};

export function Primitives({ reduceMotion }: Props) {
  const [pairIndex, setPairIndex] = useState(0);
  const [logoPairIndex, setLogoPairIndex] = useState(0);
  const [cycleGeneration, setCycleGeneration] = useState(0);
  const [auth, db] = INTEGRATION_PAIRS[pairIndex] ?? INTEGRATION_PAIRS[0];
  const [logoAuth, logoDb] =
    INTEGRATION_PAIRS[logoPairIndex] ?? INTEGRATION_PAIRS[0];

  const holdTimerRef = useRef(0);
  const settledRef = useRef(new Set<RewriteTokenId>());
  const pairIndexRef = useRef(pairIndex);
  pairIndexRef.current = pairIndex;

  const scheduleHold = useCallback(() => {
    window.clearTimeout(holdTimerRef.current);
    holdTimerRef.current = window.setTimeout(() => {
      // Clear before bumping generation so child effects that settle
      // synchronously in this cycle are not wiped by a later effect.
      settledRef.current.clear();
      setCycleGeneration((g) => g + 1);
      setPairIndex((i) => (i + 1) % INTEGRATION_PAIRS.length);
    }, PAIR_HOLD_MS);
  }, []);

  const handleTokenSettled = useCallback(
    (id: RewriteTokenId) => {
      settledRef.current.add(id);
      if (settledRef.current.size >= REWRITE_TOKEN_IDS.length) {
        settledRef.current.clear();
        setLogoPairIndex(pairIndexRef.current);
        scheduleHold();
      }
    },
    [scheduleHold],
  );

  useEffect(() => {
    if (reduceMotion) {
      const id = window.setInterval(() => {
        setPairIndex((i) => {
          const next = (i + 1) % INTEGRATION_PAIRS.length;
          setLogoPairIndex(next);
          return next;
        });
      }, PAIR_HOLD_MS);
      return () => window.clearInterval(id);
    }

    scheduleHold();
    return () => window.clearTimeout(holdTimerRef.current);
  }, [reduceMotion, scheduleHold]);

  return (
    <section className="section primitives" id="primitives">
      <p className="section-label">Primitives</p>
      <h2>Services & Integrations</h2>
      <p>
        Containers and IaC create resources. They do not name an instance, wait
        for health, hold a lease, or prove teardown.{" "}
        <code>stackless.toml</code> is that full loop, split into two tables.
      </p>

      <div className="primitive-split">
        <div className="primitive-pane">
          <p className="primitive-pane-label">
            <code>[services.*]</code>
          </p>
          <h3>Anything you spin up</h3>
          <p>
            Your app processes: web, API, workers, fixtures. Each needs{" "}
            <code>source</code>, <code>health</code>, and a substrate{" "}
            <code>run</code> (or cloud deploy) block. Not Stripe Projects; your
            code.
          </p>
          <PrimitiveToml
            auth={auth}
            db={db}
            reduceMotion={reduceMotion}
            cycleGeneration={cycleGeneration}
            onTokenSettled={reduceMotion ? undefined : handleTokenSettled}
          />
        </div>

        <div className="primitive-pane">
          <p className="primitive-pane-label">
            <code>[integrations.*]</code>
          </p>
          <h3>Hosted third parties</h3>
          <p>
            Provisioned through Stripe Projects into the stack vault. Wire with{" "}
            <code>{("${integrations.<name>.*}")}</code>. Every provider in the
            catalog registry is first-class — same lifecycle, same wiring.
          </p>
          <ProviderLogoGrid selected={[logoAuth, logoDb]} />
        </div>
      </div>
    </section>
  );
}
