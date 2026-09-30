import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { bytes } from "../lib/format";

interface Props {
  installDir: string;
  onClose: () => void;
  /** The game is gone: the Download tab offers to download it again. */
  onUninstalled: () => void;
}

type Step = "confirm" | "removing" | "done" | "failed";

/** Mirrors `download::Footprint`. */
interface Footprint {
  files: number;
  bytes: number;
}

/**
 * The Download tab's uninstaller: what goes and what stays, then a bar while
 * the game's files are deleted (download.rs, remove_game), then a last screen.
 * Only the game's own files go; other files in the folder are kept.
 */
export function UninstallDialog({ installDir, onClose, onUninstalled }: Props) {
  const [step, setStep] = useState<Step>("confirm");
  const [size, setSize] = useState<Footprint | null>(null);
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [error, setError] = useState("");

  useEffect(() => {
    void invoke<Footprint>("game_footprint").then(setSize).catch(() => setSize(null));
    const off = listen<{ done: number; total: number }>("uninstall:progress", (e) => setProgress(e.payload));
    return () => void off.then((f) => f());
  }, []);

  const busy = step === "removing";
  const close = () => {
    if (busy) return;
    if (step === "done") onUninstalled();
    else onClose();
  };

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  });

  const uninstall = async () => {
    setStep("removing");
    setError("");
    try {
      await invoke("uninstall_game");
      setStep("done");
    } catch (e) {
      setError(String(e));
      setStep("failed");
    }
  };

  const pct = progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : 0;

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="uninstall-title" onMouseDown={close}>
      <div className="dialog__card uninstall" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title" id="uninstall-title">
          {step === "done" ? "SUPER PEOPLE was uninstalled" : "Uninstall SUPER PEOPLE"}
        </h2>

        {(step === "confirm" || step === "failed") && (
          <>
            <dl className="uninstall__facts">
              <div>
                <dt>Folder</dt>
                <dd title={installDir}>{installDir}</dd>
              </div>
              <div>
                <dt>Space freed</dt>
                <dd>{size ? `${bytes(size.bytes)} (${size.files.toLocaleString("en-US")} files)` : "Measuring..."}</dd>
              </div>
            </dl>
            <p className="field__hint">
              Only the game's own files are deleted: anything else you keep in that folder stays. To play again,
              download the game from this tab.
            </p>
            {step === "failed" && <p className="field__hint is-warn uninstall__error">{error}</p>}
          </>
        )}

        {step === "removing" && (
          <div className="bigprogress uninstall__bar">
            <div className="bigprogress__head">
              <span className="bigprogress__pct">{`${pct.toFixed(0)}%`}</span>
              <span className="bigprogress__speed">Deleting</span>
            </div>
            <div className="bigprogress__track">
              <div className="bigprogress__fill" style={{ width: `${pct}%` }} />
            </div>
            <div className="bigprogress__file">
              {progress.total > 0
                ? `${progress.done.toLocaleString("en-US")} of ${progress.total.toLocaleString("en-US")} files`
                : "Starting..."}
            </div>
          </div>
        )}

        {step === "done" && (
          <p className="field__hint uninstall__done">
            {size ? `${bytes(size.bytes)} freed. ` : ""}The Download tab can install it again whenever you want.
          </p>
        )}

        <div className="dialog__actions">
          {step === "done" ? (
            <button type="button" className="btn btn--primary" autoFocus onClick={close}>
              Close
            </button>
          ) : (
            <>
              <button type="button" className="btn" disabled={busy} onClick={close}>
                Cancel
              </button>
              <button type="button" className="btn btn--primary" disabled={busy} onClick={() => void uninstall()}>
                {busy ? "Uninstalling..." : step === "failed" ? "Try again" : "Uninstall"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
