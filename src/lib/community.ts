import { invoke } from "@tauri-apps/api/core";
import type { CommunityComment, CommunityItem, ItemStatus, Vote } from "../types";

/**
 * The website's Ideas, Roadmap and Completed items. The Rust side
 * (src-tauri/src/community.rs) calls superpeople.dev with the player's
 * session -- the webview itself may not reach outside hosts (CSP).
 */

export type Board = "ideas" | "roadmap" | "completed";

export const listItems = (board: Board) => invoke<CommunityItem[]>("community_items", { board });

export const listComments = (id: string) => invoke<CommunityComment[]>("community_comments", { id });

/** Presses an arrow, as on the website: the arrow already chosen takes the
 * vote back, the other switches it. Answers the vote and score as they are now. */
export const castVote = (id: string, direction: Vote) =>
  invoke<{ score: number; myVote: Vote | null }>("community_vote", { id, direction });

export const postComment = (id: string, body: string) =>
  invoke<CommunityComment>("community_comment", { id, body });

export type IdeaKind = "idea" | "bug";
/** A platform a new idea is filed under (Game, Launcher, ...), by the website's id. */
export interface Platform {
  id: string;
  name: string;
}

export const listPlatforms = () => invoke<Platform[]>("community_platforms");

/** Posts an idea or bug report, under a platform id or "other". It waits for
 * the team's review before it shows on the boards, as on the website. */
export const postIdea = (title: string, description: string, kind: IdeaKind, platform: string) =>
  invoke<void>("community_post_idea", { title, description, kind, platform });

/** The website's limits (site.ts): a longer text is refused there. */
export const LIMITS = { title: [3, 60], description: 2000, comment: 1000 } as const;

export const STATUS_LABEL: Record<ItemStatus, string> = {
  open: "Open",
  under_review: "In review",
  planned: "Planned",
  in_progress: "In progress",
  completed: "Done",
  closed: "Closed",
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
