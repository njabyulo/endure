import { Icon, Mark } from "./Icons";
export function RuntimeArchitecture() {
  return (
    <section
      className="architecture story-panel content-inset"
      id="runtime"
      aria-labelledby="runtime-title"
    >
      <div className="architecture-copy">
        <span className="eyebrow">03 / FREEDOM, WITH BOUNDARIES</span>
        <h2 id="runtime-title">
          Big plans.
          <br />
          <em>Thoughtful permissions.</em>
        </h2>
        <p>
          Your JavaScript and TypeScript run in V8. The Rust host checks
          authority before an external operation leaves the runtime.
        </p>
        <dl className="boundary-notes">
          <div>
            <dt>
              <Icon name="shield" size={16} /> Permission before connection.
            </dt>
            <dd>
              Agent requests and operator grants intersect. Missing permissions
              stay denied.
            </dd>
          </div>
          <div>
            <dt>
              <Icon name="check" size={16} /> A record before continuation.
            </dt>
            <dd>
              Supported operation outcomes commit before they return to
              JavaScript.
            </dd>
          </div>
          <div>
            <dt>
              <Icon name="bolt" size={16} /> Resources with a boundary.
            </dt>
            <dd>
              Configure token, spend, concurrency and deadline limits. Active
              execution and V8 heap limits remain separate.
            </dd>
          </div>
        </dl>
        <a className="button button-light" href="#feature-1">
          Explore capability permissions <Icon name="arrow" size={14} />
        </a>
      </div>
      <figure className="runtime-diagram">
        <div className="agent-node diagram-node">
          <Mark />
          <div>
            <span>YOUR AGENT</span>
            <strong>A world of ideas.</strong>
            <code>ctx.tools.call(…)</code>
          </div>
        </div>
        <div className="diagram-connector" aria-hidden="true">
          ↓
        </div>
        <div className="host-boundary">
          <span className="eyebrow">RUST HOST / CHECK AUTHORITY</span>
          <h3>Only the doors you open.</h3>
          <div className="permission-paths">
            <div className="allowed-path">
              <Icon name="check" size={18} />
              <span>ALLOWED</span>
              <strong>
                Record. Dispatch.
                <br />
                Commit. Continue.
              </strong>
              <small>Configured tool + grant</small>
            </div>
            <div className="denied-path">
              <Icon name="shield" size={18} />
              <span>DENIED</span>
              <strong>
                Stop before
                <br />
                external dispatch.
              </strong>
              <small>No matching grant</small>
            </div>
          </div>
        </div>
        <div className="journal-footnote">
          <span className="status-dot" /> SQLite journal · committed operation
          history
        </div>
        <figcaption>
          Host-enforced capabilities govern registered operations. They do not
          provide an OS sandbox.
        </figcaption>
      </figure>
    </section>
  );
}
