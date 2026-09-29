import type { Update } from "@tauri-apps/plugin-updater";
import type { AuthStatus, Config, HostsStatus } from "../types";
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
  auth: AuthStatus | null;
  onSignOut: () => void;
}

const TOGGLES: { key: keyof Config; name: string; hint: string }[] = [
  { key: "close_on_launch", name: "Minimize to tray on launch", hint: "Send the launcher to the tray once the game starts, instead of staying open" },
  { key: "client_fixes_enabled", name: "Client fixes", hint: "Apply client fixes" },
];

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
  auth,
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

        {auth && auth.signed_in ? (
          <>
            <div className="field">
              <span className="field__label">Signed in as</span>
              <span className="field__hint">
                {auth.display_name || auth.account_id || "your account"}
                {auth.status && auth.status !== "active" ? ` — key ${auth.status}` : ""}
              </span>
            </div>
            {auth.status === "suspended" && (
              <span className="field__hint">
                Your key is suspended, so the game will not start. A Key Master can lift it in Discord.
              </span>
            )}
            {auth.status === "revoked" && (
              <span className="field__hint">
                Your key has been revoked and can no longer be used.
              </span>
            )}
            <div className="field__row" style={{ marginTop: 10 }}>
              <button className="btn" type="button" onClick={onSignOut}>
                Sign out
              </button>
            </div>
            <span className="field__hint" style={{ marginTop: 8, display: "block" }}>
              Signing out forgets the key on this PC. You can enter it again at any time.
            </span>
          </>
        ) : (
          <span className="field__hint">
            Not signed in. Go to the Play tab and enter your launcher key.
          </span>
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
        {config.client_fixes_enabled && (
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
