export function RuntimeArchitecture() {
  return (
    <section
      className="architecture content-inset"
      id="runtime"
      aria-labelledby="runtime-title"
    >
      <div className="architecture-copy">
        <span className="eyebrow">02 / HOW IT WORKS</span>
        <h2 id="runtime-title">
          Your code runs in V8.<br />
          <em>Its authority lives in Rust.</em>
        </h2>
        <p>
          Agent code uses the APIs exposed by the host. Rust checks network
          permission, records supported operations, and delivers their outcomes
          back to JavaScript.
        </p>
        <dl className="boundary-notes">
          <div>
            <dt>Permission before connection.</dt>
            <dd>Network access is denied by default. A denied fetch throws in the host callback before a socket opens.</dd>
          </div>
          <div>
            <dt>A record before continuation.</dt>
            <dd>For a new durable GET, intent commits before dispatch. Its outcome commits before the promise settles.</dd>
          </div>
          <div>
            <dt>Execution with a budget.</dt>
            <dd>The CLI bounds active execution time and V8-managed heap usage. These are not total process-memory limits or an OS sandbox.</dd>
          </div>
        </dl>
      </div>
      <figure className="runtime-diagram">
        <div className="diagram-node agent-node">
          <span>AGENT CODE</span>
          <strong>JavaScript / TypeScript</strong>
        </div>
        <div className="diagram-connector" aria-hidden="true">↓</div>
        <div className="diagram-node">
          <span>EXECUTION</span>
          <strong>V8 isolate</strong>
          <code>fetch(endpoint)</code>
        </div>
        <div className="diagram-connector">
          <span>host call</span><span aria-hidden="true">↓</span>
        </div>
        <div className="host-boundary">
          <div className="diagram-node">
            <span>RUST HOST BOUNDARY</span>
            <strong>Check network permission</strong>
          </div>
          <div className="permission-paths">
            <div className="denied-path">
              <span>DENIED</span><strong>Throw permission error</strong>
              <small>No socket opened</small>
            </div>
            <div className="allowed-path">
              <span>ALLOWED · NEW GET</span>
              <ol>
                <li>Commit intent</li>
                <li>Dispatch request</li>
                <li>Commit outcome</li>
                <li>Settle promise</li>
              </ol>
            </div>
          </div>
          <div className="journal-footnote">SQLite WAL · recorded operation history</div>
        </div>
        <figcaption>
          During replay, a completed recorded GET supplies its outcome from
          history. No new request is sent for that operation.
        </figcaption>
      </figure>
    </section>
  );
}
