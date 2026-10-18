import { useEffect, useRef, useState } from "react";
import { FeatureExamples } from "./FeatureExamples";
import { featureExamples } from "./feature-data";
import { AgentContext } from "./AgentContext";
import { RecoveryWalkthrough } from "./RecoveryWalkthrough";
import { RuntimeArchitecture } from "./RuntimeArchitecture";
import { Icon, Mark } from "./Icons";
import "./App.css";

const GITHUB = "https://github.com/njabyulo/endure";
const INSTALL =
  "git clone --branch feat/agent-runtime-website https://github.com/njabyulo/endure.git\ncd endure\ncargo build --release --bin ondure\npython3 examples/model-operations/run.py";

function Header() {
  const [open, setOpen] = useState(false);
  const toggle = useRef<HTMLButtonElement>(null);
  return (
    <header
      className="header"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          setOpen(false);
          toggle.current?.focus();
        }
      }}
    >
      <a className="wordmark" href="#" aria-label="Ondure home">
        <Mark />
        <span>ondure</span>
      </a>
      <nav
        className={`navigation ${open ? "is-open" : ""}`}
        id="navigation"
        aria-label="Main navigation"
      >
        <a href="#runtime" onClick={() => setOpen(false)}>
          Runtime
        </a>
        <a href="#features" onClick={() => setOpen(false)}>Features</a>
        <a href="#recovery" onClick={() => setOpen(false)}>
          Recovery
        </a>
        <a
          href={`${GITHUB}/blob/feat/agent-runtime-website/README.md`}
          target="_blank"
          rel="noreferrer"
          onClick={() => setOpen(false)}
        >
          Documentation <span>↗</span>
        </a>
      </nav>
      <a
        className="header-github"
        href={GITHUB}
        target="_blank"
        rel="noreferrer"
      >
        <Icon name="source" size={14} /> View on GitHub <span>↗</span>
      </a>
      <button
        ref={toggle}
        className="menu-toggle"
        aria-label={open ? "Close menu" : "Open menu"}
        aria-controls="navigation"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name={open ? "close" : "menu"} size={22} />
      </button>
    </header>
  );
}
function CopyInstall() {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  async function copy() {
    clearTimeout(timer.current);
    try {
      await navigator.clipboard.writeText(INSTALL);
      setState("copied");
    } catch {
      setState("failed");
    }
    timer.current = setTimeout(() => setState("idle"), 3000);
  }
  return (
    <div className="install-code">
      <div className="code-heading">
        <span>
          <span className="status-dot" />
          TERMINAL
        </span>
        <button onClick={copy} aria-label="Copy build and run commands">
          <Icon name={state === "copied" ? "check" : "copy"} size={14} />
          {state === "copied" ? "Copied" : "Copy"}
        </button>
      </div>
      <pre tabIndex={0} aria-label="Build and run commands">
        <code>{INSTALL}</code>
      </pre>
      <span role="status" className="copy-status">
        {state === "copied"
          ? "Commands copied."
          : state === "failed"
            ? "Copy unavailable. Select the commands above to copy."
            : "Requires Rust and Python 3. First build downloads V8."}
      </span>
    </div>
  );
}
function App() {
  return (
    <>
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <div className="page-shell">
        <Header />
        <main id="main">
          <section className="hero" aria-labelledby="hero-title">
            <div className="eyebrow hero-eyebrow">
              <span className="status-dot" />
              THE DURABLE AI RUNTIME
            </div>
            <h1 id="hero-title">
              Autonomous agents fail. <br />
              Ondure makes them <em>durable.</em>
            </h1>
            <p className="hero-description">
              Ondure is a Rust-built JavaScript and TypeScript runtime
              <br className="desktop-break" /> for long-running agents. It combines V8 execution,
              <br className="desktop-break" /> host-enforced permissions, and a durable operation history.
            </p>
            <div className="hero-actions">
              <a className="button button-primary" href="#get-started">
                Get started <Icon name="arrow" size={15} />
              </a>
              <a className="button button-secondary" href="#recovery">
                <span className="play-circle">
                  <Icon name="play" size={9} />
                </span>
                See recovery in action
              </a>
            </div>
            <div className="hero-proof">
              <span className="isolate-stack" aria-hidden="true">
                <Mark />
                <Mark />
                <Mark />
              </span>
              <span>
                <span className="status-dot" />
                RECORD PROGRESS. CONTROL ACCESS. RECOVER.
              </span>
            </div>
          </section>
          <AgentContext />
          <section className="technology" aria-label="Technology foundation">
            <p>
              V8 runs the code. <span>Rust enforces the boundary. SQLite keeps the history.</span>
            </p>
            <div className="technology-logos">
              <span className="tech-rust">
                <span className="rust-gear">R</span> Rust
              </span>
              <span className="tech-v8">
                <span className="v8-mark">V8</span>
              </span>
              <span className="tech-tokio">
                <span className="tokio-symbol">◌</span> tokio
              </span>
              <span className="tech-sqlite">
                <span className="sqlite-symbol">▱</span> SQLite{" "}
                <small>WAL</small>
              </span>
              <span className="tech-typescript">
                <span className="ts-symbol">TS</span> TypeScript{" "}
                <small>via SWC</small>
              </span>
            </div>
          </section>
          <RuntimeArchitecture />
          <RecoveryWalkthrough />
          <FeatureExamples examples={featureExamples} />
          <section className="direction content-inset" aria-labelledby="direction-title">
            <div className="section-heading"><div><span className="eyebrow">FUTURE / OUTSIDE THIS VERSION</span><h2 id="direction-title">A bounded foundation.<br /><em>Room to grow.</em></h2></div><p>Streaming inference, embeddings, model-generated summaries, execution forks, local inference, OS sandboxing and package registries remain future work.</p></div>
          </section>
          <section
            className="guarantees content-inset"
            aria-labelledby="guarantees-title"
          >
            <div className="section-heading">
              <div>
                <span className="eyebrow">05 / GUARANTEES AND LIMITS</span>
                <h2 id="guarantees-title">Know what <em>survives.</em></h2>
              </div>
              <p>Recovery starts with a clear record of what completed—and what is still uncertain.</p>
            </div>
            <div className="guarantee-grid">
              <div>
                <span className="eyebrow">01 / COMPLETED OPERATIONS</span>
                <h3>Use the result you already have.</h3>
                <p>A completed model or tool operation returns its committed outcome during replay. The runtime does not send that operation again.</p>
              </div>
              <div>
                <span className="eyebrow">02 / INTERRUPTED REQUESTS</span>
                <h3>Make retries explicit.</h3>
                <p>An intent records a request, not proof of its external outcome. Unsafe interrupted operations stop for reconciliation; explicitly retryable requests may run again and incur costs.</p>
              </div>
              <div>
                <span className="eyebrow">03 / EXTERNAL EFFECTS</span>
                <h3>Respect the outside world.</h3>
                <p>A local journal cannot undo an external action or guarantee exactly-once effects. Those guarantees need cooperation from the receiving service.</p>
              </div>
              <div>
                <span className="eyebrow">04 / EXECUTION HISTORY</span>
                <h3>Rebuild from the record.</h3>
                <p>Ondure verifies captured code and replays supported recorded operations in a fresh isolate. Recovery uses history, not a saved JavaScript heap.</p>
              </div>
            </div>
            <p className="guarantees-footnote">Process-crash tests do not establish hardware power-loss behavior. Persistence depends on storage honoring synchronization requests. Dollar reservations use configured estimates and prices, not an exact provider billing cap.</p>
          </section>
          <section
            className="get-started content-inset"
            id="get-started"
            aria-labelledby="start-title"
          >
            <div>
              <span className="eyebrow">
                <span className="status-dot" />
                06 / TRY ONDURE · BUILD FROM SOURCE
              </span>
              <h2 id="start-title">
                Start with a run.
                <br />
                <em>Keep its history.</em>
              </h2>
              <p>
                Build the CLI and try a local model fixture.
                <br />
                No provider credentials or paid calls are required.
              </p>
              <a
                className="text-link"
                href={GITHUB}
                target="_blank"
                rel="noreferrer"
              >
                Explore Ondure on GitHub <Icon name="arrow" size={15} />
              </a>
            </div>
            <div className="getting-started-examples">
              <CopyInstall />
              <div className="approval-example">
                <div className="code-heading"><span>examples/agent-lifecycle/main.ts</span><span>SUPPORTED TODAY</span></div>
                <pre tabIndex={0} aria-label="Durable approval example"><code>{'const decision = await ctx.human.approve({\n  action: "Continue onboarding?",\n  timeout: "1h",\n  onTimeout: "reject",\n});\nconsole.log(decision.approved, decision.note);'}</code></pre>
                <p>A pending approval is persisted. Once other live work finishes, the isolate is released. Answer with the CLI to reconstruct the run and continue.</p>
                <a className="text-link" href={`${GITHUB}/blob/feat/agent-runtime-website/examples/agent-lifecycle/README.md`} target="_blank" rel="noreferrer">Approval and recovery commands <Icon name="arrow" size={15} /></a>
              </div>
            </div>
          </section>
        </main>
        <footer className="footer content-inset">
          <a className="wordmark" href="#" aria-label="Ondure home">
            <Mark />
            <span>ondure</span>
          </a>
          <p>Source build · feature-series review branch.</p>
          <div>
            <a href={`${GITHUB}/blob/feat/agent-runtime-website/README.md`} target="_blank" rel="noreferrer">
              Documentation ↗
            </a>
            <a
              href={`${GITHUB}/blob/main/Cargo.toml`}
              target="_blank"
              rel="noreferrer"
            >
              MIT license ↗
            </a>
          </div>
        </footer>
      </div>
      <div className="outside-footer">
        <span>JAVASCRIPT & TYPESCRIPT. V8 + RUST + SQLITE.</span>
        <span>ONDURE / 2026</span>
      </div>
    </>
  );
}
export default App;
