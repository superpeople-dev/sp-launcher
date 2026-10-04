import { invoke } from "@tauri-apps/api/core";
import type { Choice, CommunityComment, CommunityItem, ItemStatus, Permission, Profile, Thread, Vote } from "../types";

/**
 * The website's Ideas, Roadmap and Completed items. The Rust side
 * (src-tauri/src/community.rs) calls superpeople.dev with the player's
 * session -- the webview itself may not reach outside hosts (CSP).
 */

export type Board = "ideas" | "roadmap" | "completed";
const BOARDS: Board[] = ["ideas", "roadmap", "completed"];

export const listItems = (board: Board) => invoke<CommunityItem[]>("community_items", { board });

export const fetchThread = (id: string) => invoke<Thread>("community_thread", { id });

// ------------------------------------------------------------------ cache ---
// The pages keep what they loaded: opening one again shows it at once and
// refreshes it quietly. All three are loaded as soon as the player is signed
// in (App.tsx), so the first visit is instant too. The cache belongs to the
// signed-in player (their own votes are in it): signing out clears it.

const itemsCache = new Map<Board, CommunityItem[]>();
const loading = new Map<Board, Promise<CommunityItem[]>>();
const threadsCache = new Map<string, Thread>();
// Pages on screen, told when their board was loaded again (after an admin
// moved an item from one board to another, say).
const listeners = new Map<Board, Set<(items: CommunityItem[]) => void>>();

export const cachedItems = (board: Board) => itemsCache.get(board) ?? null;
export const storeItems = (board: Board, items: CommunityItem[]) => void itemsCache.set(board, items);

export function onBoard(board: Board, listener: (items: CommunityItem[]) => void) {
  const set = listeners.get(board) ?? new Set();
  set.add(listener);
  listeners.set(board, set);
  return () => void set.delete(listener);
}

/** A board's items, fresh from the website; one request at a time per board. */
export function loadItems(board: Board): Promise<CommunityItem[]> {
  let pending = loading.get(board);
  if (!pending) {
    pending = listItems(board)
      .then((items) => {
        itemsCache.set(board, items);
        listeners.get(board)?.forEach((listener) => listener(items));
        return items;
      })
      .finally(() => loading.delete(board));
    loading.set(board, pending);
  }
  return pending;
}

/** Loads all three boards: at sign-in, and after an admin change. */
export function preloadBoards() {
  for (const board of BOARDS) void loadItems(board).catch(() => {});
}

export const cachedThread = (id: string) => threadsCache.get(id) ?? null;
export const storeThread = (id: string, thread: Thread) => void threadsCache.set(id, thread);

export async function loadThread(id: string) {
  const thread = await fetchThread(id);
  threadsCache.set(id, thread);
  return thread;
}

export function clearCommunityCache() {
  itemsCache.clear();
  threadsCache.clear();
}

// ----------------------------------------------------------------- player ---

/** Presses an arrow, as on the website: the arrow already chosen takes the
 * vote back, the other switches it. Answers the vote and score as they are now. */
export const castVote = (id: string, direction: Vote) =>
  invoke<{ score: number; myVote: Vote | null }>("community_vote", { id, direction });

export const postComment = (id: string, body: string) =>
  invoke<CommunityComment>("community_comment", { id, body });

export const deleteComment = (id: string, commentId: string) =>
  invoke<void>("community_delete_comment", { id, commentId });

export type IdeaKind = "idea" | "bug";
/** A platform a new idea is filed under (Game, Launcher, ...), by the website's id. */
export type Platform = Choice;

/** The platforms and types an idea is filed under. */
export const loadMeta = () => invoke<{ platforms: Choice[]; types: Choice[] }>("community_meta");

/** Posts an idea or bug report, under a platform id or "other". It waits for
 * the team's review before it shows on the boards, as on the website. */
export const postIdea = (title: string, description: string, kind: IdeaKind, platform: string) =>
  invoke<void>("community_post_idea", { title, description, kind, platform });

/** The website's limits (site.ts): a longer text is refused there. An admin's
 * edit allows a longer title. */
export const LIMITS = { title: [3, 60], titleAdmin: 100, description: 2000, comment: 1000 } as const;

// ------------------------------------------------------------------ admin ---
// The website's admin routes; the website checks the permission on every call,
// these only decide which buttons an admin sees.

export const can = (profile: Profile, permission: Permission) => profile.admin && profile.permissions.includes(permission);

const admin = (id: string, change: Record<string, unknown>) => invoke<void>("community_admin", { id, change });

/** Moves an item (approving an idea waiting for review is moving it to "open"). */
export const setStatus = (id: string, status: ItemStatus) => admin(id, { action: "status", status });

export const editItem = (id: string, fields: { title: string; description: string; typeId: string | null; categoryId: string | null }) =>
  admin(id, { action: "edit", ...fields });

/** To an admin, or null for the whole team. */
export const assignItem = (id: string, assignee: string | null) => admin(id, { action: "assign", assignee });

/** Deleting an idea waiting for review is rejecting it. */
export const deleteItem = (id: string) => admin(id, { action: "delete" });

export const setCommentsOff = (id: string, off: boolean) => invoke<void>("community_comments_off", { id, off });

/** The page an item shows on (the website's items route). Closed ones show nowhere. */
export function boardOf(status: ItemStatus): Board | null {
  if (status === "open" || status === "under_review") return "ideas";
  if (status === "planned" || status === "in_progress") return "roadmap";
  return status === "completed" ? "completed" : null;
}

/** Where an admin can move an item, as on the website's "Move to". */
export const DESTINATIONS: { status: ItemStatus; label: string }[] = [
  { status: "open", label: "Ideas" },
  { status: "planned", label: "Planned" },
  { status: "in_progress", label: "In progress" },
  { status: "completed", label: "Done" },
];

// -------------------------------------------------------------------- text ---

export const STATUS_LABEL: Record<ItemStatus, string> = {
  open: "Open",
  under_review: "In review",
  planned: "Planned",
  in_progress: "In progress",
  completed: "Done",
  closed: "Closed",
};

/** The status badge colours (community.css .status--*), for the menus' dots. */
export const STATUS_COLOR: Record<ItemStatus, string> = {
  open: "var(--paper-dim)",
  under_review: "var(--paper-dim)",
  planned: "var(--gold)",
  in_progress: "var(--signal-hi)",
  completed: "var(--good)",
  closed: "var(--surface-3)",
};

/** What a vote changes locally before the website answers. */
export function applyVote(item: CommunityItem, vote: Vote | null): CommunityItem {
  const value = (v: Vote | null) => (v === "up" ? 1 : v === "down" ? -1 : 0);
  return { ...item, myVote: vote, score: item.score - value(item.myVote) + value(vote) };
}

/** "just now", "5 min ago", "3 h ago", "2 days ago", then the date. */
export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d < 14) return d === 1 ? "yesterday" : `${d} days ago`;
  return day(ms);
}

/** "Sep 29", with the year only when it is not this year. */
export function day(ms: number): string {
  const date = new Date(ms);
  const thisYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleDateString("en-US", { day: "numeric", month: "short", year: thisYear ? undefined : "numeric" });
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "3 Oct 2026": day, month, year, always with the year (Completed's dates). Built by hand:
 *  en-GB writes September as "Sept", out of step with the other three-letter months. */
export function fullDay(ms: number): string {
  const date = new Date(ms);
  return `${date.getDate()} ${MONTHS[date.getMonth()]} ${date.getFullYear()}`;
}
