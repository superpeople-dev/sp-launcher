import { useMemo, useState } from "react";
import { day } from "../../lib/community";
import type { Person } from "../../types";
import { ItemDetail } from "./ItemDetail";
import { useBoard } from "./useBoard";

const PAGE = 5;

/** The latest finished work, newest first: five at a time. */
export function CompletedPanel({ me, onError }: { me: Person; onError: (message: string) => void }) {
  const { items, vote, replace } = useBoard("completed", onError);
  const [shown, setShown] = useState(PAGE);
  const [open, setOpen] = useState<string | null>(null);

  const done = useMemo(
    () => [...(items ?? [])].sort((a, b) => (b.completedAt ?? b.createdAt) - (a.completedAt ?? a.createdAt)),
    [items],
  );
  const current = done.find((i) => i.id === open) ?? null;

  return (
    <section className="panel done is-active">
      <header className="done__head">
        <h2 className="done__title">Recently completed</h2>
        <p className="done__lead">What the team finished lately, from your ideas and bug reports.</p>
      </header>

      <ol className="timeline" aria-busy={items === null}>
        {items === null
          ? Array.from({ length: PAGE }, (_, i) => <li key={i} className="entry entry--ghost" />)
          : done.slice(0, shown).map((item) => (
              <li key={item.id}>
                <button type="button" className="entry" onClick={() => setOpen(item.id)}>
                  <span className="entry__date">{day(item.completedAt ?? item.createdAt)}</span>
                  <span className="entry__mark" aria-hidden>
                    <svg viewBox="0 0 16 16">
                      <path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" strokeWidth="2" />
                    </svg>
                  </span>
                  <span className="entry__main">
                    <span className="entry__title">{item.title}</span>
                    {item.description && <span className="entry__desc">{item.description}</span>}
                    <span className="entry__meta">
                      {item.tags.map((t) => (
                        <span key={t.name} className="tag" style={{ color: t.color, borderColor: `${t.color}66` }}>
                          {t.name}
                        </span>
                      ))}
                      <span className="entry__score">▲ {item.score}</span>
                    </span>
                  </span>
                </button>
              </li>
            ))}
      </ol>

      {items !== null && done.length > shown && (
        <button type="button" className="btn done__more" onClick={() => setShown((n) => n + PAGE)}>
          Show {Math.min(PAGE, done.length - shown)} more
        </button>
      )}
      {items !== null && done.length === 0 && <p className="done__empty">Nothing finished yet.</p>}

      {current && (
        <div className="sheet" onMouseDown={() => setOpen(null)}>
          <div onMouseDown={(e) => e.stopPropagation()}>
            <ItemDetail item={current} me={me} onVote={vote} onCommented={replace} onError={onError} onClose={() => setOpen(null)} />
          </div>
        </div>
      )}
    </section>
  );
}
