import { useEffect } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Ban } from "../types";

const DISCORD_URL = "https://discord.gg/superpeopleofficial";

const dateOf = (ms: number) =>
  new Date(ms).toLocaleString("en-GB", { day: "numeric", month: "long", year: "numeric", hour: "2-digit", minute: "2-digit" });

/** "in 3 days", "in 5 hours", "in 12 minutes": how long a temporary ban still runs. */
function leftOf(until: number) {
  const minutes = Math.max(1, Math.round((until - Date.now()) / 60_000));
  if (minutes >= 2 * 1440) return `in ${Math.round(minutes / 1440)} days`;
  if (minutes >= 120) return `in ${Math.round(minutes / 60)} hours`;
  return minutes === 1 ? "in 1 minute" : `in ${minutes} minutes`;
}

/** The welcome screen's line for a ban until lifted (it signed the launcher out). */
export const bannedLine = (ban: Ban) =>
  `This account is banned from SUPER PEOPLE${ban.reason ? `: ${ban.reason}` : ""}. If you think this is a mistake, contact us on Discord.`;

/** The Play button's small line under "Play" while a temporary ban runs. */
export const bannedNote = (ban: Ban) =>
  ban.until ? `Banned until ${new Date(ban.until).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}` : "Banned";

/**
 * A temporary ban, on the Play page where the launch arguments usually are:
 * when it was made, when it ends, and why (sp-website /api/launcher/me).
 */
export function BanCard({ ban }: { ban: Ban }) {
  return (
    <div className="bancard" role="status">
      <div className="bancard__head">
        <svg className="bancard__icon" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
          <circle cx="7" cy="7" r="5.5" />
          <path d="M3.1 10.9l7.8-7.8" />
        </svg>
        <span className="bancard__title">You are banned</span>
      </div>
      {ban.until && (
        <p className="bancard__line">
          Until <b>{dateOf(ban.until)}</b> ({leftOf(ban.until)})
        </p>
      )}
      <p className="bancard__line">Banned on {dateOf(ban.at)}</p>
      {ban.reason && <p className="bancard__reason">{ban.reason}</p>}
    </div>
  );
}

interface DialogProps {
  ban: Ban;
  /** "play": Play was pressed. "closed": the launcher closed the game after the match. */
  why: "play" | "closed";
  onClose: () => void;
}

/** The same ban, when Play is pressed or after the launcher closed the game. */
export function BanDialog({ ban, why, onClose }: DialogProps) {
  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  });

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="ban-title" onMouseDown={onClose}>
      <div className="dialog__card bandialog" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title bandialog__title" id="ban-title">
          {why === "closed" ? "Your game was closed" : "You can't play right now"}
        </h2>
        <p className="field__hint bandialog__lead">
          {why === "closed"
            ? "You are banned, so the launcher closed the game once your match was over."
            : "You are banned from playing SUPER PEOPLE."}
        </p>
        <dl className="bandialog__facts">
          <dt>Banned on</dt>
          <dd>{dateOf(ban.at)}</dd>
          <dt>Ends</dt>
          <dd>{ban.until ? `${dateOf(ban.until)} (${leftOf(ban.until)})` : "When it is lifted"}</dd>
          {ban.reason && (
            <>
              <dt>Reason</dt>
              <dd className="bandialog__reason">{ban.reason}</dd>
            </>
          )}
        </dl>
        <p className="field__hint">If you think this is a mistake, contact us on Discord.</p>
        <div className="dialog__actions">
          <button type="button" className="btn" onClick={() => void openUrl(DISCORD_URL).catch(() => {})}>
            Discord
          </button>
          <button type="button" className="btn btn--primary" onClick={onClose}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
