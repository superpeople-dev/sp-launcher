/** Mirrors `Config` in src-tauri/src/config.rs — keep the two in step. */
export interface Config {
  install_dir: string;
  launch_args: string;
  /** Last `ip:port` entered in the connect prompt; pre-fills it next launch. */
  last_server: string;
  hosts_redirect: boolean;
  close_on_launch: boolean;
  auto_update: boolean;
  verify_before_launch: boolean;
  debug_logging: boolean;
  client_fixes_enabled: boolean;
  client_fixes_debug_window: boolean;

  // The sign-in belongs to the Rust side: set_config keeps its own copy of
  // these whatever the frontend sends.
  /** DPAPI-encrypted Discord session. Never the session itself. */
  session_sealed: string;
  profile: Profile | null;
  /** Identifies this installation; a label, not a secret. */
  device_id: string;
  /** Left by a retired launcher key sign-in. */
  auth_key_sealed: string;
  account_id: string;
  display_name: string;
  key_status: string;
}

/** Mirrors `auth::AuthState`: read from this PC, no network. */
export interface AuthState {
  profile: Profile | null;
}

export interface InstallState {
  installed: boolean;
  exe_path: string | null;
}

export interface HostsStatus {
  path: string;
  /** False means the launcher is not running as administrator. */
  writable: boolean;
  applied: boolean;
  /** Hand-written lines mapping the same hostnames elsewhere in the file. */
  conflicts: string[];
}

export type Tab = "play" | "ideas" | "roadmap" | "completed" | "download" | "settings";

/** The signed-in player, from Discord. Mirrors `auth::Profile`. */
export interface Profile {
  /** Discord user id. */
  id: string;
  /** Discord display name, or the username when there is none. */
  name: string;
  username: string;
  avatar: string | null;
}

// ------------------------------------------------------------- community ---
// The website's Ideas, Roadmap and Completed items (superpeople.dev), as the
// launcher shows them. Mirrors `community::Item` in src-tauri/src/community.rs.

export type ItemStatus = "open" | "under_review" | "planned" | "in_progress" | "completed" | "closed";
export type Vote = "up" | "down";

export interface Person {
  name: string;
  /** Discord avatar URL; the initial is drawn when there is none. */
  avatar: string | null;
}

export interface ItemTag {
  name: string;
  color: string;
}

export interface CommunityItem {
  id: string;
  title: string;
  description: string;
  status: ItemStatus;
  /** Upvotes minus downvotes, as on the website. */
  score: number;
  myVote: Vote | null;
  commentCount: number;
  /** Unix milliseconds. */
  createdAt: number;
  completedAt: number | null;
  tags: ItemTag[];
  author: Person | null;
}

export interface CommentReply {
  id: string;
  author: Person | null;
  /** Posted by the SUPER PEOPLE team. */
  official: boolean;
  body: string;
  createdAt: number;
}

export interface CommunityComment extends CommentReply {
  replies: CommentReply[];
}

/** Mirrors `download::Status` in src-tauri/src/download.rs — keep the two in step. */
export interface DownloadStatus {
  phase: "idle" | "checking" | "downloading" | "paused" | "verifying" | "extracting" | "done" | "failed";
  dir: string;
  /** Bytes done in the current step (downloaded / hashed / unpacked). */
  done: number;
  /** Size of the current step; 0 while unknown. */
  total: number;
  /** Bytes per second, smoothed. */
  speed: number;
  eta_secs: number | null;
  message: string;
  free_bytes: number | null;
  needed_bytes: number | null;
  retries: number;
  install_dir: string;
}

export type Phase = "ready" | "not-installed";

/** Mirrors `news::NewsItem` in src-tauri/src/news.rs — keep the two in step. */
export interface NewsItem {
  id: string;
  tag: string;
  title: string;
  description: string;
  /** URL of the slide's background image. */
  image: string;
  /** ISO-8601. Empty means "already started". */
  starts_at: string;
  /** ISO-8601. Empty means "never ends". */
  ends_at: string;
  clickable: boolean;
  /** Only meaningful when `clickable` is true. */
  url?: string | null;
}
