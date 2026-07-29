export function Authoring() {
  return (
    <section className="section authoring" id="authoring">
      <p className="section-label">Authoring</p>
      <h2>Scaffold, adopt, check, then doctor.</h2>
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
        . Humans install once; machines drive the loop.
      </p>
      <ol className="verb-list">
        <li>
          <code>init</code>
          <p>Scaffold a minimal valid definition.</p>
        </li>
        <li>
          <code>adopt</code>
          <p>
            Inspect the repo and draft or merge services into{" "}
            <code>stackless.toml</code>.
          </p>
        </li>
        <li>
          <code>check</code>
          <p>
            Validate definition and derived graph per substrate (
            <code>--on</code>).
          </p>
        </li>
        <li>
          <code>doctor</code>
          <p>Preflight daemon, persistence, env, and cloud keys.</p>
        </li>
      </ol>
    </section>
  );
}
