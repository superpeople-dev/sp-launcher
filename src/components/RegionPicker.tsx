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

/**
 * Next to Play: the region the player's matches are in (account_region in lib.rs). Every match is in one
 * region, so there is no "Any": only the regions with servers right now (Europe for now; more as they get
 * servers). Without a pick the backend answers Europe. Hidden until the backend answers, and when it has
 * no regions (an older backend, or the sign-in pass was refused: Play says why). Dev shows last, and only
 * for a player the backend lists it for (read once, when the launcher starts or reloads).
 */
export function RegionPicker({ onError }: Props) {
  const [state, setState] = useState<GameRegion | null>(null);
  const [saving, setSaving] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    invoke<GameRegion>("account_region")
      .then((got) => live && setState(got))
      .catch(() => live && setState(null));
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
      setState(await invoke<GameRegion>("account_region", { region }));
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
