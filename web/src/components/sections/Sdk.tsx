import { CodeBlock } from "@/components/CodeBlock";

const BIND_SHELL = `stackless bind --file stackless.toml \\
  --idl .stackless/stack.idl.json \\
  --emit typescript=e2e/stack.gen.ts \\
  --emit rust=tests/support/stack_bind.rs \\
  --emit go=internal/stack/origins.go \\
  --emit python=tests/stack_bind.py`;

const CLIENT_TS = `import { Client } from "@stackless/sdk";

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
        Rust embeds a sync <code>Client</code>. TypeScript, Python, and Go shell{" "}
        <code>stackless --json</code>. <code>stackless bind</code> emits typed{" "}
        <code>Origins</code>, <code>Integrations</code>, and{" "}
        <code>VerifyTier</code> for all four languages.
      </p>

      <ul className="sdk-list">
        <li>
          <code>stackless</code> (crate)
          <span>
            <code>Client::system()</code> or hermetic <code>TestContext</code>
          </span>
        </li>
        <li>
          <code>@stackless/sdk</code>
          <span>TypeScript CLI client</span>
        </li>
        <li>
          <code>stackless</code> (Python)
          <span>PyPI-shaped CLI client</span>
        </li>
        <li>
          <code>sdks/go</code>
          <span>Go module CLI client</span>
        </li>
      </ul>

      <CodeBlock code={BIND_SHELL} lang="shell" />
      <CodeBlock code={CLIENT_TS} lang="ts" />
    </section>
  );
}
