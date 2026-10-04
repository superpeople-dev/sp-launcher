import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Icon } from "./community/Icon";

/** Mirrors `leaderboard::Row`. */
interface Row {
  rank: number;
  name: string;
  rp: number;
  tier: number;
  country: string | null;
  flag: string | null;
}

/** Mirrors `leaderboard::Board`: lists by "solo_tpp" .. "squad_fpp". */
interface Board {
  updated: number;
  lists: Record<string, Row[]>;
}

// Each shows the icon of the same name (community/Icon.tsx).
const MODES = [
  { id: "solo", label: "Solo" },
  { id: "duo", label: "Duo" },
  { id: "trio", label: "Trio" },
  { id: "squad", label: "Squad" },
] as const;
const VIEWS = [
  { id: "tpp", label: "TPP" },
  { id: "fpp", label: "FPP" },
] as const;
type Mode = (typeof MODES)[number]["id"];
type View = (typeof VIEWS)[number]["id"];

// The last board read, kept on this PC, shows at once each time the page opens (it used to start empty
// and wait for the backend every time). It is read again behind it once it is 5 minutes old, while the
// page is open, or on Refresh; what comes back is shown and kept in its place. The backend itself
// refreshes the lists every minute.
const REFRESH_MS = 5 * 60_000;
// After a read that failed, the next one is tried a minute later.
const RETRY_MS = 60_000;
const KEY = "sp.leaderboard";

interface Kept {
  board: Board;
  at: number;
}
let kept: Kept | null = (() => {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "null");
    return saved && typeof saved.at === "number" && saved.board && typeof saved.board.lists === "object" ? saved : null;
  } catch {
    return null;
  }
})();
function keep(board: Board | null) {
  kept = board ? { board, at: Date.now() } : null;
  try {
    if (kept) localStorage.setItem(KEY, JSON.stringify(kept));
    else localStorage.removeItem(KEY);
  } catch {
    /* no storage: only this run keeps it */
  }
}

// One read at a time, whoever asks (the timer, Refresh, the page opening again).
let reading: Promise<Board | null> | null = null;
let triedAt = 0;
function read(): Promise<Board | null> {
  if (!reading) {
    triedAt = Date.now();
    reading = invoke<Board | null>("leaderboard").finally(() => (reading = null));
  }
  return reading;
}

function ago(ms: number): string {
  const min = Math.floor(ms / 60_000);
  if (min < 1) return "Updated just now";
  if (min < 60) return `Updated ${min} min ago`;
  const h = Math.floor(min / 60);
  return h < 48 ? `Updated ${h} h ago` : `Updated ${Math.floor(h / 24)} days ago`;
}

// The game's tier ids (sp-backend lib/tables.js): 420100001 Super Soldier .. 420100004 Master, then
// Diamond I (420100005) down to Iron V (420100034). The colours are the website's.
const LADDER: Record<number, [string, string]> = {
  420100001: ["Super Soldier", "superSoldier"],
  420100002: ["Legendary", "legendary"],
  420100003: ["Grand Master", "grandMaster"],
  420100004: ["Master", "master"],
};
const GROUPS: [string, string][] = [
  ["Diamond", "diamond"],
  ["Platinum", "platinum"],
  ["Gold", "gold"],
  ["Silver", "silver"],
  ["Bronze", "bronze"],
  ["Iron", "iron"],
];
const STEPS = ["I", "II", "III", "IV", "V"];

function tierOf(id: number): { label: string; group: string } | null {
  if (LADDER[id]) return { label: LADDER[id][0], group: LADDER[id][1] };
  const offset = id - 420100005;
  if (!Number.isInteger(offset) || offset < 0 || offset >= GROUPS.length * STEPS.length) return null;
  const [name, group] = GROUPS[Math.floor(offset / STEPS.length)];
  return { label: `${name} ${STEPS[offset % STEPS.length]}`, group };
}

// Names match with or without accents and capitals ("lea" finds "Léa"), as on the website.
const plain = (text: string) => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

const regions = (() => {
  try {
    return new Intl.DisplayNames(["en"], { type: "region" });
  } catch {
    return null;
  }
})();
const countryName = (code: string) => {
  try {
    return regions?.of(code) ?? code;
  } catch {
    return code;
  }
};

/**
 * The Leaderboard page: each mode's top 100 of the season (leaderboard.rs), the mode and view picked
 * above the list, and a search by name that stays when the list changes. Opens on Solo TPP and stays
 * there until the player picks another list (it used to jump to the busiest list once loaded). Players
 * found keep their rank. The board kept from last time shows at once; Refresh, top right, reads it again.
 */
