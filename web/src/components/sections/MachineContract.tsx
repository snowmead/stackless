import { CodeBlock } from "@/components/CodeBlock";

const OK_JSON = `{
  "schema_version": 1,
  "ok": true,
  "instance": "demo",
  "substrate": "local",
  "origins": [
    { "service": "web", "origin": "http://demo.localhost:4444/" }
  ],
  "integrations": {
    "db": { "database_url": "postgres://…" }
  }
}`;

const ERR_JSON = `{
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
}`;

export function MachineContract() {
  return (
    <section className="section agents" id="contract">
      <p className="section-label">Machine contract</p>
      <h2>
        Stdout is the envelope. Always <code>--json</code>.
      </h2>
      <p>
        Branch on <code>error.code</code> only. Stderr is NDJSON progress during{" "}
        <code>up</code>. On failure: remediation, then{" "}
        <code>stackless logs &lt;name&gt; [service]</code>.
      </p>
      <CodeBlock code={OK_JSON} lang="json" />
      <CodeBlock code={ERR_JSON} lang="json" />
    </section>
  );
}
