import { useState } from "react";
import type { Person } from "../../types";

/** A Discord avatar, or the name's initial on a colour picked from the name
 * when there is no picture (or it fails to load). */
export function Avatar({ person, size = 24 }: { person: Person | null; size?: number }) {
  const [broken, setBroken] = useState(false);
  const name = person?.name || "?";
  const style = { width: size, height: size, fontSize: Math.round(size * 0.46) };
  if (person?.avatar && !broken) {
    return (
      <img
        className="avatar"
        src={person.avatar}
        alt=""
        style={style}
        draggable={false}
        onError={() => setBroken(true)}
      />
    );
  }
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return (
    <span className="avatar avatar--letter" style={{ ...style, background: `hsl(${hash % 360} 38% 32%)` }} aria-hidden>
      {name.charAt(0).toUpperCase()}
    </span>
  );
}
