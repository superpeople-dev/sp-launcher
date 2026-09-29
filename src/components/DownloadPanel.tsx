import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { pickInstallFolder } from "../lib/browse";
import { bytes, rate } from "../lib/format";
import type { DownloadStatus } from "../types";

/**
 * The Download tab. All real work happens in src-tauri/src/download.rs; this
 * only draws `download:status` events and sends start / pause / cancel.
 * The worker keeps running when the tab (or the window) is closed -- the
 * launcher just hides to the tray.
 */

interface Props {
  installed: boolean;
  installDir: string;
  /** The one Game folder setting (config.install_dir), shared with Settings. */
  onFolder: (dir: string) => void;
  onError: (message: string) => void;
  /** Called once the game is unpacked and the install folder was set. */
  onInstalled: () => void;
}

const STEP_LABEL: Record<DownloadStatus["phase"], string> = {
  idle: "Ready",
  checking: "Checking",
  downloading: "Downloading",
  paused: "Paused",
  verifying: "Verifying",
  extracting: "Unpacking",
  done: "Installed",
  failed: "Stopped",
};

// The four steps shown as a checklist under the bar.
const STEPS: { phase: DownloadStatus["phase"]; name: string }[] = [
  { phase: "downloading", name: "Download (27.7 GB from archive.org)" },
  { phase: "verifying", name: "Verify checksum" },
  { phase: "extracting", name: "Unpack" },
  { phase: "done", name: "Ready to play" },
];
const ORDER: DownloadStatus["phase"][] = ["idle", "checking", "downloading", "verifying", "extracting", "done"];

function duration(secs: number | null): string {
  if (secs == null || !Number.isFinite(secs)) return "";
  if (secs < 90) return `${Math.ceil(secs)} s left`;
  const m = Math.ceil(secs / 60);
  if (m < 90) return `${m} min left`;
  const h = Math.floor(m / 60);
  return `${h} h ${m % 60} min left`;
}

