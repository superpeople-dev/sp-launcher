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
  /** The version that last reported itself to the team (#launcher-logs). */
  last_version: string;
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

/** Mirrors lib.rs `GameFiles`: Play's check of the game's files (integrity.rs). */
export interface GameFiles {
  ok: boolean;
  /** Why Play is refused, for the player. */
  message: string;
  missing: number;
  changed: number;
  unchecked: number;
  /** Paks and DLLs that are not part of the game. */
  extra: string[];
  /** DLSS / XeSS libraries swapped for a build the launcher does not recognise. */
  replaced: number;
}

/** Mirrors `auth::Terms`: what Play needs accepted, from the website. */
export interface Terms {
  /** The legal pages' date, YYYY-MM-DD. */
  version: string;
  accepted: boolean;
  docs: { title: string; url: string; intro: string; sections: { title: string; body: string }[] }[];
}

export interface HostsStatus {
  path: string;
  /** False means the launcher is not running as administrator. */
  writable: boolean;
  applied: boolean;
  /** Hand-written lines mapping the same hostnames elsewhere in the file. */
  conflicts: string[];
}

export type Tab = "play" | "download" | "ideas" | "roadmap" | "completed" | "leaderboard" | "twitch" | "settings";

/** The signed-in player, from Discord. Mirrors `auth::Profile`. */
export interface Profile {
  /** Discord user id. */
  id: string;
  /** Discord display name, or the username when there is none. */
  name: string;
  username: string;
  avatar: string | null;
  /** A website admin: the admin tools show. The website checks again on every call. */
  admin: boolean;
  permissions: Permission[];
  /** What they are on the team (sp-website lib/staff.ts); empty for a player, and from websites
   *  before the staff kinds. Only the staff see the client fixes debug window setting. */
  staff?: StaffKind[];
}

export type StaffKind = "admin" | "moderator" | "developer";

/** An admin, moderator or developer (auth.rs `Profile::is_staff`). */
export const isStaff = (profile: Profile | null | undefined) => Boolean(profile && (profile.admin || profile.staff?.length));

/** What a website admin may do (sp-website lib/board.ts). */
export type Permission = "review" | "manage" | "comments" | "bans";

// ------------------------------------------------------------- community ---
// The website's Ideas, Roadmap and Completed items (superpeople.dev), as the
// launcher shows them. Mirrors `community::Item` in src-tauri/src/community.rs.

export type ItemStatus = "open" | "under_review" | "planned" | "in_progress" | "completed" | "closed";
export type Vote = "up" | "down";

export interface Person {
  name: string;
  /** Discord avatar URL; Discord's default picture is drawn when there is none (Avatar.tsx). */
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
  /** Its type and platform, which an admin's edit keeps. */
  typeId: string | null;
  platformId: string | null;
}

export interface CommentReply {
  id: string;
  author: Person | null;
  /** Posted by the SUPER PEOPLE team. */
  official: boolean;
  body: string;
  createdAt: number;
  /** Written by the player, who may delete it. */
  mine: boolean;
}

export interface CommunityComment extends CommentReply {
  replies: CommentReply[];
}

/** An admin: whom an item is assigned to, and whom it can be assigned to. */
export interface Member {
  id: string | null;
  name: string;
  avatar: string | null;
}

/** An item's discussion, and what an admin sees of it. */
export interface Thread {
  comments: CommunityComment[];
  /** Comments turned off: only admins may still post. */
  off: boolean;
  /** Null: the whole team. */
  assignee: Member | null;
  /** Whom an admin who manages items can assign it to. */
  staff: Member[];
}

/** A platform or a type, by the website's id. */
export interface Choice {
  id: string;
  name: string;
}

/** Mirrors `download::Status` in src-tauri/src/download.rs — keep the two in step. */
export interface DownloadStatus {
  phase: "idle" | "checking" | "downloading" | "preparing" | "paused" | "done" | "failed";
  dir: string;
  /** Bytes of the missing files downloaded so far. */
  done: number;
  /** Size of the missing files; 0 while unknown. */
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

/** What stops this player from playing (lib.rs ban_status, sp-website /api/launcher/me). */
export interface Ban {
  reason: string;
  /** When it was made, epoch ms. */
  at: number;
  /** When it ends, epoch ms; null for a ban until lifted (`permanent`), which signs the launcher out. */
  until: number | null;
  permanent: boolean;
  /** In a match right now: a running game is closed once that round is over. */
  inMatch: boolean;
}
