import { Icon } from "./Icons";

export function MemoryStory() {
  return (
    <section
      className="memory-story story-panel content-inset"
      aria-labelledby="memory-title"
    >
      <div className="memory-copy">
        <span className="eyebrow">02 / ROOM TO REMEMBER</span>
        <h2 id="memory-title">
          A little context.
          <br />
          <em>A lot of possibility.</em>
        </h2>
        <p>
          Keep the conversation close, useful facts within reach, and shared
          state in order. Give every next step something to build on.
        </p>
        <a className="button button-light" href="#feature-2">
          Explore agent memory <Icon name="arrow" size={14} />
        </a>
        <small>
          SQLite full-text retrieval. Scoped access.
          <br />
          Recorded outcomes for consistent replay.
        </small>
      </div>
      <div
        className="memory-desk"
        role="img"
        aria-label="Illustration of context, memory and shared state"
      >
        <article className="memory-paper context-paper">
          <span className="eyebrow">◷ CONVERSATION CONTEXT</span>
          <h3>Where we left off.</h3>
          <p className="paper-message">
            “Let’s make the welcome
            <br />a little more personal.”
          </p>
          <div className="paper-rule" />
          <p>
            System messages stay protected.
            <br />
            Older context can be archived.
          </p>
          <span className="paper-tag">A little breathing room</span>
        </article>
        <article className="memory-paper fact-paper">
          <span className="eyebrow">✳ SCOPED RECALL</span>
          <h3>The useful little things.</h3>
          <p>
            <span className="highlight-yellow">Prefers a warm welcome.</span>
            <br />
            Remembered in the onboarding scope.
          </p>
          <div className="memory-search">
            <span>⌕</span> welcome <Icon name="arrow" size={13} />
          </div>
        </article>
        <article className="memory-paper state-paper">
          <span className="eyebrow">◇ SHARED STATE</span>
          <h3>Everyone on the same page.</h3>
          <div className="state-row">
            <span>visits</span>
            <strong>1</strong>
            <small>version 2</small>
          </div>
          <p>Version checks catch conflicting updates.</p>
        </article>
        <span className="desk-star" aria-hidden="true">
          ✳
        </span>
      </div>
    </section>
  );
}
