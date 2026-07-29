export function Lifecycle() {
  return (
    <section className="section verbs" id="lifecycle">
      <p className="section-label">Lifecycle</p>
      <h2>
        <code>up</code>, <code>verify</code>, <code>down</code>. Healthy in,
        gone out.
      </h2>
      <p>
        A lease is a TTL on the instance. <code>verify</code> renews it. Expiry
        reaps what you forget. Death is part of the contract, not a footnote.
      </p>
      <ol className="verb-list">
        <li>
          <code>up</code>
          <p>
            Create or resume. Exit 0 only when every service is healthy.{" "}
            <code>--on</code> picks the substrate and sticks for the life of the
            instance.
          </p>
        </li>
        <li>
          <code>verify</code>
          <p>
            Run the checks in the toml. Renews the lease. Provisioned is not
            verified.
          </p>
        </li>
        <li>
          <code>down</code>
          <p>
            Tear the graph down and confirm it is gone. Expired leases clean up
            the rest.
          </p>
        </li>
        <li>
          <code>status</code> / <code>logs</code>
          <p>
            Staged truth and captured output. Logs survive <code>down</code>.
          </p>
        </li>
      </ol>
    </section>
  );
}
