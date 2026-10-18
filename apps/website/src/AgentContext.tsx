const phases = [
  { title: "Gather information", detail: "Read from an external service.", status: "GET SUPPORTED", planned: false },
  { title: "Request a decision", detail: "Ask a model what to do next.", status: "JOURNALED MODEL CALL", planned: false },
  { title: "Wait for approval", detail: "Park until a person answers.", status: "DURABLE WAIT SUPPORTED", planned: false },
  { title: "Take action", detail: "Dispatch an authorized tool.", status: "CONFIGURED MCP TOOLS", planned: false },
];

export function AgentContext() {
  return (
    <section className="agent-context content-inset" aria-labelledby="context-title">
      <div className="section-heading">
        <div>
          <span className="eyebrow">01 / WHY ONDURE EXISTS</span>
          <h2 id="context-title">An agent’s work outlives<br /><em>a single process.</em></h2>
        </div>
        <p>Model requests. External actions. Human approvals. Long waits. One workflow crosses all of them.</p>
      </div>
      <div className="context-copy">
        <p>When execution stops halfway through, the application needs to know what completed, what can retry, and what remains uncertain. A log of what happened is only useful for recovery if execution can use it.</p>
        <p>Ondure puts supported operations behind a Rust host and a persistent journal. The agent runs in V8; its recorded history gives recovery a place to start. Model responses, configured tools and approvals use that same operation boundary.</p>
      </div>
      <figure className="agent-workflow">
        <figcaption><span className="eyebrow">SUPPORTED WORKFLOW PRIMITIVES</span><span>Registered operations and explicit retry policies</span></figcaption>
        <ol>
          {phases.map((phase) => (
            <li key={phase.title} className={phase.planned ? "workflow-planned" : ""}>
              <span className="workflow-status">{phase.status}</span>
              <h3>{phase.title}</h3>
              <p>{phase.detail}</p>
            </li>
          ))}
        </ol>
        <div className="workflow-history">Recorded operation history · permission checks at external dispatch</div>
      </figure>
    </section>
  );
}
