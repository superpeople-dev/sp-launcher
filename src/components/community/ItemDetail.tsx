import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import {
  DESTINATIONS,
  STATUS_COLOR,
  STATUS_LABEL,
  ago,
  assignItem,
  boardOf,
  can,
  cachedThread,
  deleteComment,
  deleteItem,
  loadThread,
  postComment,
  preloadBoards,
  setCommentsOff,
  setStatus,
  storeThread,
} from "../../lib/community";
import type { CommentReply, CommunityItem, ItemStatus, Profile, Thread, Vote } from "../../types";
import { Avatar } from "./Avatar";
import { EditDialog } from "./EditDialog";
import { Icon } from "./Icon";
import { Menu, Picker, TeamMark, type MenuEntry } from "./Picker";
import { VoteControl } from "./VoteControl";

interface Props {
  item: CommunityItem;
  me: Profile;
  onVote: (item: CommunityItem, vote: Vote | null) => void;
  /** The item changed here (a comment, an admin's edit): the list follows. */
  onChange: (item: CommunityItem) => void;
  /** The item left this page (deleted, or moved to another page). */
  onGone: (item: CommunityItem) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
  /** Shown as a panel over the page (Roadmap, Completed) instead of a column. */
  onClose?: () => void;
}

const ROADMAP: ItemStatus[] = ["planned", "in_progress", "completed"];
const count = (thread: Thread) => thread.comments.reduce((n, c) => n + 1 + c.replies.length, 0);

/** One item with its discussion: the Ideas page's right column, and the panel
 * the Roadmap and Completed pages open. An admin also gets the website's admin
 * tools here: approve or reject, move, assign, edit, delete, comments on/off. */
