import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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

// The backend refreshes the lists every minute; so does the page while it is open.
const REFRESH_MS = 60_000;

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
 * found keep their rank.
 */
export function LeaderboardPanel() {
  const [board, setBoard] = useState<Board | null | undefined>(undefined);
  const [failed, setFailed] = useState("");
  const [key, setKey] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  const load = useCallback(async () => {
    try {
      setBoard(await invoke<Board | null>("leaderboard"));
      setFailed("");
    } catch (e) {
      setFailed(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
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
      </header>

      <div className="leaders__bar">
        <div className="seg" role="group" aria-label="Mode">
          {MODES.map((m) => (
            <button key={m.id} type="button" className={`seg__btn${m.id === mode ? " is-on" : ""}`} aria-pressed={m.id === mode} onClick={() => setKey(`${m.id}_${view}`)}>
              {m.label}
            </button>
          ))}
        </div>
        <div className="seg" role="group" aria-label="View">
          {VIEWS.map((v) => (
            <button key={v.id} type="button" className={`seg__btn${v.id === view ? " is-on" : ""}`} aria-pressed={v.id === view} onClick={() => setKey(`${mode}_${v.id}`)}>
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
