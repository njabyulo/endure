import { Icon, Mark } from "./Icons";

/** A static illustration, not a live run or a screenshot of a separate product. */
export function WorkflowIllustration() {
  return (
    <figure
      className="workflow-illustration"
      aria-label="Illustration of an agent workflow with recorded progress"
    >
      <div className="paper-cloud cloud-left" aria-hidden="true" />
      <div className="paper-cloud cloud-right" aria-hidden="true" />
      <svg
        className="sky-scribble"
        viewBox="0 0 1100 400"
        fill="none"
        aria-hidden="true"
      >
        <path
          d="M0 160C100 90 105 190 170 170S200 30 270 70M790 170c60 60 130-90 100-90s-45 130 30 115 70-130 180-115"
          stroke="currentColor"
        />
        <path
          d="m50 250 50-45m-30 65 50-45M970 265l45-45m-20 65 45-45"
          stroke="currentColor"
        />
      </svg>
      <div className="paper-sheet paper-lilac" aria-hidden="true" />
      <div className="paper-sheet paper-yellow" aria-hidden="true" />
      <div className="paper-sheet paper-mint" aria-hidden="true" />
      <div className="floating-note approval-note">
        <span className="note-icon">
          <Icon name="check" size={17} />
        </span>
        <div>
          <small>A HUMAN MOMENT</small>
          <strong>Approved. Keep going.</strong>
        </div>
        <span className="note-spark" aria-hidden="true">
          ✧
        </span>
      </div>
      <div className="workflow-board">
        <div className="board-chrome">
          <span className="window-dots">
            <i />
            <i />
            <i />
          </span>
          <span>my-first-agent.ts</span>
          <span className="board-saved">
            <Icon name="check" size={10} /> Saved to history
          </span>
        </div>
        <div className="board-body">
          <aside className="board-sidebar">
            <span>
              <Mark />
              My workspace
            </span>
            <span className="board-nav-selected">↗ Onboarding</span>
            <span>◷ Run history</span>
            <span>◇ Approvals</span>
            <small>ONE STEP AT A TIME</small>
          </aside>
          <div className="board-canvas">
            <div className="board-title">
              <span>WORKFLOW / ONBOARDING</span>
              <span>Illustrative run</span>
            </div>
            <h3>
              A warm welcome,
              <br />
              <em>with a way forward.</em>
            </h3>
            <div className="board-columns">
              <div className="run-card run-model">
                <span>01 / MODEL</span>
                <strong>Make a little plan.</strong>
                <p>
                  Welcome, Reader.
                  <br />
                  Here’s where we begin.
                </p>
                <small>
                  <Icon name="check" size={10} /> Outcome recorded
                </small>
              </div>
              <div className="run-card run-approval">
                <span>02 / APPROVAL</span>
                <strong>Leave room for a person.</strong>
                <p>
                  Ready to continue
                  <br />
                  onboarding?
                </p>
                <small>
                  <Icon name="check" size={10} /> Answer recorded
                </small>
              </div>
              <div className="run-card run-continue">
                <span>03 / CONTINUE</span>
                <strong>Pick up the thread.</strong>
                <p>
                  A fresh process.
                  <br />
                  The same history.
                </p>
                <small>
                  <Icon name="arrow" size={10} /> Next operation
                </small>
              </div>
            </div>
            <div className="board-history">
              <span className="status-dot" /> Committed outcomes are ready for
              replay.<span>SQLite journal ↗</span>
            </div>
          </div>
        </div>
      </div>
      <div className="floating-note history-note">
        <span className="history-lines" aria-hidden="true">
          ≋
        </span>
        <div>
          <small>A PLACE TO COME BACK TO</small>
          <strong>Progress, recorded.</strong>
        </div>
      </div>
      <figcaption>AN ILLUSTRATION OF SUPPORTED WORKFLOW PRIMITIVES</figcaption>
    </figure>
  );
}
