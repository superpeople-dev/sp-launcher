import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { dayAndYear } from "../../lib/community";
import type { Profile } from "../../types";
import { ItemDetail } from "./ItemDetail";
import { Tag } from "./Tag";
import { useBoard } from "./useBoard";

const PAGE = 10;
const NEAR = 240;

/** The latest finished work, newest first: ten at a time, the next ten as the list nears its end. */
interface Props {
  me: Profile;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}

export function CompletedPanel({ me, onError, onNotice }: Props) {
  const { items, vote, replace, remove } = useBoard("completed", onError);
  const [shown, setShown] = useState(PAGE);
  const [open, setOpen] = useState<string | null>(null);

  const done = useMemo(
    () => [...(items ?? [])].sort((a, b) => (b.completedAt ?? b.createdAt) - (a.completedAt ?? a.createdAt)),
    [items],
  );
  const current = done.find((i) => i.id === open) ?? null;

  // Scrolling to within NEAR of the list's end shows the next ones. Checked again after each page
  // and when the window changes size, so a list too short to scroll keeps filling until it can or
  // runs out.
  const list = useRef<HTMLOListElement>(null);
  const more = items !== null && done.length > shown;
  const fill = useCallback(() => {
    const el = list.current;
    if (more && el && el.scrollHeight - el.scrollTop - el.clientHeight < NEAR) setShown((n) => n + PAGE);
  }, [more]);
  useEffect(fill, [fill, shown]);
  useEffect(() => {
    window.addEventListener("resize", fill);
    return () => window.removeEventListener("resize", fill);
  }, [fill]);

  return (
    <section className="panel done is-active">
      <header className="done__head">
        <h2 className="done__title">Recently completed</h2>
        <p className="done__lead">What the team finished lately, from your ideas and bug reports.</p>
      </header>

      <ol className="timeline" aria-busy={items === null} ref={list} onScroll={fill}>
        {items === null
          ? Array.from({ length: PAGE }, (_, i) => <li key={i} className="entry entry--ghost" />)
          : done.slice(0, shown).map((item) => {
              const when = dayAndYear(item.completedAt ?? item.createdAt);
              return (
                <li key={item.id}>
                  <button type="button" className="entry" onClick={() => setOpen(item.id)}>
                    <span className="entry__date">
                      {when.day}
                      {when.year && <span className="entry__year">{when.year}</span>}
                    </span>
                    <span className="entry__main">
                      <span className="entry__title">{item.title}</span>
                      {item.description && <span className="entry__desc">{item.description}</span>}
                      <span className="entry__meta">
                        {item.tags.map((t) => <Tag key={t.name} tag={t} />)}
                        <span className="entry__score">▲ {item.score}</span>
                      </span>
                    </span>
                  </button>
                </li>
              );
            })}
      </ol>

      {items !== null && done.length === 0 && <p className="done__empty">Nothing finished yet.</p>}

      {current && (
        <div className="sheet" onMouseDown={() => setOpen(null)}>
          <div onMouseDown={(e) => e.stopPropagation()}>
            <ItemDetail
              item={current}
              me={me}
              onVote={vote}
              onChange={replace}
              onGone={(item) => {
                remove(item);
                setOpen(null);
              }}
              onError={onError}
              onNotice={onNotice}
              onClose={() => setOpen(null)}
            />
          </div>
        </div>
      )}
    </section>
  );
}
