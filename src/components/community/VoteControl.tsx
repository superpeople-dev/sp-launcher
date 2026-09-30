import type { CommunityItem, Vote } from "../../types";

interface Props {
  item: CommunityItem;
  onVote: (vote: Vote | null) => void;
  /** A row: arrows either side of the score, for headers. Default: a column. */
  row?: boolean;
  disabled?: boolean;
}

const ARROW = "M12 5 5 13h4.5v6h5v-6H19z";

/** Up and down arrows around the score, as on the website: pressing the
 * arrow already chosen takes the vote back. */
export function VoteControl({ item, onVote, row, disabled }: Props) {
  const toggle = (vote: Vote) => onVote(item.myVote === vote ? null : vote);
  return (
    <div
      className={`vote${row ? " vote--row" : ""}${item.myVote ? ` is-${item.myVote}` : ""}`}
      onClick={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        className="vote__btn vote__btn--up"
        aria-label="Upvote"
        aria-pressed={item.myVote === "up"}
        disabled={disabled}
        onClick={() => toggle("up")}
      >
        <svg viewBox="0 0 24 24" aria-hidden>
          <path d={ARROW} />
        </svg>
      </button>
      <span className="vote__score">{item.score}</span>
      <button
        type="button"
        className="vote__btn vote__btn--down"
        aria-label="Downvote"
        aria-pressed={item.myVote === "down"}
        disabled={disabled}
        onClick={() => toggle("down")}
      >
        <svg viewBox="0 0 24 24" aria-hidden style={{ transform: "rotate(180deg)" }}>
          <path d={ARROW} />
        </svg>
      </button>
    </div>
  );
}