export function DownloadPanel({ installed, installDir, onFolder, onError, onInstalled }: Props) {
  const [st, setSt] = useState<DownloadStatus | null>(null);
  const [dir, setDir] = useState("");
  const [lastActive, setLastActive] = useState<DownloadStatus["phase"]>("idle");

  useEffect(() => {
    void invoke<DownloadStatus>("download_status").then(async (s) => {
      setSt(s);
      if (s.dir) setDir(s.dir);
      else setDir(await invoke<string>("download_default_dir"));
    });
    const offs = [
      listen<DownloadStatus>("download:status", (e) => {
        setSt(e.payload);
        if (e.payload.dir) setDir(e.payload.dir);
      }),
      listen<string>("download:installed", () => onInstalled()),
    ];
    return () => offs.forEach((p) => void p.then((off) => off()));
  }, [onInstalled]);

  // Remember which step a pause/failure interrupted, for the checklist.
  useEffect(() => {
    if (st && ORDER.includes(st.phase)) setLastActive(st.phase);
  }, [st]);

  const active = st != null && ["checking", "downloading", "verifying", "extracting"].includes(st.phase);
  const resumable = st != null && (st.phase === "paused" || (st.phase === "failed" && st.done > 0));
  // The game already installed: nothing to download, and no Download button.
  const ready = installed && !active && !resumable;

  // The installed game's folder is the one shown.
  useEffect(() => {
    if (ready && installDir) setDir(installDir);
  }, [ready, installDir]);

  // A folder that already has the game (a copy the player had, or an earlier
  // install) becomes the Game folder at once, instead of offering a download.
  const onFolderRef = useRef(onFolder);
  useEffect(() => {
    onFolderRef.current = onFolder;
  }, [onFolder]);
  useEffect(() => {
    const target = dir.trim();
    if (installed || active || resumable || !target) return;
    const t = window.setTimeout(() => {
      void invoke<string | null>("find_game", { dir: target }).then((found) => found && onFolderRef.current(found));
    }, 400);
    return () => window.clearTimeout(t);
  }, [dir, installed, active, resumable]);

  if (!st) return <section className="panel dl is-active" />;
  const pct = st.total > 0 ? Math.min(100, (st.done / st.total) * 100) : 0;
  const lowSpace = st.free_bytes != null && st.needed_bytes != null && st.free_bytes < st.needed_bytes;

  const start = () => void invoke("download_start", { dir }).catch((e) => onError(String(e)));
  const pause = () => void invoke("download_pause").catch((e) => onError(String(e)));
  const cancel = async () => {
    const sure = await ask("Stop and delete the partial download? The progress will be lost.", {
      title: "Cancel download",
      kind: "warning",
    });
    if (!sure) return;
    void invoke("download_cancel").catch((e) => onError(String(e)));
  };
  // Picking a folder here changes the Game folder itself. If the game is
  // already in it, the launcher is ready and nothing needs downloading.
  const browse = async () => {
    const picked = await pickInstallFolder();
    if (picked) {
      setDir(picked);
      onFolder(picked);
    }
  };

  const stepState = (phase: DownloadStatus["phase"]) => {
    const cur = st.phase === "paused" || st.phase === "failed" ? lastActive : st.phase;
    const a = ORDER.indexOf(phase);
    const b = ORDER.indexOf(cur);
    if (ready || st.phase === "done" || b > a) return { cls: "ok", text: "Done" };
    if (b === a) {
      if (st.phase === "paused") return { cls: "wait", text: "Paused" };
      if (st.phase === "failed") return { cls: "busy", text: "Stopped" };
      return { cls: "busy", text: "In progress" };
    }
    return { cls: "wait", text: "Waiting" };
  };

  return (
    <section className="panel dl is-active">
      <div className="card">
        <h2 className="card__title">Game folder</h2>
        {!installed && (
          <p className="field__hint" style={{ marginBottom: 10 }}>
            Already have the game? Browse to its folder (the one with BravoHotelClient.exe) and you're done.
            Otherwise pick where it should go and press Download.
          </p>
        )}
        <div className="field">
          <div className="field__row">
            <input
              className="input"
              value={dir}
              disabled={active || resumable}
              onChange={(e) => setDir(e.target.value)}
              onBlur={() => dir.trim() && dir.trim() !== installDir && onFolder(dir.trim())}
              spellCheck={false}
            />
            <button className="btn" type="button" disabled={active || resumable} onClick={() => void browse()}>
              Browse
            </button>
          </div>
          <span className={`field__hint${lowSpace ? " is-warn" : ""}`}>
            {st.free_bytes != null ? `${bytes(st.free_bytes)} free on this drive` : "Free space unknown"}
            {st.needed_bytes != null ? ` — about ${bytes(st.needed_bytes)} needed while installing` : " — about 64 GB needed while installing"}
            {". The download is deleted after unpacking."}
          </span>
        </div>
      </div>

      <div className="card">
        <h2 className="card__title">SUPER PEOPLE — Testing Grounds [S-DEV]</h2>

        <div className="bigprogress">
          <div className="bigprogress__head">
            <span className="bigprogress__pct">{ready ? "100%" : st.phase === "idle" ? "—" : `${pct.toFixed(1)}%`}</span>
            <span className="bigprogress__speed">
              {ready ? STEP_LABEL.done : STEP_LABEL[st.phase]}
              {!ready && st.speed > 0 && ` · ${rate(st.speed)}`}
              {!ready && st.eta_secs != null && ` · ${duration(st.eta_secs)}`}
            </span>
          </div>
          <div className="bigprogress__track">
            <div
              className={`bigprogress__fill${st.phase === "paused" ? " is-paused" : ""}${st.phase === "failed" ? " is-failed" : ""}`}
              style={{ width: `${ready ? 100 : pct}%` }}
            />
          </div>
          <div className="bigprogress__file">
            {ready
              ? `In ${installDir}`
              : `${st.total > 0 ? `${bytes(st.done)} of ${bytes(st.total)}` : ""}${
                  st.message ? `${st.total > 0 ? " — " : ""}${st.message}` : ""
                }`}
          </div>
        </div>

        <div className="filelist" style={{ marginTop: 12 }}>
          {STEPS.map((s) => {
            const state = stepState(s.phase);
            return (
              <div className="filerow" key={s.phase}>
                <span className="filerow__name">{s.name}</span>
                <span className={`filerow__state ${state.cls}`}>{state.text}</span>
              </div>
            );
          })}
        </div>

        <div className="field__row" style={{ marginTop: 14 }}>
          {active ? (
            <button className="btn btn--primary" type="button" onClick={pause}>
              Pause
            </button>
          ) : resumable ? (
            <button className="btn btn--primary" type="button" onClick={start}>
              Continue
            </button>
          ) : ready || st.phase === "done" ? null : (
            <button className="btn btn--primary" type="button" disabled={!dir.trim()} onClick={start}>
              {st.phase === "failed" ? "Try again" : "Download"}
            </button>
          )}
          {(active || resumable) && (
            <button className="btn" type="button" onClick={() => void cancel()}>
              Cancel
            </button>
          )}
        </div>
        <p className="field__hint" style={{ marginTop: 10 }}>
          {ready
            ? "Nothing to download: the game is already in this folder. To install it somewhere else, pick an empty folder above."
            : "You can close the launcher window while it downloads — it keeps going from the tray, and your PC won't go to sleep. If the launcher is quit or the PC restarts, press Continue to resume where it stopped."}
        </p>
      </div>
    </section>
  );
}
