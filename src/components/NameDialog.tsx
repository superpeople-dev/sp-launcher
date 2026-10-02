import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  onClose: () => void;
}

/** Mirrors `auth::GameName`: next_change_at is null when the name may change now. */
interface GameName {
  name: string;
  next_change_at: string | null;
}

// The backend's rules (sp-backend lib/playername.js), checked here first so a typo shows at once; the
// backend checks them again, and the names nobody may have.
const MIN = 2;
const MAX = 16;
const SHAPE = /^[\p{L}\p{M}\p{N}_.-]+$/u;

function problem(name: string) {
  const length = [...name].length;
  if (length < MIN || length > MAX) return `A name has ${MIN} to ${MAX} characters.`;
  if (!SHAPE.test(name)) return "Use letters, numbers, _ . and - only, without spaces.";
  return null;
}

const day = (iso: string) =>
  new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" });

/**
 * The profile menu's "In-game name": the name the game shows, and a new one, once every 14 days
 * (account_name in lib.rs). The game shows a new name from the next time it starts.
 */
export function NameDialog({ onClose }: Props) {
  const [current, setCurrent] = useState<GameName | null>(null);
  const [loadError, setLoadError] = useState("");
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [done, setDone] = useState(false);

  useEffect(() => {
    let live = true;
    invoke<GameName>("account_name")
      .then((got) => live && setCurrent(got))
      .catch((e) => live && setLoadError(String(e)));
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && !saving && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  });

  const name = value.trim();
  const waiting = current?.next_change_at ? new Date(current.next_change_at).getTime() > Date.now() : false;
  const hint = name ? problem(name) : null;
  const canSave = !!current && !waiting && !saving && !!name && !hint && name !== current.name;

  const save = async () => {
    if (!canSave) return;
    setSaving(true);
    setError("");
    try {
      setCurrent(await invoke<GameName>("account_name", { name }));
      setDone(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="name-title" onMouseDown={() => !saving && onClose()}>
      <div className="dialog__card rename" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title" id="name-title">
          In-game name
        </h2>

        {loadError ? (
          <p className="field__hint is-warn">{loadError}</p>
        ) : !current ? (
          <p className="field__hint">Loading...</p>
        ) : done ? (
          <p className="field__hint rename__done">
            Your name is now <b>{current.name}</b>. The game shows it the next time you start it.
          </p>
        ) : (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <dl className="uninstall__facts">
              <div>
                <dt>Now</dt>
                <dd>{current.name}</dd>
              </div>
            </dl>
            <label className="field">
              <span className="field__label">New name</span>
              <input
                className="input"
                value={value}
                maxLength={MAX + 8}
                autoFocus
                spellCheck={false}
                autoComplete="off"
                disabled={waiting || saving}
                placeholder={current.name}
                onChange={(e) => {
                  setValue(e.target.value);
                  setError("");
                }}
              />
            </label>
            <p className={`field__hint${hint || error || waiting ? " is-warn" : ""}`}>
              {waiting && current.next_change_at
                ? `You can change it again on ${day(current.next_change_at)}.`
                : error || hint || `${MIN} to ${MAX} letters, numbers, _ . or -. You can change it once every 14 days.`}
            </p>
          </form>
        )}

        <div className="dialog__actions">
          {done || loadError || waiting ? (
            <button type="button" className="btn btn--primary" autoFocus={done} onClick={onClose}>
              Close
            </button>
          ) : (
            <>
              <button type="button" className="btn" disabled={saving} onClick={onClose}>
                Cancel
              </button>
              <button type="button" className="btn btn--primary" disabled={!canSave} onClick={() => void save()}>
                {saving ? "Saving..." : "Change name"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
