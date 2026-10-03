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
  // The channel and the stream's details; a website from before them sends none.
  avatar?: string;
  bio?: string;
  badge?: "partner" | "affiliate" | "";
  since?: string;
  tags?: string[];
  mature?: boolean;
  clips?: Clip[];
}

/** Mirrors `twitch::Clip`: one of the channel's most watched SUPER PEOPLE clips. */
interface Clip {
  title: string;
  url: string;
  thumbnail: string;
  views: number;
  seconds: number;
  createdAt: string;
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

// "pt" -> "Portuguese"; Twitch's "other" (and anything unknown) shows nothing.
const languageNames = new Intl.DisplayNames(["en"], { type: "language" });
function languageName(code: string): string {
  if (!code || code === "other") return "";
  try {
    return languageNames.of(code) ?? "";
  } catch {
    return "";
  }
}

/** "On Twitch since 2019", or nothing. */
function onTwitchSince(iso?: string): string {
  const year = iso ? new Date(iso).getUTCFullYear() : NaN;
  return Number.isFinite(year) ? `On Twitch since ${year}` : "";
}

/** "0:42", "1:05": a clip's length. */
const clipLength = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.round(seconds % 60)).padStart(2, "0")}`;

const badges = { partner: "Partner", affiliate: "Affiliate" } as const;

// Twitch's player only plays for the page it names as its parent: tauri.localhost in the launcher
// (served over https, tauri.conf.json useHttpsScheme), localhost in development.
const player = (login: string) =>
  `https://player.twitch.tv/?channel=${encodeURIComponent(login)}&parent=${encodeURIComponent(window.location.hostname)}&autoplay=true&muted=false`;

/**
 * Under the player: the channel's bio, its language and years on Twitch, the stream's tags, and the
 * channel's most watched SUPER PEOPLE clips (opened on Twitch). Scrolls when it is longer than the
 * room left under the player; nothing shows for a website from before these details.
 */
function About({ stream, onOpen }: { stream: Stream; onOpen: (url: string) => void }) {
  const language = languageName(stream.language);
  const facts = [language, onTwitchSince(stream.since), stream.mature ? "Mature audiences" : ""].filter(Boolean);
  // Twitch streams often tag their language too: shown once.
  const tags = (stream.tags ?? []).filter((tag) => tag.toLowerCase() !== language.toLowerCase());
  const clips = stream.clips ?? [];
  if (!stream.bio && !facts.length && !tags.length && !clips.length) return null;
  return (
    <div className="twitch__about">
      {stream.bio && <p className="twitch__bio">{stream.bio}</p>}
      {(facts.length > 0 || tags.length > 0) && (
        <ul className="twitch__facts" aria-label="About this stream">
          {facts.map((fact) => (
            <li key={fact} className="twitch__fact">
              {fact}
            </li>
          ))}
          {tags.map((tag) => (
            <li key={`tag-${tag}`} className="twitch__tag">
              {tag}
            </li>
          ))}
        </ul>
      )}
      {clips.length > 0 && (
        <section className="twitch__clips" aria-label={`Top SUPER PEOPLE clips from ${stream.name}`}>
          <h3 className="twitch__sub">Top clips</h3>
          <ul className="twitch__cliplist">
            {clips.map((clip) => (
              <li key={clip.url}>
                <button type="button" className="clip" title={clip.title} onClick={() => onOpen(clip.url)}>
                  <span className="clip__shot">
                    {clip.thumbnail && <img src={clip.thumbnail} alt="" loading="lazy" draggable={false} />}
                    <span className="clip__len">{clipLength(clip.seconds)}</span>
                  </span>
                  <span className="clip__title">{clip.title}</span>
                  <span className="clip__views">{`${clip.views.toLocaleString("en-US")} ${clip.views === 1 ? "view" : "views"}`}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

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
              {playing.avatar && <img className="twitch__avatar" src={playing.avatar} alt="" draggable={false} />}
              <div className="twitch__who">
                <span className="twitch__name">
                  {playing.name}
                  {playing.badge && <span className={`twitch__badge twitch__badge--${playing.badge}`}>{badges[playing.badge]}</span>}
                </span>
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
            <About stream={playing} onOpen={(url) => void open(url)} />
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
