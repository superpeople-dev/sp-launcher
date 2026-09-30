import { useMemo, useState } from "react";
import { ago } from "../../lib/community";
import type { CommunityItem, Person } from "../../types";
import { ItemDetail } from "./ItemDetail";
import { SuggestDialog } from "./SuggestDialog";
import { useBoard } from "./useBoard";
import { VoteControl } from "./VoteControl";

type Kind = "all" | "idea" | "bug";
type Sort = "top" | "new";

const isBug = (item: CommunityItem) => item.tags.some((t) => /bug/i.test(t.name));

/** Ideas and bug reports: the list on the left, the selected one with its
 * comments on the right. */
interface Props {
  me: Person;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}

export function IdeasPanel({ me, onError, onNotice }: Props) {
  const { items, vote, replace } = useBoard("ideas", onError);
  const [kind, setKind] = useState<Kind>("all");
  const [sort, setSort] = useState<Sort>("top");
  const [selected, setSelected] = useState<string | null>(null);
  const [suggesting, setSuggesting] = useState(false);

  const shown = useMemo(() => {
    const list = (items ?? []).filter((i) => kind === "all" || (kind === "bug") === isBug(i));
    return [...list].sort((a, b) => (sort === "top" ? b.score - a.score : b.createdAt - a.createdAt));
  }, [items, kind, sort]);

  const current = shown.find((i) => i.id === selected) ?? shown[0] ?? null;

  return (
    <section className="panel ideas is-active">
      <div className="ideas__bar">
        <div className="seg" role="tablist" aria-label="Show">
          {(["all", "idea", "bug"] as Kind[]).map((k) => (
            <button key={k} type="button" className={`seg__btn${kind === k ? " is-on" : ""}`} onClick={() => setKind(k)}>
              {k === "all" ? "All" : k === "idea" ? "Ideas" : "Bugs"}
            </button>
          ))}
        </div>
        <div className="seg" role="tablist" aria-label="Sort">
          {(["top", "new"] as Sort[]).map((s) => (
            <button key={s} type="button" className={`seg__btn${sort === s ? " is-on" : ""}`} onClick={() => setSort(s)}>
              {s === "top" ? "Top" : "New"}
            </button>
          ))}
        </div>
        <button type="button" className="btn btn--primary ideas__suggest" onClick={() => setSuggesting(true)}>
          <svg viewBox="0 0 12 12" aria-hidden>
            <path d="M6 1.5v9M1.5 6h9" stroke="currentColor" strokeWidth="1.6" />
          </svg>
          Suggest
        </button>
      </div>

      <div className="ideas__body">
        <ol className="ideas__list" aria-busy={items === null}>
          {items === null
            ? Array.from({ length: 6 }, (_, i) => <li key={i} className="row row--ghost" />)
            : shown.map((item) => (
                <li key={item.id}>
                  <button
                    type="button"
                    className={`row${current?.id === item.id ? " is-selected" : ""}`}
                    onClick={() => setSelected(item.id)}
                  >
                    <VoteControl item={item} onVote={(v) => vote(item, v)} />
                    <span className="row__main">
                      <span className="row__title">{item.title}</span>
                      <span className="row__meta">
                        {item.tags.slice(0, 1).map((t) => (
                          <span key={t.name} className="row__tag" style={{ color: t.color }}>
                            {t.name}
                          </span>
                        ))}
                        <span className="row__comments">
                          <svg viewBox="0 0 24 24" aria-hidden>
                            <path d="M12 4c-4.7 0-8.5 3.2-8.5 7.2 0 2.2 1.1 4.1 2.9 5.4L5.7 20.5l4.3-2a10 10 0 0 0 2 .2c4.7 0 8.5-3.2 8.5-7.3S16.7 4 12 4z" />
                          </svg>
                          {item.commentCount}
                        </span>
                        <span>{ago(item.createdAt)}</span>
                      </span>
                    </span>
                  </button>
                </li>
              ))}
          {items !== null && shown.length === 0 && <li className="ideas__empty">Nothing here yet.</li>}
        </ol>

        {current ? (
          <ItemDetail item={current} me={me} onVote={vote} onCommented={replace} onError={onError} />
        ) : (
          <div className="detail detail--empty">{items === null ? "" : "Pick an idea to read the discussion."}</div>
        )}
      </div>

      {suggesting && (
        <SuggestDialog
          onClose={() => setSuggesting(false)}
          onPosted={() => {
            setSuggesting(false);
            onNotice("Thanks! Your post shows up here once the team has reviewed it.");
          }}
          onError={onError}
        />
      )}
    </section>
  );
}
