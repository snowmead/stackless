const HOSTS = [
  { name: "local", note: "Daemon, reverse proxy, lease reaper" },
  { name: "render", note: "Cloud services + health" },
  { name: "vercel", note: "Deployments as stack services" },
  { name: "fly", note: "Machines via --on fly" },
  { name: "netlify", note: "Static upload substrate" },
] as const;

const PHASE2 = [
  "railway",
  "cloudflare",
  "gitlab",
  "laravel-cloud",
  "wordpress",
] as const;

export function Substrates() {
  return (
    <section className="section substrates" id="substrates">
      <p className="section-label">Substrates</p>
      <h2>
        Same definition. <code>--on</code> picks where it lives.
      </h2>
      <p>
        Substrate is fixed at create. Resume by name; never re-ask. Local pins
        with <code>--source svc=PATH</code> for edit loops; cloud rejects{" "}
        <code>--source</code>.
      </p>
      <ul className="substrate-list">
        {HOSTS.map((host) => (
          <li key={host.name}>
            <code>{host.name}</code>
            <span>{host.note}</span>
          </li>
        ))}
      </ul>
      <p className="substrate-phase2">
        Phase-2 hosts registered (not yet deploy-ready):{" "}
        {PHASE2.map((name, i) => (
          <span key={name}>
            <code>{name}</code>
            {i < PHASE2.length - 1 ? ", " : "."}
          </span>
        ))}
      </p>
    </section>
  );
}
