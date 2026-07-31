import { CodePanel } from "@/components/CodePanel";
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
import type { ComponentType, CSSProperties } from "react";

const SERVICES_TOML = `[services.web]
source = { repo = "https://github.com/acme/app", ref = "main" }
health = { http = { path = "/", expect = 200 } }

  [services.web.local]
  run = "python3 -m http.server $PORT --bind 127.0.0.1"

[services.api]
source = { repo = "https://github.com/acme/api", ref = "main" }
health = { http = { path = "/health", expect = 200 } }
env = { DATABASE_URL = "\${integrations.db.database_url}" }

  [services.api.local]
  run = "npm start -- --port $PORT"`;

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

function ProviderLogoGrid() {
  return (
    <div className="primitive-provider-shell">
      <ul
        className="primitive-provider-grid"
        role="list"
        aria-label="Supported providers"
      >
        {INTEGRATION_PROVIDERS.map(({ id, label, Logo, color, ink }) => (
          <li key={id} aria-label={label}>
            <span
              className="primitive-provider-cell"
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
        ))}
      </ul>
    </div>
  );
}

export function Primitives() {
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
          <CodePanel
            code={SERVICES_TOML}
            lang="toml"
            label="stackless.toml"
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
          <ProviderLogoGrid />
        </div>
      </div>
    </section>
  );
}
