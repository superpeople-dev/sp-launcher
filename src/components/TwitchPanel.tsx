import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

interface Props {
  onError: (message: string) => void;
}

/** Mirrors `twitch::Stream`. */
interface Stream {
  login: string;
  name: string;
  title: string;
  viewers: number;
  thumbnail: string;
  startedAt: string;
  language: string;
}

/** Mirrors `twitch::Streams`: streams is null when Twitch could not be reached. */
interface Streams {
  configured: boolean;
  streams: Stream[] | null;
  category: string;
}

// The website asks Twitch at most once a minute; so does the tab while it is open.
const REFRESH_MS = 60_000;

/** "1h 20m", "45m": how long a stream has been live. */
function liveFor(iso: string): string {
  const minutes = Math.floor((Date.now() - new Date(iso).getTime()) / 60_000);
  if (!Number.isFinite(minutes)) return "";
  const hours = Math.floor(Math.max(0, minutes) / 60);
  return hours ? `${hours}h ${minutes % 60}m` : `${Math.max(0, minutes)}m`;
}

const viewers = (n: number) => `${n.toLocaleString("en-US")} ${n === 1 ? "viewer" : "viewers"}`;

// Twitch's player only plays for the page it names as its parent: tauri.localhost in the launcher
// (served over https, tauri.conf.json useHttpsScheme), localhost in development.
const player = (login: string) =>
  `https://player.twitch.tv/?channel=${encodeURIComponent(login)}&parent=${encodeURIComponent(window.location.hostname)}&autoplay=true&muted=false`;

/**
 * The Twitch tab: the most popular SUPER PEOPLE streams (twitch.rs, through the website). The one
 * with the most viewers plays here; the others, most viewers first, are beside it and play instead
 * when picked. Leaving the tab stops it.
 */
export function TwitchPanel({ onError }: Props) {
  const [data, setData] = useState<Streams | null>(null);
  const [failed, setFailed] = useState("");
  // The stream picked in the list; the most watched one until then, or when it ends.
  const [picked, setPicked] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await invoke<Streams>("twitch_streams"));
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

  const open = (url: string) => openUrl(url).catch((err) => onError(`Could not open Twitch: ${String(err)}`));
  const streams = data?.streams ?? null;
  const playing = streams?.find((s) => s.login === picked) ?? streams?.[0] ?? null;

  const empty = !data
    ? failed || null
    : !data.configured
      ? "Twitch streams aren't set up yet."
      : streams === null
        ? "Twitch can't be reached right now. Try again in a minute."
        : streams.length === 0
          ? "Nobody is live in SUPER PEOPLE right now. Stream in the SUPER PEOPLE category on Twitch and you show up here."
          : null;

  return (
    <section className="panel twitch is-active">
      <header className="done__head twitch__head">
        <h2 className="done__title">Live on Twitch</h2>
        <p className="done__lead">The most popular SUPER PEOPLE streams.</p>
        <button
          type="button"
          className="btn twitch__all"
          onClick={() => void open(data?.category || "https://www.twitch.tv/directory/category/super-people")}
        >
          See all on Twitch
        </button>
      </header>

      {!data && !failed ? (
        <div className="twitch__body" aria-busy>
          <div className="twitch__main">
            <div className="twitch__player skel" />
          </div>
          <ul className="twitch__list">
            {Array.from({ length: 4 }, (_, i) => (
              <li key={i} className="chan chan--ghost" />
            ))}
          </ul>
        </div>
      ) : empty || !playing ? (
        <div className="twitch__empty">
          <p className="done__empty">{empty}</p>
          {failed && (
            <button type="button" className="btn" onClick={() => void load()}>
              Try again
            </button>
          )}
        </div>
      ) : (
        <div className="twitch__body">
          <div className="twitch__main">
            <iframe
              key={playing.login}
              className="twitch__player"
              src={player(playing.login)}
              title={`${playing.name} on Twitch`}
              allow="autoplay; fullscreen"
              allowFullScreen
            />
            <div className="twitch__now">
              <div className="twitch__who">
                <span className="twitch__name">{playing.name}</span>
                <span className="twitch__title" title={playing.title}>
                  {playing.title}
                </span>
              </div>
              <span className="twitch__stats">
                <span className="twitch__dot" aria-hidden />
                {viewers(playing.viewers)}
                {playing.startedAt && ` - ${liveFor(playing.startedAt)}`}
              </span>
              <button type="button" className="btn" onClick={() => void open(`https://www.twitch.tv/${playing.login}`)}>
                Open on Twitch
              </button>
            </div>
          </div>
          <ul className="twitch__list" aria-label="Live streams, most viewers first">
            {streams?.map((s, i) => (
              <li key={s.login}>
                <button
                  type="button"
                  className={`chan${s.login === playing.login ? " is-on" : ""}`}
                  aria-pressed={s.login === playing.login}
                  onClick={() => setPicked(s.login)}
                >
                  <span className="chan__shot">
                    {s.thumbnail && <img src={s.thumbnail} alt="" loading="lazy" draggable={false} />}
                    <span className="chan__rank">{i + 1}</span>
                  </span>
                  <span className="chan__text">
                    <span className="chan__name">{s.name}</span>
                    <span className="chan__viewers">{viewers(s.viewers)}</span>
                    <span className="chan__title">{s.title}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
