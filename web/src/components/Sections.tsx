import { CheckIcon, CopyIcon } from "lucide-react";

import { Fleet } from "@/components/Fleet";
import { Button } from "@/components/ui/button";
import { useCopy } from "@/hooks/use-copy";
import type { Conductor } from "@/lib/conductor";
import { INSTALL_CMD } from "@/lib/copy";
import { cn } from "@/lib/utils";

type Props = {
  reduceMotion: boolean;
  conductor: Conductor;
};

export function Sections({ reduceMotion, conductor }: Props) {
  const { copied, copy } = useCopy();

  return (
    <main>
      <section className="section verbs">
        <p className="section-label">Lifecycle</p>
        <h2>
          <code>up</code>, <code>verify</code>, <code>down</code>. Healthy in,
          gone out.
        </h2>
        <p>
          A lease is a TTL on the instance. <code>verify</code> renews it.
          Expiry reaps what you forget.
        </p>
        <ol className="verb-list">
          <li>
            <code>up</code>
            <p>
              Create or resume. Exit 0 only when every service is healthy.{" "}
              <code>--on</code> picks where it runs and sticks for the life of
              the instance.
            </p>
          </li>
          <li>
            <code>verify</code>
            <p>Run the checks in the toml. Renews the lease.</p>
          </li>
          <li>
            <code>down</code>
            <p>
              Tear the graph down and confirm it is gone. Expired leases clean
              up the rest.
            </p>
          </li>
        </ol>
      </section>

      <section className="section install" id="install">
        <p className="section-label">Install</p>
        <h2>Install once. Hand the rest to an agent.</h2>
        <p>
          Point the agent at the{" "}
          <a
            href="https://github.com/snowmead/stackless/blob/main/.cursor/skills/stackless/SKILL.md"
            rel="noopener noreferrer"
          >
            skill
          </a>{" "}
          and{" "}
          <a
            href="https://github.com/snowmead/stackless/blob/main/docs/SCHEMA.md"
            rel="noopener noreferrer"
          >
            SCHEMA
          </a>
          . The CLI does the loop.
        </p>
        <div className="code-with-copy">
          <pre
            className="code-block install-cmd"
            id="install-cmd"
            tabIndex={0}
          >
            <code>{INSTALL_CMD}</code>
          </pre>
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            className={cn(
              "copy-icon absolute top-[0.65rem] right-[0.65rem] rounded-sm",
              copied &&
                "is-copied text-[var(--signal)] border-[var(--signal-dim)]",
            )}
            aria-label={copied ? "Copied" : "Copy install command"}
            onClick={() => void copy(INSTALL_CMD)}
          >
            {copied ? (
              <CheckIcon data-icon="inline-start" />
            ) : (
              <CopyIcon data-icon="inline-start" />
            )}
            <span className="visually-hidden">
              {copied ? "Copied" : "Copy"}
            </span>
          </Button>
        </div>
        <pre className="code-block" tabIndex={0}>
          <code>{`stackless up --name demo --on local --json
stackless verify demo --json
stackless down demo --json`}</code>
        </pre>
      </section>

      <section className="section agents">
        <p className="section-label">For agents</p>
        <h2>Stdout is the envelope. Always <code>--json</code>.</h2>
        <p>
          Branch on <code>error.code</code> only. Stderr is NDJSON progress
          during <code>up</code>. On failure: remediation, then{" "}
          <code>stackless logs &lt;name&gt; [service]</code> (survives{" "}
          <code>down</code>).
        </p>
        <pre className="code-block" tabIndex={0}>
          <code>{`{
  "schema_version": 1,
  "ok": true,
  "instance": "demo",
  "substrate": "local",
  "origins": [
    { "service": "web", "origin": "http://demo.localhost:4444/" }
  ]
}`}</code>
        </pre>
        <pre className="code-block" tabIndex={0}>
          <code>{`{
  "ok": false,
  "error": {
    "schema_version": 1,
    "code": "local.health_failed",
    "message": "web failed its health contract",
    "instance": "demo",
    "remediation": "fix the health contract; re-run up",
    "context": {
      "service": "web",
      "log_hint": "stackless logs demo web"
    }
  }
}`}</code>
        </pre>
      </section>

      <section className="section fleets" id="fleets">
        <p className="section-label">Parallel agents</p>
        <h2>Many named instances. Same definition.</h2>
        <p>
          Each agent gets its own name so graphs do not collide. Prefer a
          worktree per agent.
        </p>
        <Fleet reduceMotion={reduceMotion} conductor={conductor} />
      </section>

      <section className="section why">
        <p className="section-label">Why</p>
        <h2>Agents need a stack contract, not another CLI.</h2>
        <p>
          Containers and IaC create resources. They do not name an instance,
          wait for health, hold a lease, or prove teardown. That full loop is
          what <code>stackless.toml</code> defines.
        </p>
      </section>
    </main>
  );
}
