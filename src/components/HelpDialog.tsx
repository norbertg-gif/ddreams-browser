import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { helpTopics, type HelpBlock } from "../help";

const normalize = (text: string) => text.normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLocaleLowerCase("sk");
const blockText = (block: HelpBlock): string => {
  if (block.type === "paragraph" || block.type === "note") return block.text;
  if (block.type === "shortcuts") return block.rows.flat().join(" ");
  return block.items.join(" ");
};

export default function HelpDialog({ onClose }: { onClose: () => void }) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(helpTopics[0].id);
  const [previousFocus] = useState(() => document.activeElement instanceof HTMLElement ? document.activeElement : null);
  const topics = useMemo(() => {
    const words = normalize(query.trim()).split(/\s+/).filter(Boolean);
    return helpTopics.filter(topic => {
      const text = normalize([topic.title, ...topic.keywords, ...topic.blocks.map(blockText)].join(" "));
      return words.every(word => text.includes(word));
    });
  }, [query]);
  const active = topics.find(topic => topic.id === selected) ?? topics[0];
  useEffect(() => {
    return () => previousFocus?.focus();
  }, [previousFocus]);

  return createPortal(<div className="modal-backdrop help-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="help-dialog" role="dialog" aria-modal="true" aria-labelledby="help-title" onKeyDown={event => event.stopPropagation()}>
      <header className="help-head"><h2 id="help-title">Pomocník</h2><button type="button" onClick={onClose} aria-label="Zavrieť pomocníka" title="Zavrieť (Esc)">×</button></header>
      <div className="help-body">
        <nav className="help-nav" aria-label="Témy pomocníka">
          <label htmlFor="help-search">Hľadať v pomocníkovi</label>
          <input id="help-search" autoFocus value={query} onChange={event => setQuery(event.target.value)} placeholder="Hľadať tému…" />
          <div className="help-topics">{topics.map(topic => <button type="button" key={topic.id} className={topic.id === active?.id ? "active" : ""} aria-current={topic.id === active?.id ? "page" : undefined} onClick={() => setSelected(topic.id)}>{topic.title}</button>)}</div>
        </nav>
        <article className="help-content" key={active?.id ?? "empty"}>
          {active ? <><h2>{active.title}</h2>{active.blocks.map((block, index) => {
            if (block.type === "paragraph") return <p key={index}>{block.text}</p>;
            if (block.type === "note") return <p key={index} className="help-note">{block.text}</p>;
            if (block.type === "shortcuts") return <table key={index}><tbody>{block.rows.map(([keys, description]) => <tr key={keys}><th scope="row">{keys}</th><td>{description}</td></tr>)}</tbody></table>;
            const items = block.items.map(item => <li key={item}>{item}</li>);
            return block.type === "steps" ? <ol key={index}>{items}</ol> : <ul key={index}>{items}</ul>;
          })}</> : <p>Žiadna téma nezodpovedá hľadaniu.</p>}
        </article>
      </div>
    </section>
  </div>, document.body);
}
