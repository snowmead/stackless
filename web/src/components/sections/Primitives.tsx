import { CodeBlock } from "@/components/CodeBlock";

const SERVICES_TOML = `[services.web]
source = { repo = "https://github.com/acme/app", ref = "main" }
health = { http = { path = "/", expect = 200 } }

  [services.web.local]
  run = "python3 -m http.server $PORT --bind 127.0.0.1"

[services.api]
source = { repo = "https://github.com/acme/api", ref = "main" }
health = { http = { path = "/health", expect = 200 } }
env = { CLERK_SECRET = "\${integrations.clerk.secret_key}" }

  [services.api.local]
  run = "npm start -- --port $PORT"`;

const INTEGRATIONS_TOML = `[integrations.clerk]
provider = "clerk"

# Stripe Projects provisions the third party
# into the instance vault. Wire credentials
# into services with \${integrations.<name>.*}.
#
# Clerk is live today. The catalog covers
# dozens of provider families (Neon, Auth0,
# Supabase, …) as the surface expands.`;

export function Primitives() {
  return (
    <section className="section primitives" id="primitives">
      <p className="section-label">Primitives</p>
      <h2>Services you run. Integrations Stripe Projects runs.</h2>
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
          <CodeBlock code={SERVICES_TOML} lang="toml" />
        </div>

        <div className="primitive-pane">
          <p className="primitive-pane-label">
            <code>[integrations.*]</code>
          </p>
          <h3>Hosted third parties</h3>
          <p>
            Provisioned through Stripe Projects into the stack vault. Wire with{" "}
            <code>{("${integrations.<name>.*}")}</code>. Clerk ships today; the
            catalog is the expanding surface.
          </p>
          <CodeBlock code={INTEGRATIONS_TOML} lang="toml" />
        </div>
      </div>
    </section>
  );
}
