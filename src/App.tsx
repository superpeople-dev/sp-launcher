import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Update } from "@tauri-apps/plugin-updater";

import { TitleBar } from "./components/TitleBar";
import { PlayPanel } from "./components/PlayPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { DownloadPanel } from "./components/DownloadPanel";
import { Welcome } from "./components/Welcome";
import { IdeasPanel } from "./components/community/IdeasPanel";
import { RoadmapPanel } from "./components/community/RoadmapPanel";
import { CompletedPanel } from "./components/community/CompletedPanel";
import { activeNews } from "./news";
import { checkForUpdate, installUpdate } from "./lib/updater";
import { clearCommunityCache, preloadBoards } from "./lib/community";
import type { AuthState, Config, HostsStatus, InstallState, NewsItem, Phase, Profile, Tab } from "./types";

// Re-check which items are in their [starts_at, ends_at) window every so
// often, so an event that just started (or just ended) updates without the
// user having to reopen the launcher.
const NEWS_REFRESH_MS = 10 * 60 * 1000;

export default function App() {
  const [tab, setTab] = useState<Tab>("play");
  const [config, setConfig] = useState<Config | null>(null);
  const [install, setInstall] = useState<InstallState>({ installed: false, exe_path: null });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hosts, setHosts] = useState<HostsStatus | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [news, setNews] = useState<NewsItem[]>([]);

  // The player's Discord account. `undefined` until the Rust side answers, so
  // a signed-in player never sees the welcome screen flash by; `null` means
  // not connected, and the welcome screen is all there is.
  const [profile, setProfile] = useState<Profile | null | undefined>(undefined);
  const [connecting, setConnecting] = useState(false);
  const [authError, setAuthError] = useState<string | null>(null);

  const [appVersion, setAppVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateChecked, setUpdateChecked] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<{ done: number; total: number | null } | null>(null);
  const [updateError, setUpdateError] = useState<string | null>(null);

  // Debounce config writes: the settings fields fire on every keystroke and
  // there is no reason to hit the disk that often.
  const saveTimer = useRef<number | null>(null);

  useEffect(() => {
    void (async () => {
      // Nothing below may throw uncaught: `config` staying null renders an
      // empty window forever, which looks exactly like the app not starting.
      let cfg: Config;
      try {
        cfg = await invoke<Config>("get_config");
      } catch (e) {
        setError(`Could not load settings: ${String(e)}`);
        return;
      }

      // Read from this PC only (no network), so it answers at once. Failing
      // falls back to the welcome screen rather than an empty window.
      void invoke<AuthState>("auth_status")
        .then((a) => setProfile(a.profile))
        .catch(() => setProfile(null));

      // No folder picker on first start any more: the Play tab's "Get the game"
      // leads to the Download tab, which holds the one Game folder setting
      // (download there, or browse to an existing install).
      setConfig(cfg);
      await invoke<InstallState>("install_state").then(setInstall).catch(() => {});
      await invoke<HostsStatus>("hosts_status").then(setHosts).catch(() => {});
    })();
  }, []);

  useEffect(() => {
    // A broken, unset, or empty news feed should never block the rest of the
    // UI — just show nothing rather than a placeholder.
    const refresh = () =>
      void invoke<NewsItem[]>("fetch_news")
        .then((items) => setNews(items))
        .catch(() => setNews([]));
    refresh();
    const id = window.setInterval(refresh, NEWS_REFRESH_MS);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    void getVersion().then(setAppVersion);
  }, []);

  // Ideas, Roadmap and Completed load as soon as the player is signed in, so
  // their tabs open at once. They carry that player's own votes: signing out
  // drops them.
  const profileId = profile?.id;
  useEffect(() => {
    if (profileId) preloadBoards();
    else clearCommunityCache();
  }, [profileId]);

  // The launcher opens in front of the other windows. After an update the
  // installer starts it from the background, and Windows would otherwise leave
  // it behind whatever the player had open. Opened by hand it already is in
  // front, and this does nothing.
  useEffect(() => {
    void getCurrentWindow().setFocus().catch(() => {});
  }, []);

  // `quiet` suppresses the error toast for the automatic startup check — a
  // launcher that can't reach its update server should still open normally.
  // A check the user asked for always reports what went wrong, so a dead
  // endpoint can't masquerade as "up to date".
  const lastUpdateCheck = useRef(0);
  const installingRef = useRef(false);
  const runUpdateCheck = useCallback((quiet = false) => {
    lastUpdateCheck.current = Date.now();
    setCheckingUpdate(true);
    setUpdateError(null);
    void checkForUpdate()
      .then((u) => {
        setUpdate(u);
        setUpdateChecked(true);
      })
      .catch((e) => {
        setUpdateError(String(e));
        if (!quiet) setError(`Update check failed: ${String(e)}`);
      })
      .finally(() => setCheckingUpdate(false));
  }, []);

  useEffect(() => {
    runUpdateCheck(true);
  }, [runUpdateCheck]);

  // Coming back to the launcher (clicking it, or opening it from the tray)
  // checks again, quietly: one left open in the tray for days would otherwise
  // only hear of an update when restarted. It also re-reads the game folder,
  // in case the game was moved or removed meanwhile. At most once a minute,
  // and never while an update is installing.
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (!focused || installingRef.current || Date.now() - lastUpdateCheck.current < 60_000) return;
        runUpdateCheck(true);
        void invoke<InstallState>("install_state").then(setInstall).catch(() => {});
      })
      .then((unlisten) => {
        if (gone) unlisten();
        else off = unlisten;
      });
    return () => {
      gone = true;
      off?.();
    };
  }, [runUpdateCheck]);

  const runUpdateInstall = useCallback(() => {
    if (!update) return;
    installingRef.current = true;
    setInstallingUpdate(true);
    setUpdateProgress(null);
    void installUpdate(update, (done, total) => setUpdateProgress({ done, total })).catch((e) => {
      // A successful run typically exits the process itself (see
      // lib/updater.ts) before this ever runs — only a genuine failure
      // reaches here.
      installingRef.current = false;
      setInstallingUpdate(false);
      setError(`Update failed: ${String(e)}`);
    });
  }, [update]);

  useEffect(() => {
    const unlisten: Promise<() => void>[] = [
      // The game exited: re-read the hosts state (the entries stay in place).
      listen<number | null>("game:exited", () => {
        setBusy(false);
        void invoke<HostsStatus>("hosts_status").then(setHosts);
      }),
      listen<string>("hosts:recovered", (e) => setNotice(e.payload)),
      listen<string>("hosts:error", (e) => setError(e.payload)),
      listen<string>("game:cleanup-failed", (e) => setError(`Client fixes cleanup failed: ${e.payload}`)),
      // The website stopped accepting the saved sign-in (lib.rs `expired`):
      // back to the welcome screen, saying why.
      listen("auth:expired", () => {
        setProfile(null);
        setError(null);
        setAuthError("Your Discord sign-in has expired. Connect again to continue.");
      }),
    ];
    return () => {
      unlisten.forEach((p) => void p.then((off) => off()));
    };
  }, []);

  const patchConfig = useCallback((patch: Partial<Config>) => {
    setConfig((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => {
        void invoke("set_config", { cfg: next })
          .then(() => invoke<InstallState>("install_state").then(setInstall))
          .catch((e) => setError(String(e)));
      }, 250);
      return next;
    });
  }, []);

  // Only relevant while not installed: the Download tab fetches the game (or
  // lets the player point at a folder that already has it, via Settings).
  // Actually starting the game is `onLaunch` below.
  const onPrimary = useCallback(() => {
    setTab("download");
  }, []);

  // The Download tab finished: the Rust side already wrote the new install
  // folder into the config, so pull it back (and drop any pending debounced
  // write that still carries the old folder).
  const onInstalled = useCallback(() => {
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    void invoke<Config>("get_config").then(setConfig);
    void invoke<InstallState>("install_state").then(setInstall);
    setNotice("The game is installed and ready to play.");
  }, []);

  // Uninstall deleted the game; the Game folder setting is unchanged.
  const onUninstalled = useCallback(() => {
    void invoke<InstallState>("install_state").then(setInstall);
    setNotice("The game was uninstalled.");
  }, []);

  // One button, no address. Joining a specific listen server by ip:port used to
  // be asked here; the lobby queue does that now, so the prompt was two extra
  // decisions on the way to playing. The backend side is untouched --
  // `launch_game` still takes an address and passes it as the first argument --
  // so bringing the prompt back is a UI change only.
  const onLaunch = useCallback(() => {
    if (!config) return;
    setBusy(true);
    setError(null);
    // Settings are normally debounced. Flush them before launch so a toggle
    // changed immediately before Play controls this run, not the next one.
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = null;
    void invoke("set_config", { cfg: config })
      .then(() => invoke("launch_game", { server: null }))
      .then(() => invoke<HostsStatus>("hosts_status").then(setHosts))
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
    // `busy` is cleared by the game:exited event, not here: the launcher
    // stays in the launched state for as long as the game is up.
  }, [config]);

  const stopGame = useCallback(() => {
    void invoke("stop_game").catch((e) => setError(String(e)));
    // Same as onPrimary: `busy` clears on the game:exited event once the
    // process actually dies, not here.
  }, []);

  const refreshHosts = useCallback(() => {
    void invoke<HostsStatus>("hosts_status").then(setHosts).catch((e) => setError(String(e)));
  }, []);

  if (!config) {
    return (
      <div className="app">
        <div className="bg" />
        {error && (
          <div className="toast" role="alert">
            {error}
          </div>
        )}
      </div>
    );
  }

  const phase: Phase = install.installed ? "ready" : "not-installed";

  // Opens the Discord window and waits until the player is through (or closes
  // it). The Rust side keeps the session; the profile is all the UI needs.
  const connect = async () => {
    setConnecting(true);
    setAuthError(null);
    try {
      setProfile(await invoke<Profile>("discord_connect"));
      setTab("play");
    } catch (e) {
      const message = String(e);
      if (message !== "cancelled") setAuthError(message);
    } finally {
      setConnecting(false);
    }
  };

  const signOut = async () => {
    try {
      await invoke("sign_out");
      setProfile(null);
      setAuthError(null);
      setTab("play");
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="app">
      <div className="bg" />
      <div className="bg__slabs">
        <span className="slab slab--mint" />
        <span className="slab slab--coral" />
        <span className="slab slab--coral-2" />
        <span className="slab slab--sliver" />
      </div>
      <div className="bg__grade" />
      <div className="bg__vignette" />
      <div className="bg__grain" />

      <TitleBar tab={tab} onTab={setTab} profile={profile ?? null} onSignOut={() => void signOut()} />

      <main className="stage">
        {profile === null && (
          <Welcome
            waiting={connecting}
            error={authError}
            onConnect={() => void connect()}
            onCancel={() => void invoke("discord_cancel").catch(() => {})}
          />
        )}

        {profile && tab === "ideas" && <IdeasPanel me={profile} onError={setError} onNotice={setNotice} />}
        {profile && tab === "roadmap" && <RoadmapPanel me={profile} onError={setError} />}
        {profile && tab === "completed" && <CompletedPanel me={profile} onError={setError} />}

        {profile && tab === "play" && (
          <PlayPanel
            news={activeNews(news)}
            phase={phase}
            launchArgs={config.launch_args}
            busy={busy}
            onLaunchArgs={(launch_args) => patchConfig({ launch_args })}
            onPrimary={onPrimary}
            onLaunch={onLaunch}
            onStop={stopGame}
            onError={setError}
          />
        )}

        {profile && tab === "download" && (
          <DownloadPanel
            installed={install.installed}
            installDir={config.install_dir}
            onFolder={(install_dir) => patchConfig({ install_dir })}
            onError={setError}
            onInstalled={onInstalled}
            onUninstalled={onUninstalled}
          />
        )}

        {profile && tab === "settings" && (
          <SettingsPanel
            config={config}
            hosts={hosts}
            onConfig={(patch) => {
              patchConfig(patch);
              // domain/IP edits change what the status means
              window.setTimeout(refreshHosts, 350);
            }}
            onHostsFix={() =>
              void invoke("hosts_apply")
                .then(refreshHosts)
                .catch((e) => setError(String(e)))
            }
            onHostsRemove={() =>
              void invoke("hosts_remove")
                .then(refreshHosts)
                .catch((e) => setError(String(e)))
            }
            onHostsRefresh={refreshHosts}
            onOpenFolder={() => void invoke("open_install_dir").catch((e) => setError(String(e)))}
            appVersion={appVersion}
            update={update}
            checkingUpdate={checkingUpdate}
            updateChecked={updateChecked}
            updateError={updateError}
            onCheckUpdate={() => runUpdateCheck(false)}
            profile={profile}
            onSignOut={() => void signOut()}
          />
        )}
      </main>

      {error && (
        <div className="toast" role="alert" onClick={() => setError(null)}>
          {error}
        </div>
      )}

      {!error && notice && (
        <div className="toast toast--info" onClick={() => setNotice(null)}>
          {notice}
        </div>
      )}

      {!error && !notice && update && (
        <div className="toast toast--info" role="status">
          {installingUpdate ? (
            <span>
              Installing v{update.version}
              {updateProgress?.total
                ? ` — ${Math.min(100, Math.round((updateProgress.done / updateProgress.total) * 100))}%`
                : "…"}
            </span>
          ) : (
            <span className="field__row" style={{ alignItems: "center" }}>
              <span style={{ marginRight: 10 }}>Update available: v{update.version}</span>
              <button className="btn btn--primary" type="button" onClick={runUpdateInstall}>
                Install &amp; Restart
              </button>
              <button className="btn" type="button" onClick={() => setUpdate(null)}>
                Later
              </button>
            </span>
          )}
        </div>
      )}
    </div>
  );
}
