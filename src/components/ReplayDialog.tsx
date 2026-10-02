import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  onClose: () => void;
}

/** Mirrors `replays::Imported`: the recording's folder in the game's Demos, and whether it was there. */
interface Imported {
  name: string;
  already: boolean;
}

type Step = { at: "ask" } | { at: "busy" } | { at: "done"; got: Imported } | { at: "error"; message: string };

/**
 * A reported match opened from its page in the browser (sp-launcher://replay/..., replays.rs): asks
 * first, then puts it in the game's replays (replay_import in lib.rs), where the game's Replay menu
 * lists it.
 */
export function ReplayDialog({ onClose }: Props) {
  const [step, setStep] = useState<Step>({ at: "ask" });
  const busy = step.at === "busy";

  const close = () => {
    if (busy) return;
    if (step.at === "ask") void invoke("replay_dismiss");
    onClose();
  };

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  });

  const add = async () => {
    setStep({ at: "busy" });
    try {
      setStep({ at: "done", got: await invoke<Imported>("replay_import") });
    } catch (e) {
      setStep({ at: "error", message: String(e) });
    }
  };

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="replay-title" onMouseDown={close}>
      <div className="dialog__card replay" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title" id="replay-title">
          Reported match
        </h2>

        {step.at === "ask" && (
          <p className="field__hint replay__text">
            Add this reported match to your game's replays? The launcher downloads it from the server (usually 2 to 15
            MB).
          </p>
        )}
        {step.at === "busy" && <p className="field__hint replay__text">Downloading the replay...</p>}
        {step.at === "done" && (
          <p className="field__hint replay__text">
            <b>{step.got.name}</b> {step.got.already ? "is already in" : "is now in"} your game's replays. Start the game,
            open <b>Replay</b> and pick it.
          </p>
        )}
        {step.at === "error" && <p className="field__hint is-warn replay__text">{step.message}</p>}

        <div className="dialog__actions">
          {step.at === "ask" || step.at === "busy" ? (
            <>
              <button type="button" className="btn" disabled={busy} onClick={close}>
                Cancel
              </button>
              <button type="button" className="btn btn--primary" autoFocus disabled={busy} onClick={() => void add()}>
                Add to my replays
              </button>
            </>
          ) : (
            <button type="button" className="btn btn--primary" autoFocus onClick={onClose}>
              Close
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