export function LeaderboardPanel() {
  const [board, setBoard] = useState<Board | null | undefined>(kept?.board);
  const [at, setAt] = useState(kept?.at ?? 0);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState("");
  const [now, setNow] = useState(() => Date.now());
  const [key, setKey] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  const load = useCallback(async () => {
    setBusy(true);
    try {
      const got = await read();
      // None: the backend has no public leaderboard (switched off), so the kept one goes too.
      keep(got);
      setBoard(got);
      setAt(kept?.at ?? 0);
      setFailed("");
    } catch (e) {
      setFailed(String(e));
    } finally {
      setBusy(false);
      setNow(Date.now());
    }
  }, []);

  useEffect(() => {
    const tick = () => {
      const t = Date.now();
      setNow(t);
      if ((!kept || t - kept.at >= REFRESH_MS) && t - triedAt >= RETRY_MS) void load();
    };
    // A read already under way when the page opens again (left and back quickly) is waited for.
    if (reading) void load();
    else tick();
    const timer = window.setInterval(tick, 15_000);
    return () => window.clearInterval(timer);
  }, [load]);

  const current = key ?? "solo_tpp";
  const [mode, view] = current.split("_") as [Mode, View];
  const list = board?.lists[current] ?? [];
  const wanted = plain(query.trim());
  const rows = wanted ? list.filter((r) => plain(r.name).includes(wanted)) : list;

  const empty =
    board === undefined
      ? failed || null
      : board === null
        ? "The leaderboard can't be loaded right now. Try again in a minute."
        : list.length === 0
          ? "Nobody is ranked in this mode yet this season."
          : rows.length === 0
            ? `No player called "${query.trim()}" in this list.`
            : null;

  return (
    <section className="panel leaders is-active">
      <header className="done__head leaders__head">
        <h2 className="done__title">Leaderboard</h2>
        <p className="done__lead">The season's top 100 of each mode.</p>
        <div className="leaders__tools">
          {board && (
            <span className={`leaders__when${failed ? " is-failed" : ""}`} title={failed || undefined}>
              {failed ? "Couldn't refresh" : ago(now - at)}
            </span>
          )}
          <button type="button" className={`btn leaders__refresh${busy ? " is-busy" : ""}`} aria-busy={busy} onClick={() => void load()}>
            <Icon name="refresh" />
            Refresh
          </button>
        </div>
      </header>

      <div className="leaders__bar">
        <div className="seg" role="group" aria-label="Mode">
          {MODES.map((m) => (
            <button key={m.id} type="button" className={`seg__btn${m.id === mode ? " is-on" : ""}`} aria-pressed={m.id === mode} onClick={() => setKey(`${m.id}_${view}`)}>
              <Icon name={m.id} />
              {m.label}
            </button>
          ))}
        </div>
        <div className="seg" role="group" aria-label="View">
          {VIEWS.map((v) => (
            <button key={v.id} type="button" className={`seg__btn${v.id === view ? " is-on" : ""}`} aria-pressed={v.id === view} onClick={() => setKey(`${mode}_${v.id}`)}>
              <Icon name={v.id} />
              {v.label}
            </button>
          ))}
        </div>
        <label className="leaders__search">
          <svg viewBox="0 0 16 16" aria-hidden>
            <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
            <path d="M10.5 10.5L14 14" stroke="currentColor" strokeWidth="1.5" />
          </svg>
          <input className="input" type="search" value={query} placeholder="Search a player" aria-label="Search a player" onChange={(e) => setQuery(e.target.value)} />
        </label>
      </div>

      {board === undefined && !failed ? (
        <ol className="leaders__list" aria-busy>
          {Array.from({ length: 8 }, (_, i) => (
            <li key={i} className="leader leader--ghost" />
          ))}
        </ol>
      ) : empty ? (
        <div className="leaders__empty">
          <p className="done__empty">{empty}</p>
          {failed && (
            <button type="button" className="btn" onClick={() => void load()}>
              Try again
            </button>
          )}
        </div>
      ) : (
        <ol className="leaders__list">
          {rows.map((r) => {
            const tier = tierOf(r.tier);
            return (
              <li key={r.rank} className={`leader${r.rank <= 3 ? ` is-top is-top-${r.rank}` : ""}`}>
                <span className="leader__rank">{r.rank}</span>
                <span className="leader__name">
                  {r.flag && r.country && (
                    <img className="leader__flag" src={r.flag} alt={countryName(r.country)} title={countryName(r.country)} width={21} height={14} draggable={false} />
                  )}
                  <bdi>{r.name}</bdi>
                </span>
                <span className={`leader__tier${tier ? ` tier--${tier.group}` : ""}`}>{tier?.label ?? ""}</span>
                <span className="leader__rp">
                  {r.rp.toLocaleString("en-US")} <small>RP</small>
                </span>
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}