export function ItemDetail({ item, me, onVote, onChange, onGone, onError, onNotice, onClose }: Props) {
  const [thread, setThread] = useState<Thread | null>(() => cachedThread(item.id));
  const [draft, setDraft] = useState("");
  const [posting, setPosting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);

  const manage = can(me, "manage");
  const moderate = can(me, "comments");
  const review = can(me, "review");
  const inReview = item.status === "under_review";

  // A discussion seen before shows at once, then refreshes quietly.
  useEffect(() => {
    let alive = true;
    const known = cachedThread(item.id);
    setThread(known);
    setDraft("");
    setEditing(false);
    scroller.current?.scrollTo({ top: 0 });
    loadThread(item.id)
      .then((fresh) => alive && setThread(fresh))
      .catch((e) => {
        if (!alive || known) return;
        setThread({ comments: [], off: false, assignee: null, staff: [] });
        onError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [item.id, onError]);

  useEffect(() => {
    if (thread) storeThread(item.id, thread);
  }, [item.id, thread]);

  const send = async () => {
    const body = draft.trim();
    if (!body || posting) return;
    setPosting(true);
    try {
      const posted = await postComment(item.id, body);
      setThread((t) => t && { ...t, comments: [...t.comments, { ...posted, mine: true }] });
      setDraft("");
      onChange({ ...item, commentCount: item.commentCount + 1 });
      requestAnimationFrame(() => scroller.current?.scrollTo({ top: scroller.current.scrollHeight, behavior: "smooth" }));
    } catch (e) {
      onError(String(e));
    } finally {
      setPosting(false);
    }
  };

  // An admin action: the website checks the permission again. Afterwards all
  // three pages load again, so an item moved to another page shows up there.
  const act = async (run: () => Promise<void>, done: string) => {
    if (busy) return false;
    setBusy(true);
    try {
      await run();
      onNotice(done);
      preloadBoards();
      return true;
    } catch (e) {
      onError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const move = async (status: ItemStatus) => {
    const approved = inReview && status === "open";
    const label = DESTINATIONS.find((d) => d.status === status)?.label ?? STATUS_LABEL[status];
    const ok = await act(() => setStatus(item.id, status), approved ? "Idea approved." : `Moved to ${label}.`);
    if (!ok) return;
    const moved = { ...item, status, completedAt: status === "completed" ? Date.now() : item.completedAt };
    if (boardOf(status) === boardOf(item.status)) onChange(moved);
    else onGone(moved);
  };

  const remove = async () => {
    const reject = inReview;
    const sure = await ask(`${reject ? "Reject" : "Delete"} “${item.title}”? It can be restored from Reflet.`, {
      title: reject ? "Reject this idea" : "Delete this item",
      kind: "warning",
      okLabel: reject ? "Reject" : "Delete",
      cancelLabel: "Cancel",
    });
    if (sure && (await act(() => deleteItem(item.id), reject ? "Idea rejected." : "Deleted."))) onGone(item);
  };

  const assign = async (id: string | null) => {
    if (!thread) return;
    const before = thread.assignee;
    const next = id ? (thread.staff.find((m) => m.id === id) ?? null) : null;
    setThread({ ...thread, assignee: next });
    const ok = await act(() => assignItem(item.id, id), `Assigned to ${next?.name ?? "the whole team"}.`);
    if (!ok) setThread((t) => t && { ...t, assignee: before });
  };

  const switchComments = async () => {
    if (!thread) return;
    const off = !thread.off;
    if (await act(() => setCommentsOff(item.id, off), off ? "Comments turned off." : "Comments turned back on.")) {
      setThread((t) => t && { ...t, off });
    }
  };

  const dropComment = async (comment: CommentReply) => {
    const sure = await ask("It will be removed for everyone and can't be brought back.", {
      title: "Delete this comment?",
      kind: "warning",
      okLabel: "Delete",
      cancelLabel: "Cancel",
    });
    if (!sure) return;
    try {
      await deleteComment(item.id, comment.id);
      setThread(
        (t) =>
          t && {
            ...t,
            comments: t.comments
              .filter((c) => c.id !== comment.id)
              .map((c) => ({ ...c, replies: c.replies.filter((r) => r.id !== comment.id) })),
          },
      );
      const removed = thread?.comments.find((c) => c.id === comment.id);
      onChange({ ...item, commentCount: Math.max(0, item.commentCount - 1 - (removed?.replies.length ?? 0)) });
    } catch (e) {
      onError(String(e));
    }
  };

  const menu: MenuEntry[] = [];
  if (manage) {
    menu.push({ label: "Edit", icon: "edit", onSelect: () => setEditing(true) }, "line", { heading: "Move to" });
    for (const d of DESTINATIONS) {
      menu.push({ label: d.label, dot: STATUS_COLOR[d.status], current: item.status === d.status, onSelect: () => void move(d.status) });
    }
  }
  if (moderate && thread) {
    if (menu.length) menu.push("line");
    menu.push({
      label: thread.off ? "Turn comments back on" : "Turn off comments",
      icon: thread.off ? "comment" : "lock",
      onSelect: () => void switchComments(),
    });
  }
  if (manage) menu.push("line", { label: inReview ? "Reject" : "Delete", icon: "trash", danger: true, onSelect: () => void remove() });

  // Everyone sees whom a roadmap item is assigned to; an admin who manages
  // items can change it.
  const onRoadmap = ROADMAP.includes(item.status);
  const assignee = !thread ? (
    <span className="skel skel--who" aria-hidden />
  ) : manage && thread.staff.length > 0 ? (
    <Picker
      small
      value={thread.assignee?.id ?? "team"}
      disabled={busy}
      onChange={(id) => void assign(id === "team" ? null : id)}
      options={[
        { id: "team", label: "Whole team", team: true },
        ...thread.staff.filter((m) => m.id).map((m) => ({ id: m.id as string, label: m.name, person: m })),
      ]}
    />
  ) : (
    <span className="who">
      {thread.assignee ? <Avatar person={thread.assignee} size={18} /> : <TeamMark />}
      {thread.assignee?.name ?? "Whole team"}
    </span>
  );

  const closed = thread?.off && (
    <p className="thread__closed" role="status">
      <Icon name="lock" />
      {moderate ? "Comments are turned off. Only admins can still reply." : "Comments are turned off for this item."}
    </p>
  );

  return (
    <article className={`detail${onClose ? " detail--sheet" : ""}`} aria-busy={busy}>
      <div className="detail__scroll" ref={scroller}>
        <header className="detail__head">
          <div className="detail__labels">
            <span className={`status status--${item.status}`}>{STATUS_LABEL[item.status]}</span>
            {item.tags.map((t) => (
              <span key={t.name} className="tag" style={{ color: t.color, borderColor: `${t.color}66` }}>
                {t.name}
              </span>
            ))}
            <span className="detail__actions">
              {menu.length > 0 && <Menu entries={menu} title="Admin actions" disabled={busy} />}
              {onClose && (
                <button type="button" className="detail__close" aria-label="Close" onClick={onClose}>
                  <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden>
                    <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
                  </svg>
                </button>
              )}
            </span>
          </div>
          <h2 className="detail__title">{item.title}</h2>
          <div className="detail__meta">
            <Avatar person={item.author} size={20} />
            <span className="detail__author">{item.author?.name ?? "SUPER PEOPLE"}</span>
            <span className="detail__dot" />
            <span>{ago(item.createdAt)}</span>
            {!inReview && <VoteControl row item={item} onVote={(v) => onVote(item, v)} />}
          </div>
          {onRoadmap && (
            <div className="detail__assigned">
              <span className="detail__fact">Assigned to</span>
              {assignee}
            </div>
          )}
        </header>

        {inReview && review && (
          <div className="review" aria-busy={busy}>
            <Icon name="shield" />
            <span className="review__text">Waiting for review: only admins see it.</span>
            <button type="button" className="btn btn--primary review__btn" disabled={busy} onClick={() => void move("open")}>
              <Icon name="check" />
              Approve
            </button>
            <button type="button" className="btn review__btn review__btn--reject" disabled={busy} onClick={() => void remove()}>
              <Icon name="close" />
              Reject
            </button>
          </div>
        )}

        {item.description && <p className="detail__body">{item.description}</p>}

        <section className="thread">
          <h3 className="thread__title">
            Comments <span className="thread__count">{thread ? count(thread) : item.commentCount}</span>
          </h3>
          {thread === null ? (
            <div className="thread__loading" aria-busy>
              <span />
              <span />
            </div>
          ) : thread.comments.length === 0 ? (
            <p className="thread__empty">No comments yet. Start the discussion.</p>
          ) : (
            <ol className="thread__list">
              {thread.comments.map((c) => (
                <li key={c.id}>
                  <Comment comment={c} onDelete={c.mine || moderate ? () => void dropComment(c) : undefined} />
                  {c.replies.length > 0 && (
                    <ol className="thread__replies">
                      {c.replies.map((r) => (
                        <li key={r.id}>
                          <Comment comment={r} onDelete={r.mine || moderate ? () => void dropComment(r) : undefined} />
                        </li>
                      ))}
                    </ol>
                  )}
                </li>
              ))}
            </ol>
          )}
        </section>
      </div>

      {closed}
      {!(thread?.off && !moderate) && (
        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          <Avatar person={me} size={26} />
          <textarea
            className="composer__input"
            value={draft}
            placeholder={`Comment as ${me.name}`}
            rows={1}
            maxLength={1000}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
            }}
          />
          <button className="btn btn--primary composer__send" type="submit" disabled={!draft.trim() || posting}>
            {posting ? "…" : "Post"}
          </button>
        </form>
      )}

      {editing && (
        <EditDialog
          item={item}
          onClose={() => setEditing(false)}
          onSaved={(fields) => {
            setEditing(false);
            onChange({ ...item, ...fields });
            onNotice("Changes saved.");
            preloadBoards();
          }}
          onError={onError}
        />
      )}
    </article>
  );
}

function Comment({ comment, onDelete }: { comment: CommentReply; onDelete?: () => void }) {
  return (
    <div className="comment">
      <Avatar person={comment.author} size={26} />
      <div className="comment__main">
        <div className="comment__meta">
          <span className="comment__name">{comment.author?.name ?? "Player"}</span>
          {comment.official && <span className="comment__team">Team</span>}
          <span className="comment__time">{ago(comment.createdAt)}</span>
          {onDelete && (
            <button type="button" className="comment__delete" title="Delete comment" aria-label="Delete comment" onClick={onDelete}>
              <Icon name="trash" />
            </button>
          )}
        </div>
        <p className="comment__body">{comment.body}</p>
      </div>
    </div>
  );
}
