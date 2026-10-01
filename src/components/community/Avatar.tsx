import { useEffect, useState, type SyntheticEvent } from "react";
import type { Person } from "../../types";

/** Discord's own default picture (its logo on a colour), chosen from the user id the way Discord
 * does, as on the website (sp-website components/Avatar.tsx). The id is the person's own when
 * known (the signed-in profile), else the one in the picture's address
 * (cdn.discordapp.com/avatars/<id>/…); without either, the first colour. */
function defaultAvatar(src: string | null | undefined, id?: string | null): string {
  const from = (id && /^\d+$/.test(id) ? id : null) || src?.match(/\/avatars\/(\d+)\//)?.[1];
  let index = 0;
  try {
    if (from) index = Number((BigInt(from) >> BigInt(22)) % BigInt(6));
  } catch {
    index = 0;
  }
  return `https://cdn.discordapp.com/embed/avatars/${index}.png`;
}

/** Discord's CDN lets a page read its pictures, so a fully transparent one (some people set
 * that) can be told apart once loaded; other hosts are drawn as they are. */
const readable = (src: string) => /^https:\/\/cdn\.discordapp\.com\//.test(src);

function isBlank(img: HTMLImageElement): boolean {
  try {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 16;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) return false;
    context.drawImage(img, 0, 0, 16, 16);
    const { data } = context.getImageData(0, 0, 16, 16);
    for (let i = 3; i < data.length; i += 4) if (data[i] > 8) return false;
    return true;
  } catch {
    return false;
  }
}

/** A Discord avatar. With none, a fully transparent one, or one that does not load, Discord's
 * default picture, as on the website; the name's initial on a colour picked from the name only
 * when even that cannot load (offline). */
export function Avatar({ person, size = 24 }: { person: (Person & { id?: string | null }) | null; size?: number }) {
  const src = person?.avatar || null;
  const [fallback, setFallback] = useState(!src);
  const [failed, setFailed] = useState(false);
  // Another person in the same place (a list that changed): start again from their picture.
  useEffect(() => {
    setFallback(!src);
    setFailed(false);
  }, [src]);

  const name = person?.name || "?";
  const style = { width: size, height: size, fontSize: Math.round(size * 0.46) };
  if (!failed) {
    const shown = fallback || !src ? defaultAvatar(src, person?.id) : src;
    const onLoad = (event: SyntheticEvent<HTMLImageElement>) => {
      if (!fallback && readable(shown) && isBlank(event.currentTarget)) setFallback(true);
    };
    return (
      <img
        className="avatar"
        src={shown}
        alt=""
        style={style}
        draggable={false}
        crossOrigin={readable(shown) ? "anonymous" : undefined}
        onLoad={onLoad}
        onError={() => (fallback ? setFailed(true) : setFallback(true))}
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
