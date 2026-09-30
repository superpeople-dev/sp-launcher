// Line icons for the community pages, drawn on a 24px grid with the text colour.
const PATHS = {
  all: "M4 6h16M4 12h16M4 18h16",
  bug: "M8.5 9h7v5.5a3.5 3.5 0 0 1-7 0zM9.5 9V7.5a2.5 2.5 0 0 1 5 0V9M3.5 13h5M15.5 13h5M5 8l3.5 2M19 8l-3.5 2M5 19l3.5-2.5M19 19l-3.5-2.5",
  idea: "M9 18h6M10 21h4M12 3a6 6 0 0 0-3.8 10.6c.6.5.8 1.2.8 1.9V16h6v-.5c0-.7.2-1.4.8-1.9A6 6 0 0 0 12 3z",
  improvement: "M3 17l6-6 4 4 8-8M15 7h6v6",
  question: "M9.3 9.3a2.8 2.8 0 1 1 3.9 2.6c-.7.3-1.2 1-1.2 1.8V14M12 17.6v.1M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z",
  top: "M12 20V5M5.5 11.5 12 5l6.5 6.5",
  new: "M12 7v5l3.5 2M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z",
  chevron: "M7 10l5 5 5-5",
  check: "M5 12.5l4.5 4.5L19 7.5",
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg className={`icon${className ? ` ${className}` : ""}`} viewBox="0 0 24 24" aria-hidden>
      <path d={PATHS[name]} />
    </svg>
  );
}
