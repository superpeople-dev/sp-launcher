import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  onClose: () => void;
}

/**
 * Play found Windows 11's Smart App Control on, and the game's community DLLs not code-signed yet
 * (smart_app_control.rs): Windows would stop the game at start with "Bad Image". Says why, and how
 * to turn it off for anyone who wants to play before the signed update.
 */
export function SmartAppControlDialog({ onClose }: Props) {
  const [failed, setFailed] = useState("");

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onClose]);

  const open = () => {
    setFailed("");
    void invoke("open_windows_security").catch((e) => setFailed(String(e)));
  };

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="sac-title" onMouseDown={onClose}>
      <div className="dialog__card sac" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title" id="sac-title">
          Windows is blocking the game
        </h2>
        <p className="field__hint replay__text">
          <b>Smart App Control</b>, a Windows 11 security feature, is on and refuses the game's community files because
          they are not code-signed yet. We are getting them signed; until then the game cannot start on this PC while it
          is on.
        </p>
        <p className="field__hint replay__text">To play now, turn it off:</p>
        <ol className="sac__steps">
          <li>
            Open <b>Windows Security</b> (the button below).
          </li>
          <li>
            Go to <b>App &amp; browser control</b>, then <b>Smart App Control settings</b>.
          </li>
          <li>
            Choose <b>Off</b>, then press Play again.
          </li>
        </ol>
        <p className="field__hint replay__text">
          On most PCs it cannot be turned back on without reinstalling Windows. Your antivirus stays on.
        </p>
        {failed && <p className="field__hint is-warn replay__text">{failed}</p>}

        <div className="dialog__actions">
          <button type="button" className="btn" onClick={onClose}>
            Close
          </button>
          <button type="button" className="btn btn--primary" autoFocus onClick={open}>
            Open Windows Security
          </button>
        </div>
      </div>
    </div>
  );
}
