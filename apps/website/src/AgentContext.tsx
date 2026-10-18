import { Icon } from "./Icons";

const phases = [
  { icon: "source", title: "Model calls", detail: "A response worth keeping." },
  { icon: "shield", title: "Permissions", detail: "A boundary you choose." },
  {
    icon: "check",
    title: "Human approvals",
    detail: "Time to think it through.",
  },
  {
    icon: "reset",
    title: "Durable recovery",
    detail: "A place to pick back up.",
  },
] as const;

export function AgentContext() {
  return (
    <section
      className="agent-context content-inset"
      aria-labelledby="context-title"
    >
      <span className="eyebrow">FOR THE WORK THAT TAKES A LITTLE LONGER</span>
      <h2 id="context-title">
        An agent’s work outlives
        <br />
        <em>a single process.</em>
      </h2>
      <p>
        Model requests. External actions. A person’s decision.
        <br className="desktop-break" /> Ondure brings them together with a
        persistent record of supported operations.
      </p>
      <div className="context-primitives">
        {phases.map((phase) => (
          <div key={phase.title}>
            <span className="primitive-icon">
              <Icon name={phase.icon} size={24} />
            </span>
            <h3>{phase.title}</h3>
            <p>{phase.detail}</p>
          </div>
        ))}
      </div>
    </section>
  );
}
