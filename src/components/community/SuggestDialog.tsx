import { useEffect, useState } from "react";
import { LIMITS, listPlatforms, postIdea, type IdeaKind, type Platform } from "../../lib/community";

interface Props {
  onClose: () => void;
  /** Posted: it shows on the board once the team has reviewed it. */
  onPosted: () => void;
  onError: (message: string) => void;
}

/** Suggest an idea or report a bug, posted to the website as the player. */
export function SuggestDialog({ onClose, onPosted, onError }: Props) {
  const [kind, setKind] = useState<IdeaKind>("idea");
  // The website's platforms (Game, Launcher, ...) plus "other"; Game first when there is one.
  const [platforms, setPlatforms] = useState<Platform[]>([]);
  const [platform, setPlatform] = useState("other");
  useEffect(() => {
    listPlatforms()
      .then((list) => {
        setPlatforms(list);
        const game = list.find((p) => /game/i.test(p.name)) ?? list[0];
        if (game) setPlatform(game.id);
      })
      .catch(() => {});
  }, []);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [min] = LIMITS.title;
  const ready = title.trim().length >= min && !busy;

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    try {
      await postIdea(title.trim(), description.trim(), kind, platform);
      onPosted();
    } catch (e) {
      onError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="suggest-title" onMouseDown={onClose}>
      <form
        className="dialog__card"
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <h2 className="card__title" id="suggest-title">
          {kind === "bug" ? "Report a bug" : "Suggest an idea"}
        </h2>
        <div className="dialog__row">
          <div className="field">
            <span className="field__label">Type</span>
            <div className="seg">
              {(["idea", "bug"] as IdeaKind[]).map((k) => (
                <button key={k} type="button" className={`seg__btn${kind === k ? " is-on" : ""}`} onClick={() => setKind(k)}>
                  {k === "idea" ? "Idea" : "Bug"}
                </button>
              ))}
            </div>
          </div>
          <div className="field">
            <span className="field__label">About</span>
            <div className="seg">
              {[...platforms, { id: "other", name: "Other" }].map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className={`seg__btn${platform === p.id ? " is-on" : ""}`}
                  onClick={() => setPlatform(p.id)}
                >
                  {p.name}
                </button>
              ))}
            </div>
          </div>
        </div>
        <label className="field">
          <span className="field__label">
            Title <span className="field__count">{title.length}/{LIMITS.title[1]}</span>
          </span>
          <input
            className="input"
            value={title}
            maxLength={LIMITS.title[1]}
            autoFocus
            placeholder={kind === "bug" ? "What goes wrong, in a few words" : "Your idea, in a few words"}
            onChange={(e) => setTitle(e.target.value)}
          />
        </label>
        <label className="field">
          <span className="field__label">
            Details <span className="field__count">{description.length}/{LIMITS.description}</span>
          </span>
          <textarea
            className="input dialog__text"
            value={description}
            maxLength={LIMITS.description}
            rows={6}
            placeholder={
              kind === "bug"
                ? "What did you do, what happened, and what did you expect?"
                : "What should change, and why would it be better?"
            }
            onChange={(e) => setDescription(e.target.value)}
          />
        </label>
        <p className="field__hint">The team reviews new posts before they show up for everyone.</p>
        <div className="dialog__actions">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn btn--primary" disabled={!ready}>
            {busy ? "Posting…" : "Post"}
          </button>
        </div>
      </form>
    </div>
  );
}
