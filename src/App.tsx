import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import type { Update } from "@tauri-apps/plugin-updater";

import { TitleBar } from "./components/TitleBar";
import { PlayPanel } from "./components/PlayPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { DownloadPanel } from "./components/DownloadPanel";
import { SignIn } from "./components/SignIn";
import { activeNews } from "./news";
import { checkForUpdate, installUpdate } from "./lib/updater";
import type { AuthStatus, Config, HostsStatus, InstallState, NewsItem, Phase, Tab } from "./types";

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

  // Sign-in state is kept separate from `config` because it is the backend's
  // answer, not a setting: the config only remembers it so the window can be
  // drawn before the first call comes back.
  const [auth, setAuth] = useState<AuthStatus | null>(null);
  const [authBusy, setAuthBusy] = useState(false);
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

      // There's no manifest to download from, so a folder with the game
      // already in it is the only way to get going — ask for it right away
      // rather than leaving the user to find the Install button on their own.
      // A picker that fails or is dismissed must not hold up the UI.
      // Not fatal: a launcher that cannot reach the backend must still open,
      // so the gate falls back to "signed out" and says why when they try.
      void invoke<AuthStatus>("auth_status")
        .then(setAuth)
        .catch(() => setAuth({ signed_in: false, account_id: "", display_name: "", status: "" }));

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

  // `quiet` suppresses the error toast for the automatic startup check — a
  // launcher that can't reach its update server should still open normally.
  // A check the user asked for always reports what went wrong, so a dead
  // endpoint can't masquerade as "up to date".
  const runUpdateCheck = useCallback((quiet = false) => {
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

  const runUpdateInstall = useCallback(() => {
    if (!update) return;
    setInstallingUpdate(true);
    setUpdateProgress(null);
    void installUpdate(update, (done, total) => setUpdateProgress({ done, total })).catch((e) => {
      // A successful run typically exits the process itself (see
      // lib/updater.ts) before this ever runs — only a genuine failure
      // reaches here.
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

  const redeemKey = async (key: string) => {
    setAuthBusy(true);
    setAuthError(null);
    try {
      const next = await invoke<AuthStatus>("redeem_key", { key });
      setAuth(next);
      // The stored copy changed on the Rust side; pull it back so Settings
      // shows the account without a restart.
      setConfig(await invoke<Config>("get_config"));
    } catch (e) {
      setAuthError(String(e));
    } finally {
      setAuthBusy(false);
    }
  };

  const signOut = async () => {
    try {
      await invoke("sign_out");
      setAuth({ signed_in: false, account_id: "", display_name: "", status: "" });
      setAuthError(null);
      setConfig(await invoke<Config>("get_config"));
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

      <TitleBar tab={tab} onTab={setTab} />

      <main className="stage">
        {tab === "play" && auth && !auth.signed_in && (
          <SignIn busy={authBusy} error={authError} onRedeem={(k) => void redeemKey(k)} />
        )}

        {tab === "play" && (!auth || auth.signed_in) && (
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

        {tab === "download" && (
          <DownloadPanel
            installed={install.installed}
            installDir={config.install_dir}
            onFolder={(install_dir) => patchConfig({ install_dir })}
            onError={setError}
            onInstalled={onInstalled}
          />
        )}

        {tab === "settings" && (
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
            auth={auth}
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
