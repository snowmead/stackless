export function Lifecycle() {
  return (
    <section className="section verbs" id="lifecycle">
      <p className="section-label">Lifecycle</p>
      <h2>CLI</h2>
      <p>
        A lease is a TTL on the instance. <code>verify</code> renews it. Expiry
        reaps what you forget. Death is part of the contract, not a footnote.
      </p>

      <div className="lifecycle-machine">
        <ol className="lifecycle-timeline" aria-label="Core lifecycle loop">
          <li className="lifecycle-step" data-verb="up">
            <code className="lifecycle-chip">stackless up</code>
            <p>
              Create or resume. Exit 0 only when every service is healthy.{" "}
              <code>--on</code> picks the substrate and sticks for the life of
              the instance.
            </p>
          </li>

          <li className="lifecycle-step" data-verb="verify">
            <code className="lifecycle-chip">stackless verify</code>
            <p>
              Run the checks in the toml. Renews the lease. Provisioned is not
              verified.
            </p>
          </li>

          <li className="lifecycle-step" data-verb="down">
            <code className="lifecycle-chip">stackless down</code>
            <p>
              Tear the graph down and confirm it is gone. Expired leases clean
              up the rest.
            </p>
          </li>
        </ol>

        <div className="lifecycle-rail">
          <p className="lifecycle-rail-label">Observe</p>
          <ul className="lifecycle-observe" aria-label="Status and logs">
            <li>
              <code className="lifecycle-chip">stackless status</code>
              <p>
                Staged truth for one instance. <code>list</code> shows every
                name still under lease.
              </p>
            </li>
            <li>
              <code className="lifecycle-chip">stackless logs</code>
              <p>
                Captured output per service. Survives <code>down</code>.
              </p>
            </li>
          </ul>
        </div>
      </div>
    </section>
  );
}
