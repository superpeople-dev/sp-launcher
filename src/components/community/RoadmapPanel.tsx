import { useState } from "react";
import type { CommunityItem, ItemStatus, Profile } from "../../types";
import { ItemDetail } from "./ItemDetail";
import { Tag } from "./Tag";
import { useBoard } from "./useBoard";
import { VoteControl } from "./VoteControl";

const COLUMNS: { status: ItemStatus; title: string; hint: string }[] = [
  { status: "planned", title: "Planned", hint: "Up next" },
  { status: "in_progress", title: "In progress", hint: "Being worked on now" },
];

/** What the team works on now and next, as two columns. A card opens its
 * discussion over the board. */
interface Props {
  me: Profile;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}

export function RoadmapPanel({ me, onError, onNotice }: Props) {
  const { items, vote, replace, remove } = useBoard("roadmap", onError);
  const [open, setOpen] = useState<string | null>(null);
  const current = items?.find((i) => i.id === open) ?? null;

  return (
    <section className="panel roadmap is-active">
      {COLUMNS.map((col) => {
        const list = (items ?? []).filter((i) => i.status === col.status).sort((a, b) => b.score - a.score);
        return (
          <div key={col.status} className={`lane lane--${col.status}`}>
            <header className="lane__head">
              <span className="lane__dot" />
              <h2 className="lane__title">{col.title}</h2>
              <span className="lane__count">{items ? list.length : ""}</span>
              <span className="lane__hint">{col.hint}</span>
            </header>
            <ol className="lane__list" aria-busy={items === null}>
              {items === null
                ? Array.from({ length: 4 }, (_, i) => <li key={i} className="task task--ghost" />)
                : list.map((item) => (
                    <li key={item.id}>
                      <Task item={item} onOpen={() => setOpen(item.id)} onVote={(v) => vote(item, v)} />
                    </li>
                  ))}
              {items !== null && list.length === 0 && <li className="lane__empty">Nothing here right now.</li>}
            </ol>
          </div>
        );
      })}

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

function Task({ item, onOpen, onVote }: { item: CommunityItem; onOpen: () => void; onVote: Parameters<typeof VoteControl>[0]["onVote"] }) {
  return (
    <div className="task" role="button" tabIndex={0} onClick={onOpen} onKeyDown={(e) => e.key === "Enter" && onOpen()}>
      <div className="task__tags">
        {item.tags.map((t) => <Tag key={t.name} tag={t} />)}
      </div>
      <p className="task__title">{item.title}</p>
      <div className="task__foot">
        <VoteControl row item={item} onVote={onVote} />
        <span className="task__comments">
          <svg viewBox="0 0 24 24" aria-hidden>
            <path d="M12 4c-4.7 0-8.5 3.2-8.5 7.2 0 2.2 1.1 4.1 2.9 5.4L5.7 20.5l4.3-2a10 10 0 0 0 2 .2c4.7 0 8.5-3.2 8.5-7.3S16.7 4 12 4z" />
          </svg>
          {item.commentCount}
        </span>
      </div>
    </div>
  );
}
