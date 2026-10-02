import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import logo from "../assets/sp-logo.png";
import type { Profile, Tab } from "../types";
import { Avatar } from "./community/Avatar";
import { NameDialog } from "./NameDialog";

// Settings lives in the profile menu, next to Sign out: the tabs fill the 860 px window (six with
// short labels leave about 35 px). Download always comes right after Play.
const TABS: { id: Tab; label: string }[] = [
  { id: "play", label: "Play" },
  { id: "download", label: "Download" },
  { id: "ideas", label: "Ideas" },
  { id: "roadmap", label: "Roadmap" },
  { id: "completed", label: "Completed" },
  { id: "twitch", label: "Twitch" },
];

interface Props {
  tab: Tab;
  onTab: (tab: Tab) => void;
  /** Null until the player connects Discord: only the window buttons show. */
  profile: Profile | null;
  onSignOut: () => void;
}

export function TitleBar({ tab, onTab, profile, onSignOut }: Props) {
  // Minimize behaves like a normal window: it drops to the taskbar, not the
  // tray. Only the X button (and Alt+F4, handled on the Rust side) sends the
  // window to the tray — the tray icon's "Open" item and the taskbar Quit
  // are the two ways back, and the tray also has a "Quit" for actually
  // ending the process.
  const minimize = () => void getCurrentWindow().minimize();
  const hideToTray = () => void invoke("hide_to_tray");
  const [renaming, setRenaming] = useState(false);

  return (
    <>
      <header className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <img className="brand__logo" src={logo} alt="SP" draggable={false} />
        </div>

        {profile ? (
          <nav className="nav">
            {TABS.map((t) => (
              <button
                key={t.id}
                className={`tab${tab === t.id ? " is-active" : ""}`}
                onClick={() => onTab(t.id)}
                type="button"
              >
                {t.label}
              </button>
            ))}
          </nav>
        ) : (
          <div className="nav" data-tauri-drag-region />
        )}

        {profile && <ServerPill />}
        {profile && (
          <ProfileMenu
            profile={profile}
            settings={tab === "settings"}
            onRename={() => setRenaming(true)}
            onSettings={() => onTab("settings")}
            onSignOut={onSignOut}
          />
        )}

        <div className="winbtns">
          <button className="winbtn" title="Minimize" type="button" onClick={minimize}>
            <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.4">
              <path d="M2 6h8" />
            </svg>
          </button>
          <button className="winbtn winbtn--close" title="Minimize to tray" type="button" onClick={hideToTray}>
            <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.4">
              <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" />
            </svg>
          </button>
        </div>
      </header>
      {renaming && profile && <NameDialog onClose={() => setRenaming(false)} />}
    </>
  );
}

/** The player's Discord picture; opens their name, the in-game name, Settings and Sign out. */
function ProfileMenu({
  profile,
  settings,
  onRename,
  onSettings,
  onSignOut,
}: {
  profile: Profile;
  settings: boolean;
  onRename: () => void;
  onSettings: () => void;
  onSignOut: () => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  const pick = (action: () => void) => () => {
    setOpen(false);
    action();
  };

  return (
    <div className={`me${open || settings ? " is-open" : ""}`} ref={root}>
      <button
        type="button"
        className="me__btn"
        aria-haspopup="menu"
        aria-expanded={open}
        title={profile.name}
        onClick={() => setOpen((o) => !o)}
      >
        <Avatar person={profile} size={26} />
        <svg className="me__chev" viewBox="0 0 10 10" aria-hidden>
          <path d="M2 3.5l3 3 3-3" fill="none" stroke="currentColor" strokeWidth="1.4" />
        </svg>
      </button>
      {open && (
        <div className="me__menu" role="menu">
          <div className="me__who">
            <Avatar person={profile} size={36} />
            <div>
              <div className="me__name">{profile.name}</div>
              <div className="me__user">@{profile.username}</div>
            </div>
          </div>
          <button type="button" role="menuitem" className="me__item" onClick={pick(onRename)}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden>
              <path d="M4 20h4L19 9l-4-4L4 16v4z" />
              <path d="M13 7l4 4" />
            </svg>
            In-game name
          </button>
          <button type="button" role="menuitem" className="me__item" onClick={pick(onSettings)}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden>
              <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
              <circle cx="15" cy="7" r="2" />
              <circle cx="9" cy="17" r="2" />
            </svg>
            Settings
          </button>
          <button type="button" role="menuitem" className="me__item me__item--out" onClick={pick(onSignOut)}>
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d="M10 4H5v16h5M15 8l4 4-4 4M19 12H9" fill="none" stroke="currentColor" strokeWidth="1.8" />
            </svg>
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}

// Backend status next to the window buttons: green dot + "N playing" while the
// backend answers, red dot + "Offline" when it does not. Polled every 30 s; the
// Rust side does the request (the webview's CSP allows no outside hosts).
const STATUS_POLL_MS = 30_000;

function ServerPill() {
  const [st, setSt] = useState<{ online: boolean; players: number } | null>(null);

  useEffect(() => {
    let alive = true;
    const poll = () =>
      void invoke<{ online: boolean; players: number }>("server_status")
        .then((s) => alive && setSt(s))
        .catch(() => alive && setSt({ online: false, players: 0 }));
    poll();
    const id = window.setInterval(poll, STATUS_POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);

  if (!st) return null;
  return (
    <div className={`srvpill${st.online ? " is-online" : " is-offline"}`} title={st.online ? "Backend online" : "Backend offline"}>
      <span className="srvpill__dot" />
      <span className="srvpill__text">{st.online ? `${st.players} playing` : "Offline"}</span>
    </div>
  );
}
