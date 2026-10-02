import type { CSSProperties } from "react";
import type { ItemTag } from "../../types";
import { Icon, type IconName } from "./Icon";

// The icon beside a tag's name, as on the website (sp-website lib/site.ts ideaTypes and
// CategoryTag.tsx): the item's type (Bug, Idea, Improvement...) or its platform (Launcher,
// Servers, Game...). Any other tag gets the plain tag icon.
const ICONS: [RegExp, IconName][] = [
  [/bug/i, "bug"],
  [/idea|feature/i, "sparkle"],
  [/improve|enhance/i, "improvement"],
  [/question/i, "question"],
  [/launch/i, "rocket"],
  [/server|backend|lobby/i, "server"],
  [/web|site/i, "globe"],
  [/game|client/i, "gamepad"],
  [/discord|community/i, "team"],
];

export const tagIcon = (name: string): IconName => ICONS.find(([pattern]) => pattern.test(name))?.[1] ?? "tag";

/** One of an item's tags in its colour, with its icon. */
export function Tag({ tag }: { tag: ItemTag }) {
  return (
    <span className="tag" style={{ "--tag": tag.color } as CSSProperties}>
      <Icon name={tagIcon(tag.name)} />
      {tag.name}
    </span>
  );
}
