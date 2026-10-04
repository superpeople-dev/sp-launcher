import type { Update } from "@tauri-apps/plugin-updater";
import { isStaff, type Config, type HostsStatus, type Profile } from "../types";
import { Avatar } from "./community/Avatar";
import { DISCORD_PATH } from "./Welcome";
import { pickInstallFolder } from "../lib/browse";

interface Props {
  config: Config;
  hosts: HostsStatus | null;
  onConfig: (patch: Partial<Config>) => void;
  onHostsFix: () => void;
  onHostsRemove: () => void;
  onHostsRefresh: () => void;
  onOpenFolder: () => void;
  appVersion: string;
  update: Update | null;
  checkingUpdate: boolean;
  updateChecked: boolean;
  updateError: string | null;
  onCheckUpdate: () => void;
  profile: Profile | null;
  onSignOut: () => void;
}

const TOGGLES: { key: keyof Config; name: string; hint: string }[] = [
  { key: "close_on_launch", name: "Minimize to tray on launch", hint: "Send the launcher to the tray once the game starts, instead of staying open" },
];
// Client fixes are always on (the game's console lock and the login ticket need them), so they
// are not a setting any more; only their debug window is, and only for the staff.

export function SettingsPanel({
  config,
  hosts,
  onConfig,
  onHostsFix,
  onHostsRemove,
  onHostsRefresh,
  onOpenFolder,
  appVersion,
  update,
  checkingUpdate,
  updateChecked,
  updateError,
  onCheckUpdate,
  profile,
  onSignOut,
}: Props) {
  const needsSetup = config.hosts_redirect && hosts && !hosts.applied;

  async function browse() {
    const picked = await pickInstallFolder(config.install_dir);
    if (picked) onConfig({ install_dir: picked });
  }

  return (
    <section className="panel settings is-active">
      <div className="card">
        <h2 className="card__title">Account</h2>

        {profile ? (
          <>
            <div className="account">
              <Avatar person={profile} size={44} />
              <div className="account__who">
                <span className="account__name">{profile.name}</span>
                <span className="account__user">@{profile.username}</span>
              </div>
              <span className="account__via">
                <svg viewBox="0 0 24 24" aria-hidden>
                  <path d={DISCORD_PATH} fill="currentColor" />
                </svg>
                Discord
              </span>
            </div>
            <div className="field__row" style={{ marginTop: 12 }}>
              <button className="btn" type="button" onClick={onSignOut}>
                Sign out
              </button>
            </div>
            <span className="field__hint" style={{ marginTop: 8, display: "block" }}>
              Signing out disconnects Discord on this PC. Connect again at any time.
            </span>
          </>
        ) : (
          <span className="field__hint">Not signed in.</span>
        )}
      </div>

      <div className="card">
        <h2 className="card__title">Game folder</h2>

        <span className="field__hint" style={{ marginBottom: 10, display: "block" }}>
          The folder with BravoHotelClient.exe in it. Same setting as on the Download tab — download
          there, or browse here to a folder that already has the game.
        </span>

        <div className="field">
          <span className="field__label">Game folder</span>
          <div className="field__row">
            <input
              className="input"
              spellCheck={false}
              value={config.install_dir}
              placeholder="Pick a folder…"
              onChange={(e) => onConfig({ install_dir: e.target.value })}
            />
            <button className="btn" type="button" onClick={() => void browse()}>Browse</button>
            <button className="btn" type="button" onClick={onOpenFolder}>Open folder</button>
          </div>
        </div>
      </div>

      <div className="card">
        <h2 className="card__title">Connection</h2>

        <div className="toggle">
          <span className="toggle__text">
            <span className="toggle__name">Hosts redirect</span>
            <span className="toggle__hint">
              Point the game's domains at the server while it runs
            </span>
          </span>
          <button
            className={`switch${config.hosts_redirect ? " is-on" : ""}`}
            type="button"
            role="switch"
            aria-checked={config.hosts_redirect}
            aria-label="Hosts redirect"
            onClick={() => onConfig({ hosts_redirect: !config.hosts_redirect })}
          />
        </div>

        <div className="field" style={{ marginTop: 10 }}>
          <span className="field__label">Launch arguments</span>
          <textarea
            className="input args"
            spellCheck={false}
            value={config.launch_args}
            onChange={(e) => onConfig({ launch_args: e.target.value })}
          />
        </div>
      </div>

      <div className="card">
        <h2 className="card__title">Hosts file</h2>

        <div className="hoststat">
          <div className="hoststat__row">
            <span className="field__label">Status</span>
            <span className={`hoststat__pill${hosts?.applied ? " is-on" : ""}`}>
              {hosts?.applied ? "Set up" : "Not set up"}
            </span>
          </div>
          <div className="hoststat__path">{hosts?.path ?? "—"}</div>
        </div>

        <span className="field__hint" style={{ margin: "8px 0", display: "block" }}>
          Set up once, then left in place — the launcher itself runs without admin rights. Windows asks
          for admin permission only when the entries are missing (first Play, or after removing them).
        </span>

        {needsSetup && (
          <div className="notice">
            <p>Not set up yet. It happens automatically on Play, or do it now:</p>
            <button className="btn btn--primary" type="button" onClick={onHostsFix}>
              Set up now (admin prompt)
            </button>
          </div>
        )}

        {hosts && hosts.conflicts.length > 0 && (
          <div className="notice notice--warn">
            <p>
              These lines elsewhere in the hosts file map the same names. Once
              the entries are set up, the launcher comments them out so its own
              Backend IP wins; "Remove entries" restores them exactly:
            </p>
            <pre className="notice__code">{hosts.conflicts.join("\n")}</pre>
          </div>
        )}

        <div className="field__row" style={{ marginTop: 12 }}>
          <button className="btn" type="button" onClick={onHostsRefresh}>Refresh</button>
          {hosts?.applied && (
            <button className="btn" type="button" onClick={onHostsRemove}>Remove entries</button>
          )}
        </div>
      </div>

      <div className="card">
        <h2 className="card__title">Launcher</h2>
        {TOGGLES.map((t) => (
          <div className="toggle" key={t.key}>
            <span className="toggle__text">
              <span className="toggle__name">{t.name}</span>
              <span className="toggle__hint">{t.hint}</span>
            </span>
            <button
              className={`switch${config[t.key] ? " is-on" : ""}`}
              type="button"
              role="switch"
              aria-checked={Boolean(config[t.key])}
              aria-label={t.name}
              onClick={() => onConfig({ [t.key]: !config[t.key] } as Partial<Config>)}
            />
          </div>
        ))}
        {/* The staff only (admins, moderators, developers); the launcher opens the window for them only. */}
        {isStaff(profile) && (
          <div className="toggle">
            <span className="toggle__text">
              <span className="toggle__name">Client fixes debug window</span>
              <span className="toggle__hint">Show the DLL diagnostic window when the game starts; takes effect on the next launch</span>
            </span>
            <button
              className={`switch${config.client_fixes_debug_window ? " is-on" : ""}`}
              type="button"
              role="switch"
              aria-checked={config.client_fixes_debug_window}
              aria-label="Client fixes debug window"
              onClick={() => onConfig({ client_fixes_debug_window: !config.client_fixes_debug_window })}
            />
          </div>
        )}

      </div>

      <div className="card">
        <h2 className="card__title">About</h2>
        <div className="field__row" style={{ alignItems: "center", justifyContent: "space-between" }}>
          <span className="field__hint">
            Version {appVersion || "—"}
            {updateChecked && !update && !checkingUpdate && !updateError ? " — up to date" : ""}
            {update ? ` — v${update.version} available` : ""}
            {updateError && !checkingUpdate ? " — check failed" : ""}
          </span>
          <button className="btn" type="button" onClick={onCheckUpdate} disabled={checkingUpdate}>
            {checkingUpdate ? "Checking…" : "Check for updates"}
          </button>
        </div>

        {/* The reason matters more than the fact: an unreachable endpoint and
            a malformed manifest need completely different fixes. */}
        {updateError && !checkingUpdate && (
          <p className="field__hint" style={{ marginTop: 8 }}>
            {updateError}
          </p>
        )}

        <p className="disclaimer">
          This project is a community-driven fan effort and is not affiliated with, endorsed by,
          or sponsored by Wonder People or any of its subsidiaries. All trademarks, service marks,
          trade names, logos, and other intellectual property referenced herein are the property of
          their respective owners and are used solely for identification and descriptive purposes.
        </p>
      </div>
    </section>
  );
}
