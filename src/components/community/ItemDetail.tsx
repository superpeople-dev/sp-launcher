import { useEffect, useRef, useState } from "react";
import { STATUS_LABEL, ago, cachedComments, loadComments, postComment, storeComments } from "../../lib/community";
import type { CommunityComment, CommunityItem, CommentReply, Person, Vote } from "../../types";
import { Avatar } from "./Avatar";
import { VoteControl } from "./VoteControl";

interface Props {
  item: CommunityItem;
  me: Person;
  onVote: (item: CommunityItem, vote: Vote | null) => void;
  /** A comment was posted: the list's comment count follows. */
  onCommented: (item: CommunityItem) => void;
  onError: (message: string) => void;
  /** Shown as a panel over the page (Roadmap, Completed) instead of a column. */
  onClose?: () => void;
}

/** One item with its discussion: the Ideas page's right column, and the panel
 * the Roadmap and Completed pages open. */
export function ItemDetail({ item, me, onVote, onCommented, onError, onClose }: Props) {
  const [comments, setComments] = useState<CommunityComment[] | null>(() => cachedComments(item.id));
  const [draft, setDraft] = useState("");
  const [posting, setPosting] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);

  // Comments seen before show at once, then refresh quietly.
  useEffect(() => {
    let alive = true;
    const known = cachedComments(item.id);
    setComments(known);
    setDraft("");
    scroller.current?.scrollTo({ top: 0 });
    loadComments(item.id)
      .then((list) => alive && setComments(list))
      .catch((e) => {
        if (!alive || known) return;
        setComments([]);
        onError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [item.id, onError]);

  useEffect(() => {
    if (comments) storeComments(item.id, comments);
  }, [item.id, comments]);

  const send = async () => {
    const body = draft.trim();
    if (!body || posting) return;
    setPosting(true);
    try {
      const posted = await postComment(item.id, body);
      setComments((list) => [...(list ?? []), posted]);
      setDraft("");
      onCommented({ ...item, commentCount: item.commentCount + 1 });
      requestAnimationFrame(() => scroller.current?.scrollTo({ top: scroller.current.scrollHeight, behavior: "smooth" }));
    } catch (e) {
      onError(String(e));
    } finally {
      setPosting(false);
    }
  };

  return (
    <article className={`detail${onClose ? " detail--sheet" : ""}`}>
      <div className="detail__scroll" ref={scroller}>
        <header className="detail__head">
          <div className="detail__labels">
            <span className={`status status--${item.status}`}>{STATUS_LABEL[item.status]}</span>
            {item.tags.map((t) => (
              <span key={t.name} className="tag" style={{ color: t.color, borderColor: `${t.color}66` }}>
                {t.name}
              </span>
            ))}
            {onClose && (
              <button type="button" className="detail__close" aria-label="Close" onClick={onClose}>
                <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden>
                  <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
                </svg>
              </button>
            )}
          </div>
          <h2 className="detail__title">{item.title}</h2>
          <div className="detail__meta">
            <Avatar person={item.author} size={20} />
            <span className="detail__author">{item.author?.name ?? "SUPER PEOPLE"}</span>
            <span className="detail__dot" />
            <span>{ago(item.createdAt)}</span>
            <VoteControl row item={item} onVote={(v) => onVote(item, v)} />
          </div>
        </header>

        {item.description && <p className="detail__body">{item.description}</p>}

        <section className="thread">
          <h3 className="thread__title">
            Comments{" "}
            <span className="thread__count">
              {comments ? comments.reduce((n, c) => n + 1 + c.replies.length, 0) : item.commentCount}
            </span>
          </h3>
          {comments === null ? (
            <div className="thread__loading" aria-busy>
              <span />
              <span />
            </div>
          ) : comments.length === 0 ? (
            <p className="thread__empty">No comments yet. Start the discussion.</p>
          ) : (
            <ol className="thread__list">
              {comments.map((c) => (
                <li key={c.id}>
                  <Comment comment={c} />
                  {c.replies.length > 0 && (
                    <ol className="thread__replies">
                      {c.replies.map((r) => (
                        <li key={r.id}>
                          <Comment comment={r} />
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
          maxLength={2000}
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
    </article>
  );
}

function Comment({ comment }: { comment: CommentReply }) {
  return (
    <div className="comment">
      <Avatar person={comment.author} size={26} />
      <div className="comment__main">
        <div className="comment__meta">
          <span className="comment__name">{comment.author?.name ?? "Player"}</span>
          {comment.official && <span className="comment__team">Team</span>}
          <span className="comment__time">{ago(comment.createdAt)}</span>
        </div>
        <p className="comment__body">{comment.body}</p>
      </div>
    </div>
  );
}
