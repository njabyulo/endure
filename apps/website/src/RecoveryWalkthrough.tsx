import { useEffect, useState } from "react";
import { Icon } from "./Icons";

const steps = [
  {
    name: "Run",
    title: "A request with a recorded intent.",
    description:
      "The agent calls fetch. Before sending the GET, Ondure commits its intent to the local journal.",
    logs: [
      "agent.ts → fetch(endpoint)",
      "journal → intent committed",
      "host → GET dispatched",
    ],
    journal: "Intent recorded",
    worker: "Running",
    result: "Awaiting response",
  },
  {
    name: "Record",
    title: "Commit the outcome. Then continue.",
    description:
      "Ondure records the response before resolving the promise. JavaScript continues with a result that is already committed.",
    logs: [
      "host → response received",
      "journal → result committed",
      "agent.ts ← response delivered",
    ],
    journal: "Result committed",
    worker: "Running",
    result: '{ "hit": 1 }',
  },
  {
    name: "Interrupt",
    title: "Execution stops. History stays.",
    description:
      "This example interrupts the worker after its GET result commits. Process memory is lost; the committed result remains in the journal.",
    logs: [
      "worker → interrupted",
      "V8 heap → lost",
      "journal → committed result retained",
    ],
    journal: "Result committed",
    worker: "Offline",
    result: '{ "hit": 1 }',
  },
  {
    name: "Recover",
    title: "A fresh isolate. The recorded result.",
    description:
      "On recovery, Ondure verifies the captured program and replays execution. This GET receives its committed result without another network request.",
    logs: [
      "host → captured code verified",
      "V8 → fresh isolate created",
      "fetch(endpoint) ← recorded result",
    ],
    journal: "Reading history",
    worker: "Replaying",
    result: '{ "hit": 1 }',
  },
  {
    name: "Continue",
    title: "Reach the next operation.",
    description:
      "The unfinished run continues beyond its recorded history. If an earlier GET had no committed result, its recovery policy could permit a retry.",
    logs: [
      "journal → recorded operation consumed",
      "agent.ts → continuation reached",
      "host → ready for the next operation",
    ],
    journal: "History replayed",
    worker: "Running",
    result: '{ "hit": 1 }',
  },
];

export function RecoveryWalkthrough() {
  const [active, setActive] = useState(0);
  const [playing, setPlaying] = useState(false);
  useEffect(() => {
    if (!playing) return;
    const timer = setTimeout(() => {
      if (active === steps.length - 1) setPlaying(false);
      else setActive(active + 1);
    }, 1600);
    return () => clearTimeout(timer);
  }, [active, playing]);
  const step = steps[active];
  function select(index: number) {
    setPlaying(false);
    setActive(index);
  }
  return (
    <section
      className="recovery story-panel content-inset"
      id="recovery"
      aria-labelledby="recovery-title"
    >
      <div className="section-heading">
        <div>
          <span className="eyebrow">01 / A THREAD TO PICK BACK UP</span>
          <h2 id="recovery-title">
            Good work deserves
            <br />
            <em>a way to continue.</em>
          </h2>
        </div>
        <span className="simulation-label">
          INTERACTIVE · BROWSER SIMULATION
        </span>
      </div>
      <ol className="recovery-steps" aria-label="Recovery steps">
        {steps.map((item, index) => (
          <li key={item.name}>
            <button
              aria-label={`${String(index + 1).padStart(2, "0")} ${item.name}`}
              aria-current={active === index ? "step" : undefined}
              onClick={() => select(index)}
            >
              <span>{String(index + 1).padStart(2, "0")}</span>
              {item.name}
            </button>
          </li>
        ))}
      </ol>
      <div className={`recovery-stage ${active === 2 ? "is-interrupted" : ""}`}>
        <div className="recovery-explanation">
          <span className="eyebrow">
            {String(active + 1).padStart(2, "0")} / {step.name.toUpperCase()}
          </span>
          <h3>{step.title}</h3>
          <p>{step.description}</p>
          <div className="walkthrough-actions">
            <button
              className="button button-primary"
              disabled={playing}
              onClick={() => {
                setActive(0);
                setPlaying(true);
              }}
            >
              <Icon name="play" size={12} />
              {playing ? "Playing walkthrough…" : "Simulate interruption"}
            </button>
            <button
              className="walkthrough-reset"
              onClick={() => select(0)}
              aria-label="Reset recovery walkthrough"
            >
              <Icon name="reset" size={16} />
            </button>
          </div>
          <p className="simulation-note">
            Browser simulation of a committed GET and process interruption.
            Select a step or play the sequence; no runtime or network request
            runs here.
          </p>
        </div>
        <div className="recovery-instrument">
          <div
            className="execution-lanes"
            aria-label="Execution and persistent history"
          >
            <div>
              <span>V8 WORKER</span>
              <strong>
                <i className="status-dot" />
                {step.worker}
              </strong>
              <small>
                {active === 2
                  ? "Process memory lost"
                  : active >= 3
                    ? "Fresh execution context"
                    : "Live execution context"}
              </small>
            </div>
            <div className="journal-lane">
              <span>SQLITE JOURNAL</span>
              <strong>{step.journal}</strong>
              <code>{step.result}</code>
            </div>
          </div>
          <div className="recovery-terminal">
            <div className="code-heading">
              <span>recovery.log</span>
              <span>ILLUSTRATIVE OUTPUT</span>
            </div>
            <pre aria-label="Simulated recovery log">
              <code>{step.logs.join("\n")}</code>
            </pre>
          </div>
        </div>
      </div>
      <p className="sr-only" role="status">
        Step {active + 1}: {step.name}. {step.title}
      </p>
    </section>
  );
}
