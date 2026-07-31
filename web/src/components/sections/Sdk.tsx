import { ArrowUpRightIcon } from "lucide-react";

import { CodePanel } from "@/components/CodePanel";

const BIND_SHELL = `stackless bind --file stackless.toml \\
  --idl .stackless/stack.idl.json \\
  --emit typescript=e2e/stack.gen.ts \\
  --emit rust=tests/support/stack_bind.rs \\
  --emit go=internal/stack/origins.go \\
  --emit python=tests/stack_bind.py`;

const CLIENT_TS = `import { Client } from "stackless-sdk";

const client = Client.system();
const up = await client.up({
  kind: "create",
  name: "demo",
  on: "local",
  file: "stackless.toml",
});
console.log(up.origins.web);
await client.verify(up.instance);
await client.down(up.instance);`;

export function Sdk() {
  return (
    <section className="section sdk" id="sdk">
      <p className="section-label">SDKs and bind</p>
      <h2>Program the e2e loop. Typed names, not stringly DNS.</h2>
      <p>
        Published packages for Rust, TypeScript, Python, and Go — same
        lifecycle verbs. <code>stackless bind</code> emits typed{" "}
        <code>Origins</code>, <code>Integrations</code>, and{" "}
        <code>VerifyTier</code> for all four.
      </p>

      <ul className="sdk-list">
        <li>
          <a href="https://crates.io/crates/stackless">
            <code>stackless</code> (crates.io)
            <ArrowUpRightIcon className="sdk-list-icon" aria-hidden="true" />
          </a>
          <span>
            Rust · <code>Client::system()</code> or hermetic{" "}
            <code>TestContext</code>
          </span>
        </li>
        <li>
          <a href="https://www.npmjs.com/package/stackless-sdk">
            <code>stackless-sdk</code> (npm)
            <ArrowUpRightIcon className="sdk-list-icon" aria-hidden="true" />
          </a>
          <span>TypeScript · <code>Client.system()</code></span>
        </li>
        <li>
          <a href="https://pypi.org/project/stackless-sdk/">
            <code>stackless-sdk</code> (PyPI)
            <ArrowUpRightIcon className="sdk-list-icon" aria-hidden="true" />
          </a>
          <span>
            Python · <code>import stackless</code>
          </span>
        </li>
        <li>
          <a href="https://pkg.go.dev/github.com/snowmead/stackless/sdks/go">
            <code>sdks/go</code>
            <ArrowUpRightIcon className="sdk-list-icon" aria-hidden="true" />
          </a>
          <span>Go · <code>stackless.System()</code></span>
        </li>
      </ul>

      <CodePanel
        code={BIND_SHELL}
        lang="shell"
        label="stackless bind"
        icon="terminal"
      />
      <CodePanel code={CLIENT_TS} lang="ts" label="client.ts" />
    </section>
  );
}
