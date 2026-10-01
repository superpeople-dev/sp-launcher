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
import { TermsDialog } from "./components/TermsDialog";
import { IdeasPanel } from "./components/community/IdeasPanel";
import { RoadmapPanel } from "./components/community/RoadmapPanel";
import { CompletedPanel } from "./components/community/CompletedPanel";
import { activeNews } from "./news";
import { checkForUpdate, installUpdate } from "./lib/updater";
import { clearCommunityCache, preloadBoards } from "./lib/community";
import type { AuthState, Config, HostsStatus, InstallState, NewsItem, Phase, Profile, Tab, Terms } from "./types";

// Re-check which items are in their [starts_at, ends_at) window every so
// often, so an event that just started (or just ended) updates without the
// user having to reopen the launcher.
const NEWS_REFRESH_MS = 10 * 60 * 1000;

// The website's last answer about the terms, per Discord account, kept on this
// PC. After a restart (an update) Play shows it at once, rather than a locked
// button for the second the website takes to answer. The answer then replaces
// it, and Play itself is checked by the website (launch_game) whatever this says.
const termsKey = (id: string) => `sp.terms.accepted.${id}`;
function acceptedBefore(id: string): boolean {
  try {
    return localStorage.getItem(termsKey(id)) === "1";
  } catch {
    return false;
  }
}
function rememberAccepted(id: string, accepted: boolean) {
  try {
    if (accepted) localStorage.setItem(termsKey(id), "1");
    else localStorage.removeItem(termsKey(id));
  } catch {
    // Without storage the button just waits for the website, as before.
  }
}

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

  // The Terms of Service, which Play needs accepted (TermsDialog). `null` until
  // the website has answered; meanwhile Play shows what it said last time on
  // this PC (acceptedBefore), locked if it never said yes.
  const [terms, setTerms] = useState<Terms | null>(null);
  const [termsOpen, setTermsOpen] = useState(false);
  const [termsNote, setTermsNote] = useState<string | null>(null);

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
      // Then the website is asked who the player is now (name, picture, admin
      // rights), quietly: offline, the saved profile stays.
      void invoke<AuthState>("auth_status")
        .then((a) => {
          setProfile(a.profile);
          if (a.profile) {
            void invoke<Profile | null>("auth_refresh")
              .then((fresh) => setProfile((now) => (now ? fresh : now)))
              .catch(() => {});
          }
        })
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

  // A notice ("Moved to Planned.") says it and goes; clicking it closes it sooner.
  useEffect(() => {
    if (!notice) return;
    const id = window.setTimeout(() => setNotice(null), 5000);
    return () => window.clearTimeout(id);
  }, [notice]);

  // Ideas, Roadmap and Completed load as soon as the player is signed in, so
  // their tabs open at once. They carry that player's own votes: signing out
  // drops them.
  const profileId = profile?.id;
  useEffect(() => {
    if (profileId) preloadBoards();
    else clearCommunityCache();
  }, [profileId]);

  // Whether this Discord account accepted the current terms, from the website.
  // `open` shows them as soon as they are here, unless already accepted.
  const lastTermsCheck = useRef(0);
  const loadTerms = useCallback((open = false) => {
    lastTermsCheck.current = Date.now();
    void invoke<Terms>("launcher_terms")
      .then((t) => {
        setTerms(t);
        if (open && !t.accepted) setTermsOpen(true);
      })
      .catch((e) => {
        if (open) setError(String(e));
      });
  }, []);

  useEffect(() => {
    if (profileId) {
      loadTerms();
    } else {
      setTerms(null);
      setTermsOpen(false);
    }
  }, [profileId, loadTerms]);

  // Each answer (at start, on focus, after accepting) is the one shown next time.
  useEffect(() => {
    if (profileId && terms) rememberAccepted(profileId, terms.accepted);
  }, [profileId, terms]);
  const termsAccepted = terms ? terms.accepted : !!profileId && acceptedBefore(profileId);

  // Coming back to the launcher asks again (at most once a minute): terms that
  // changed meanwhile lock Play until they are read.
  useEffect(() => {
    if (!profileId) return;
    let off: (() => void) | undefined;
    let gone = false;
    void getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused && Date.now() - lastTermsCheck.current >= 60_000) loadTerms();
      })
      .then((unlisten) => {
        if (gone) unlisten();
        else off = unlisten;
      });
    return () => {
      gone = true;
      off?.();
    };
  }, [profileId, loadTerms]);

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

  // The download or Verify files a launcher update interrupted continues
  // where it stopped (download.rs pause_for_update), on the Download tab.
  const resumeDownload = useCallback(() => {
    void invoke<{ dir: string; verify: boolean } | null>("download_take_resume")
      .then((run) => {
        if (!run) return;
        setTab("download");
        return invoke("download_start", { dir: run.dir, verify: run.verify });
      })
      .catch((e) => setError(String(e)));
  }, []);

  const runUpdateInstall = useCallback(async () => {
    if (!update) return;
    installingRef.current = true;
    setInstallingUpdate(true);
    setUpdateProgress(null);
    // A running download or Verify files first stops where it is, its files
    // closed; the updated launcher continues it.
    const paused = await invoke<boolean>("download_pause_for_update").catch(() => false);
    try {
      await installUpdate(update, (done, total) => setUpdateProgress({ done, total }));
    } catch (e) {
      // A successful run typically exits the process itself (see
      // lib/updater.ts) before this ever runs — only a genuine failure
      // reaches here, and the paused download carries on at once.
      installingRef.current = false;
      setInstallingUpdate(false);
      setError(`Update failed: ${String(e)}`);
      if (paused) resumeDownload();
    }
  }, [update, resumeDownload]);

  // After a launcher update: the download it paused continues, once the player
  // is signed in (the website wants the sign-in for each file's link).
  const resumed = useRef(false);
  useEffect(() => {
    if (!profileId || resumed.current) return;
    resumed.current = true;
    resumeDownload();
  }, [profileId, resumeDownload]);

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
      // Play found the terms not accepted (they changed since the launcher
      // last asked): show the current ones.
      listen("terms:required", () => loadTerms(true)),
    ];
    return () => {
      unlisten.forEach((p) => void p.then((off) => off()));
    };
  }, [loadTerms]);

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
        {profile && tab === "roadmap" && <RoadmapPanel me={profile} onError={setError} onNotice={setNotice} />}
        {profile && tab === "completed" && <CompletedPanel me={profile} onError={setError} onNotice={setNotice} />}

        {profile && tab === "play" && (
          <PlayPanel
            news={activeNews(news)}
            phase={phase}
            launchArgs={config.launch_args}
            busy={busy}
            locked={!termsAccepted}
            onUnlock={() => (terms ? setTermsOpen(true) : loadTerms(true))}
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

      {profile && termsOpen && terms && (
        <TermsDialog
          key={terms.version}
          terms={terms}
          note={termsNote}
          onClose={() => {
            setTermsOpen(false);
            setTermsNote(null);
          }}
          onResult={(fresh) => {
            setTerms(fresh);
            if (fresh.accepted) {
              setTermsOpen(false);
              setTermsNote(null);
              setError(null);
              setNotice("Thanks! You can play now.");
            } else {
              setTermsNote("The terms changed while you were reading them. Here is the new version.");
            }
          }}
        />
      )}

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
