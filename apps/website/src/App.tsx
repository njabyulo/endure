import { useEffect, useRef, useState } from "react";
import { FeatureExamples } from "./FeatureExamples";
import { featureExamples } from "./feature-data";
import { AgentContext } from "./AgentContext";
import { WorkflowIllustration } from "./WorkflowIllustration";
import { MemoryStory } from "./MemoryStory";
import { RecoveryWalkthrough } from "./RecoveryWalkthrough";
import { RuntimeArchitecture } from "./RuntimeArchitecture";
import { Icon, Mark } from "./Icons";
import "./App.css";
import { HighlightedCode } from "./HighlightedCode";
import { INSTALL, APPROVAL } from "./installation-code";

const GITHUB = "https://github.com/njabyulo/endure";

function Header() {
  const [open, setOpen] = useState(false);
  const toggle = useRef<HTMLButtonElement>(null);
  const firstNavigationLink = useRef<HTMLAnchorElement>(null);
  useEffect(() => {
    if (open) firstNavigationLink.current?.focus();
  }, [open]);
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
        <a
          ref={firstNavigationLink}
          href="#runtime"
          onClick={() => setOpen(false)}
        >
          Runtime
        </a>
        <a href="#features" onClick={() => setOpen(false)}>
          Features
        </a>
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
        <HighlightedCode code={INSTALL} />
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
        <div className="sky-cap">
          <Header />
        </div>
        <main id="main" aria-label="Explore Ondure">
          <div className="sky-opening">
            <section className="hero" aria-labelledby="hero-title">
              <span className="eyebrow hero-eyebrow">
                A DURABLE RUNTIME FOR AGENTS
              </span>
              <h1 id="hero-title">
                Big ideas.
                <br />
                Agents that <em>keep going.</em>
              </h1>
              <p className="hero-description">
                A little interruption shouldn’t end a good workflow.
                <br className="desktop-break" /> Run JavaScript and TypeScript
                agents with a history to come back to.
              </p>
              <div className="hero-actions">
                <a className="button button-primary" href="#get-started">
                  Get started <Icon name="arrow" size={15} />
                </a>
                <a className="hero-watch" href="#recovery">
                  <Icon name="play" size={12} /> See how recovery works
                </a>
              </div>
              <WorkflowIllustration />
            </section>
          </div>
          <div className="page-content">
            <AgentContext />
            <RecoveryWalkthrough />
            <section className="technology" aria-label="Technology foundation">
              <p>Thoughtfully built on familiar foundations.</p>
              <div className="technology-logos">
                <span>
                  <b className="tech-symbol rust-symbol">R</b>Rust
                </span>
                <span>
                  <b className="tech-symbol v8-symbol">V8</b>V8
                </span>
                <span>
                  <b className="tech-symbol sqlite-symbol">▱</b>SQLite
                </span>
                <span>
                  <b className="tech-symbol ts-symbol">TS</b>TypeScript
                </span>
              </div>
              <p className="technology-note">
                V8 runs the code. Rust enforces the boundary. SQLite keeps the
                history.
              </p>
            </section>
            <MemoryStory />
            <div className="editorial-pause">
              <span className="little-star" aria-hidden="true">
                ✳
              </span>
              <p>
                The next big step?
                <br />
                <em>Knowing where you left off.</em>
              </p>
              <span>Recorded operations give recovery a place to start.</span>
            </div>
            <RuntimeArchitecture />
            <FeatureExamples examples={featureExamples} />
            <section
              className="guarantees content-inset"
              aria-labelledby="guarantees-title"
            >
              <div className="section-heading centered">
                <span className="eyebrow">A CLEARER PICTURE</span>
                <h2 id="guarantees-title">
                  Know what <em>survives.</em>
                </h2>
                <p>
                  A durable workflow starts with a clear record of what
                  completed
                  <br className="desktop-break" /> and what is still uncertain.
                </p>
              </div>
              <div className="guarantee-grid">
                <article>
                  <span className="number-disc">01</span>
                  <h3>Keep completed results.</h3>
                  <p>
                    A committed model or tool outcome is reused on replay. That
                    recorded operation isn’t sent again.
                  </p>
                </article>
                <article>
                  <span className="number-disc">02</span>
                  <h3>Make retries explicit.</h3>
                  <p>
                    An intent isn’t proof of an external outcome. Unsafe
                    interrupted operations stop for reconciliation; retryable
                    requests may run again and incur costs.
                  </p>
                </article>
                <article>
                  <span className="number-disc">03</span>
                  <h3>Rebuild from history.</h3>
                  <p>
                    Captured code and supported operations replay in a fresh
                    isolate. Recovery uses the journal, not a saved JavaScript
                    heap.
                  </p>
                </article>
              </div>
              <p className="guarantees-footnote">
                External effects require cooperation from the receiver; a local
                journal does not guarantee exactly-once effects. Process-crash
                tests do not establish hardware power-loss behavior. Dollar
                accounting uses configured estimates and prices, not an exact
                provider billing cap.
              </p>
            </section>
            <section
              className="get-started content-inset"
              id="get-started"
              aria-labelledby="start-title"
            >
              <div className="section-heading centered">
                <span className="eyebrow">MAKE YOURSELF AT HOME</span>
                <h2 id="start-title">Your first durable run.</h2>
                <p>
                  Build from source. Try a local fixture.
                  <br />
                  No provider credentials or paid calls required.
                </p>
              </div>
              <div className="getting-started-examples">
                <CopyInstall />
                <div className="approval-example">
                  <div className="code-heading">
                    <span>HUMAN IN THE LOOP</span>
                    <span>JAVASCRIPT</span>
                  </div>
                  <pre tabIndex={0} aria-label="Durable approval example">
                    <HighlightedCode code={APPROVAL} />
                  </pre>
                  <p>
                    An approval can stay pending across process restarts. A
                    trusted local operator answers through the CLI.
                  </p>
                  <a
                    className="text-link"
                    href={`${GITHUB}/blob/feat/agent-runtime-website/examples/agent-lifecycle/README.md`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    Approval and recovery commands{" "}
                    <Icon name="arrow" size={15} />
                  </a>
                </div>
              </div>
            </section>
            <section
              className="closing content-inset"
              aria-labelledby="closing-title"
            >
              <span className="closing-doodle" aria-hidden="true">
                ↗
              </span>
              <span className="eyebrow">GOOD IDEAS DESERVE A CONTINUATION</span>
              <h2 id="closing-title">
                <em>Let’s keep going.</em>
              </h2>
              <p>Give your next agent a history to build on.</p>
              <a
                className="button button-primary"
                href={GITHUB}
                target="_blank"
                rel="noreferrer"
              >
                Explore Ondure on GitHub <Icon name="arrow" size={15} />
              </a>
              <a className="closing-secondary" href="#features">
                Find your first example
              </a>
            </section>
            <aside className="future-note content-inset">
              <span className="eyebrow">MORE TO EXPLORE, LATER</span>
              <p>
                Streaming inference, embeddings, model-generated summaries,
                forks, local inference, OS sandboxing and package registries
                remain future work.
              </p>
            </aside>
          </div>
        </main>
        <footer className="footer">
          <div className="footer-top">
            <a className="wordmark" href="#" aria-label="Ondure home">
              <Mark />
              <span>ondure</span>
            </a>
            <p>Big ideas. A history to come back to.</p>
          </div>
          <div className="footer-links">
            <div>
              <span>EXPLORE</span>
              <a href="#runtime">How it works</a>
              <a href="#recovery">Recovery walkthrough</a>
              <a href="#features">Runnable examples</a>
            </div>
            <div>
              <span>BUILD</span>
              <a
                href={`${GITHUB}/blob/feat/agent-runtime-website/README.md`}
                target="_blank"
                rel="noreferrer"
              >
                Documentation ↗
              </a>
              <a href={GITHUB} target="_blank" rel="noreferrer">
                GitHub ↗
              </a>
              <a
                href={`${GITHUB}/blob/feat/agent-runtime-website/LICENSE`}
                target="_blank"
                rel="noreferrer"
              >
                MIT license ↗
              </a>
            </div>
            <div className="footer-colophon">
              <span>BUILT WITH INTENTION</span>
              <p>
                JavaScript & TypeScript.
                <br />
                V8 + Rust + SQLite.
              </p>
              <p>Source build · feature-series review branch.</p>
            </div>
          </div>
          <div className="footer-bottom">
            <span>© 2026 Ondure</span>
            <span>For the work worth continuing.</span>
            <a href="#">Back to top ↑</a>
          </div>
        </footer>
      </div>
    </>
  );
}
export default App;
