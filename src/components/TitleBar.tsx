import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import logo from "../assets/sp-logo.png";
import type { Profile, Tab } from "../types";
import { Avatar } from "./community/Avatar";
import { NameDialog } from "./NameDialog";

// The pages, grouped so the 860 px window holds as many as needed: Play and Download are one
// click away (Download always right after Play), the others sit in sections whose list opens below
// them on hover or click. Settings lives in the profile menu, next to Sign out.
type Page = { id: Tab; label: string; hint: string; icon?: ReactNode };

// A section page's icon: 24-unit line drawings like the profile menu's, in the text colour.
const icon = (paths: ReactNode) => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    {paths}
  </svg>
);
const ICONS = {
  // A light bulb: ideas and bug reports.
  ideas: icon(
    <>
      <path d="M9 18h6M10 21h4" />
      <path d="M12 3a6 6 0 0 0-3.6 10.8c.7.5 1.1 1.3 1.1 2.1V16h5v-.1c0-.8.4-1.6 1.1-2.1A6 6 0 0 0 12 3z" />
    </>,
  ),
  // A folded map with its route: what the team is working on.
  roadmap: icon(
    <>
      <path d="M9 4L3 6.5v13.5l6-2.5 6 2.5 6-2.5V4l-6 2.5L9 4z" />
      <path d="M9 4v13.5M15 6.5V20" />
    </>,
  ),
  // A tick in a circle: what was finished.
  completed: icon(
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M8 12.5l2.8 2.8L16 9.8" />
    </>,
  ),
  // A cup: the season's top 100.
  leaderboard: icon(
    <>
      <path d="M7 4h10v5a5 5 0 0 1-10 0V4z" />
      <path d="M7 6H4v1.5A3.5 3.5 0 0 0 7.5 11M17 6h3v1.5A3.5 3.5 0 0 1 16.5 11M12 14v4M8 21h8M9.5 18h5" />
    </>,
  ),
  // Twitch's speech-bubble mark.
  twitch: icon(
    <>
      <path d="M5 3L3.5 6.5V19H8v2.5l2.5-2.5h3.5l5.5-5.5V3H5z" />
      <path d="M11 8v4.5M15.5 8v4.5" />
    </>,
  ),
};
type Section = { label: string; pages: Page[] };
const SECTIONS: Section[] = [
  { label: "Play", pages: [{ id: "play", label: "Play", hint: "" }] },
  { label: "Download", pages: [{ id: "download", label: "Download", hint: "" }] },
  {
    label: "Community",
    pages: [
      { id: "ideas", label: "Ideas", hint: "Vote on ideas and bug reports, or post your own", icon: ICONS.ideas },
      { id: "roadmap", label: "Roadmap", hint: "What the team is working on", icon: ICONS.roadmap },
      { id: "completed", label: "Completed", hint: "What was finished lately", icon: ICONS.completed },
    ],
  },
  {
    label: "Players",
    pages: [
      { id: "leaderboard", label: "Leaderboard", hint: "The season's top 100 of each mode", icon: ICONS.leaderboard },
      { id: "twitch", label: "Twitch", hint: "The most popular SUPER PEOPLE streams, live", icon: ICONS.twitch },
    ],
  },
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
            {SECTIONS.map((s) =>
              s.pages.length === 1 ? (
                <button
                  key={s.label}
                  className={`tab${tab === s.pages[0].id ? " is-active" : ""}`}
                  onClick={() => onTab(s.pages[0].id)}
                  type="button"
                >
                  {s.label}
                </button>
              ) : (
                <SectionMenu key={s.label} section={s} tab={tab} onTab={onTab} />
              ),
            )}
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

// A section's list stays open this long after the pointer leaves, so moving down into it from the
// tab does not close it.
const CLOSE_MS = 160;

/** A section of the top menu: its pages in a list below it, on hover or click. */
function SectionMenu({ section, tab, onTab }: { section: Section; tab: Tab; onTab: (tab: Tab) => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const timer = useRef(0);
  const active = section.pages.some((p) => p.id === tab);

  const show = () => {
    window.clearTimeout(timer.current);
    setOpen(true);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpen(false), CLOSE_MS);
  };

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

  useEffect(() => () => window.clearTimeout(timer.current), []);

  return (
    <div className={`section${open ? " is-open" : ""}`} ref={root} onMouseEnter={show} onMouseLeave={hide}>
      <button
        type="button"
        className={`tab section__btn${active ? " is-active" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        {section.label}
        <svg className="section__chev" viewBox="0 0 10 10" aria-hidden>
          <path d="M2 3.5l3 3 3-3" fill="none" stroke="currentColor" strokeWidth="1.4" />
        </svg>
      </button>
      {open && (
        <div className="section__menu" role="menu">
          {section.pages.map((p) => (
            <button
              key={p.id}
              type="button"
              role="menuitem"
              className={`section__item${p.id === tab ? " is-on" : ""}`}
              onClick={() => {
                setOpen(false);
                onTab(p.id);
              }}
            >
              <span className="section__icon">{p.icon}</span>
              <span className="section__text">
                <span className="section__name">{p.label}</span>
                <span className="section__hint">{p.hint}</span>
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
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
