import { useEffect, useMemo, useRef, useState } from "react";
import { ago } from "../../lib/community";
import type { CommunityItem, Person } from "../../types";
import { Icon, type IconName } from "./Icon";
import { ItemDetail } from "./ItemDetail";
import { SuggestDialog } from "./SuggestDialog";
import { useBoard } from "./useBoard";
import { VoteControl } from "./VoteControl";

type Kind = "all" | "bug" | "idea" | "improvement" | "question";
type Sort = "top" | "new";

// The website's types (its first tag), as the dropdown offers them.
const KINDS: { id: Kind; label: string; icon: IconName }[] = [
  { id: "all", label: "All types", icon: "all" },
  { id: "bug", label: "Bugs", icon: "bug" },
  { id: "idea", label: "Ideas", icon: "idea" },
  { id: "improvement", label: "Improvements", icon: "improvement" },
  { id: "question", label: "Questions", icon: "question" },
];
const SORTS: { id: Sort; label: string; icon: IconName }[] = [
  { id: "top", label: "Top", icon: "top" },
  { id: "new", label: "New", icon: "new" },
];

const kindOf = (item: CommunityItem): Kind | null => {
  const name = item.tags[0]?.name.toLowerCase() ?? "";
  return (["bug", "idea", "improvement", "question"] as Kind[]).find((k) => name.startsWith(k)) ?? null;
};

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
    const list = (items ?? []).filter((i) => kind === "all" || kindOf(i) === kind);
    return [...list].sort((a, b) => (sort === "top" ? b.score - a.score : b.createdAt - a.createdAt));
  }, [items, kind, sort]);

  const current = shown.find((i) => i.id === selected) ?? shown[0] ?? null;

  return (
    <section className="panel ideas is-active">
      <div className="ideas__bar">
        <KindPicker value={kind} onChange={setKind} count={(k) => (items ?? []).filter((i) => k === "all" || kindOf(i) === k).length} />
        <div className="seg" role="tablist" aria-label="Sort">
          {SORTS.map((s) => (
            <button key={s.id} type="button" className={`seg__btn${sort === s.id ? " is-on" : ""}`} onClick={() => setSort(s.id)}>
              <Icon name={s.icon} />
              {s.label}
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

/** The type filter: a dropdown of the website's types, with how many of each. */
function KindPicker({ value, onChange, count }: { value: Kind; onChange: (kind: Kind) => void; count: (kind: Kind) => number }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const current = KINDS.find((k) => k.id === value) ?? KINDS[0];

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div className={`pick${open ? " is-open" : ""}`} ref={root}>
      <button type="button" className="pick__btn" aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <Icon name={current.icon} />
        {current.label}
        <Icon name="chevron" className="pick__chev" />
      </button>
      {open && (
        <ul className="pick__menu" role="listbox" aria-label="Type">
          {KINDS.map((k) => (
            <li key={k.id}>
              <button
                type="button"
                role="option"
                aria-selected={k.id === value}
                className={`pick__opt${k.id === value ? " is-on" : ""}`}
                onClick={() => {
                  onChange(k.id);
                  setOpen(false);
                }}
              >
                <Icon name={k.icon} />
                <span className="pick__label">{k.label}</span>
                <span className="pick__count">{count(k.id)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
