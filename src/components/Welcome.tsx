import logo from "../assets/sp-logo.png";

interface Props {
  /** The Discord window is open: waiting for the player to finish there. */
  waiting: boolean;
  error: string | null;
  /** Signed in with a launcher key before: say why they see this again. */
  hadKey: boolean;
  onConnect: () => void;
  onCancel: () => void;
}

export const DISCORD_PATH =
  "M20.317 4.369a19.79 19.79 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.211.375-.445.865-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128c.126-.094.252-.192.372-.291a.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.009c.12.099.246.198.373.292a.077.077 0 0 1-.006.127c-.598.35-1.22.644-1.873.891a.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.056c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.028ZM8.02 15.331c-1.183 0-2.157-1.086-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.211 0 2.176 1.096 2.157 2.42 0 1.332-.955 2.418-2.157 2.418Zm7.975 0c-1.183 0-2.157-1.086-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.211 0 2.176 1.096 2.157 2.42 0 1.332-.946 2.418-2.157 2.418Z";

/** The first screen until the player connects Discord: the only way in. */
export function Welcome({ waiting, error, hadKey, onConnect, onCancel }: Props) {
  return (
    <div className="welcome">
      <div className="welcome__glow" aria-hidden />
      <div className="welcome__card">
        <img className="welcome__logo" src={logo} alt="SUPER PEOPLE" draggable={false} />
        <p className="welcome__kicker">Community revival</p>
        <h1 className="welcome__title">Welcome to SUPER PEOPLE</h1>
        <p className="welcome__lead">
          Connect your Discord account to play, vote on ideas and follow what the team is working on.
        </p>

        {hadKey && !waiting && (
          <p className="welcome__notice">
            Launcher keys are retired. Connect the Discord account you got your key with: your account and progress come
            with it.
          </p>
        )}

        {waiting ? (
          <div className="welcome__waiting">
            <span className="spinner" aria-hidden />
            <span>Finish signing in in the Discord window…</span>
            <button type="button" className="welcome__link" onClick={onCancel}>
              Cancel
            </button>
          </div>
        ) : (
          <button type="button" className="discordbtn" onClick={onConnect}>
            <svg viewBox="0 0 24 24" aria-hidden>
              <path d={DISCORD_PATH} fill="currentColor" />
            </svg>
            Connect with Discord
          </button>
        )}

        {error && (
          <p className="welcome__error" role="alert">
            {error}
          </p>
        )}

        <p className="welcome__fine">
          The launcher only uses your Discord name, picture and roles on the SUPER PEOPLE server. Nothing is posted
          for you.
        </p>
      </div>
    </div>
  );
}
