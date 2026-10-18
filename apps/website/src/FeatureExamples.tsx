import { useEffect, useRef, useState } from "react";
import { Icon } from "./Icons";

export type FeatureExample = { title: string; detail: string; code: string; path: string; language?: string };

export function FeatureExamples({ examples }: { examples: FeatureExample[] }) {
  return <section className="feature-examples content-inset" id="features" aria-labelledby="features-title">
    <div className="section-heading"><div><span className="eyebrow">04 / SEVEN RUNTIME PRIMITIVES</span><h2 id="features-title">An agent’s work.<br /><em>A runtime’s responsibility.</em></h2></div><p>Each example connects a workflow to a boundary the host can enforce and record.</p></div>
    <div className="feature-example-grid">{examples.map((example, index) => <Example key={example.title} example={example} index={index} />)}</div>
  </section>;
}
function Example({ example, index }: { example: FeatureExample; index: number }) {
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  async function copy() {
    clearTimeout(timer.current);
    try { await navigator.clipboard.writeText(example.code); setCopied(true); setFailed(false); }
    catch { setFailed(true); }
    timer.current = setTimeout(() => { setCopied(false); setFailed(false); }, 3000);
  }
  return <article className="feature-example" aria-labelledby={`feature-${index}`}>
    <span className="eyebrow">{String(index + 1).padStart(2, "0")} / {example.language ?? "JAVASCRIPT"}</span>
    <h3 id={`feature-${index}`}>{example.title}</h3><p>{example.detail}</p>
    <div className="feature-code"><div className="code-heading"><span>RUNNABLE EXAMPLE</span><button onClick={copy} aria-label={`Copy ${example.title} example`}><Icon name={copied ? "check" : "copy"} size={14}/>{copied ? "Copied" : "Copy"}</button></div><pre tabIndex={0} aria-label={`${example.title} code`}><code>{example.code}</code></pre></div>
    <span className="copy-status" role="status">{failed ? "Select the code to copy manually." : copied ? "Example copied." : ""}</span>
    <a className="text-link" href={`https://github.com/njabyulo/endure/blob/feat/agent-runtime-website/${example.path}`} target="_blank" rel="noreferrer">Run this example <Icon name="arrow" size={15}/></a>
  </article>;
}
