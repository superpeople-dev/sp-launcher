import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  onError: (message: string) => void;
}

/** Mirrors `auth::GameRegion`: region is where the player's matches are (one of regions), regions counts servers per region. */
interface GameRegion {
  region: string;
  regions: Record<string, number>;
}

// The backend's regions (sp-backend lib/regions.js), in the order they show. dev is the Dev region:
// private servers the backend lists only for the staff and the players the owner invited, so it is
// never there for anyone else.
const NAMES: Record<string, string> = {
  europe: "Europe",
  asia: "Asia",
  northAmerica: "North America",
  southAmerica: "South America",
  oceania: "Oceania",
  africa: "Africa",
  dev: "Dev",
};

const servers = (n: number) => (n === 1 ? "1 server" : `${n} servers`);

// The backend's last answer, so the picker is there at once, as it was, when the Play tab shows again
// or the launcher starts (and refreshed behind it), instead of appearing a moment later. Kept on this
// PC; forgotten on sign-out (forgetRegion), as it is the account's.
const KEY = "sp.region";
let last: GameRegion | null = (() => {
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? "null");
    return saved && typeof saved.region === "string" && saved.regions && typeof saved.regions === "object" ? saved : null;
  } catch {
    return null;
  }
})();
function remember(got: GameRegion | null) {
  last = got;
  try {
    if (got) localStorage.setItem(KEY, JSON.stringify(got));
    else localStorage.removeItem(KEY);
  } catch {
    /* no storage: only this run remembers */
  }
}
export const forgetRegion = () => remember(null);

/**
 * Next to Play: the region the player's matches are in (account_region in lib.rs). Every match is in one
 * region, so there is no "Any": only the regions with servers right now. Without a pick the backend
 * answers the region nearest the player that has servers. Shows the last answer at once and asks again
 * behind it; hidden only before the first answer ever, and when there are no regions (an older backend).
 * A failed refresh keeps what was shown. Dev shows last, and only for a player the backend lists it for.
 */
export function RegionPicker({ onError }: Props) {
  const [state, setState] = useState<GameRegion | null>(last);
  const [saving, setSaving] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    invoke<GameRegion>("account_region")
      .then((got) => {
        remember(got);
        if (live) setState(got);
      })
      .catch(() => {
        /* offline, or the pass was refused (Play says why): keep the last answer */
      });
    return () => {
      live = false;
    };
  }, []);

  if (!state) return null;
  const offered = Object.keys(NAMES).filter((id) => (state.regions[id] ?? 0) > 0);
  if (!offered.length) return null;

  const pick = async (region: string) => {
    if (region === state.region || saving) return;
    setSaving(region);
    try {
      const got = await invoke<GameRegion>("account_region", { region });
      remember(got);
      setState(got);
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(null);
    }
  };

  const options = offered.map((id) => ({
    id,
    label: NAMES[id],
    title:
      id === "dev"
        ? "Staff and invited players only"
        : `Matches on servers in ${NAMES[id]} (${servers(state.regions[id])} now). In a party, the leader's region counts.`,
  }));

  return (
    <div className="region">
      <span className="region__label" id="region-label">
        Region
      </span>
      <div className="seg" role="group" aria-labelledby="region-label">
        {options.map((o) => (
          <button
            key={o.id}
            type="button"
            className={`seg__btn${state.region === o.id ? " is-on" : ""}`}
            aria-pressed={state.region === o.id}
            title={o.title}
            disabled={saving !== null}
            onClick={() => void pick(o.id)}
          >
            {saving === o.id ? "..." : o.label}
          </button>
        ))}
      </div>
    </div>
  );
}
