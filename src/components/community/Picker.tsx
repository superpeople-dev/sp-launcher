import { useEffect, useRef, useState } from "react";
import type { Person } from "../../types";
import { Avatar } from "./Avatar";
import { Icon, type IconName } from "./Icon";

/** Open until a click elsewhere or Escape. */
function useDropdown() {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => !root.current?.contains(e.target as Node) && setOpen(false);
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);
  return { open, setOpen, root };
}

export interface PickOption<T extends string> {
  id: T;
  label: string;
  icon?: IconName;
  /** A coloured dot instead of an icon (statuses). */
  dot?: string;
  /** A Discord picture instead of an icon (admins). */
  person?: Person | null;
  /** The SP logo: the whole team (an item assigned to nobody in particular). */
  team?: boolean;
  count?: number;
}

function Mark<T extends string>({ option }: { option: PickOption<T> }) {
  if (option.team) return <TeamMark />;
  if (option.person !== undefined) return <Avatar person={option.person} size={18} />;
  if (option.dot) return <span className="pick__dot" style={{ background: option.dot }} />;
  if (option.icon) return <Icon name={option.icon} />;
  return null;
}

/** The whole team, where an item is not assigned to one admin: the SP logo in
 * place of an avatar, as on the website. */
export function TeamMark({ size = 18 }: { size?: number }) {
  return <span className="team-mark" style={{ width: size, height: size }} aria-hidden />;
}

/** A dropdown that picks one value: the Ideas type filter, an item's status
 * and whom it is assigned to. */
export function Picker<T extends string>({
  value,
  options,
  onChange,
  label,
  small,
  align = "left",
  disabled,
}: {
  value: T;
  options: PickOption<T>[];
  onChange: (value: T) => void;
  /** Shown before the chosen option ("Status"), for small pickers. */
  label?: string;
  small?: boolean;
  align?: "left" | "right";
  disabled?: boolean;
}) {
  const { open, setOpen, root } = useDropdown();
  const current = options.find((o) => o.id === value) ?? options[0];
  return (
    <div className={`pick${open ? " is-open" : ""}${small ? " pick--small" : ""}`} ref={root}>
      <button
        type="button"
        className="pick__btn"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        {label && <span className="pick__prefix">{label}</span>}
        {current && <Mark option={current} />}
        <span className="pick__current">{current?.label}</span>
        <Icon name="chevron" className="pick__chev" />
      </button>
      {open && (
        <ul className={`pick__menu${align === "right" ? " pick__menu--right" : ""}`} role="listbox">
          {options.map((o) => (
            <li key={o.id}>
              <button
                type="button"
                role="option"
                aria-selected={o.id === value}
                className={`pick__opt${o.id === value ? " is-on" : ""}`}
                onClick={() => {
                  setOpen(false);
                  if (o.id !== value) onChange(o.id);
                }}
              >
                <Mark option={o} />
                <span className="pick__label">{o.label}</span>
                {o.count !== undefined && <span className="pick__count">{o.count}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export interface MenuAction {
  label: string;
  icon?: IconName;
  /** A coloured dot instead of an icon (statuses). */
  dot?: string;
  onSelect: () => void;
  danger?: boolean;
  /** Where the item already is: ticked, and not offered. */
  current?: boolean;
}

/** A heading over the next actions ("Move to"), or a line between groups. */
export type MenuEntry = MenuAction | { heading: string } | "line";

/** A "more" button with a list of actions (an item's admin actions). */
export function Menu({ entries, title, disabled }: { entries: MenuEntry[]; title: string; disabled?: boolean }) {
  const { open, setOpen, root } = useDropdown();
  return (
    <div className={`pick pick--small${open ? " is-open" : ""}`} ref={root}>
      <button
        type="button"
        className="pick__btn pick__btn--icon"
        title={title}
        aria-label={title}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        <Icon name="more" />
      </button>
      {open && (
        <ul className="pick__menu pick__menu--right" role="menu">
          {entries.map((entry, i) =>
            entry === "line" ? (
              <li key={i} className="pick__line" role="separator" />
            ) : "heading" in entry ? (
              <li key={i} className="pick__heading" aria-hidden>
                {entry.heading}
              </li>
            ) : (
              <li key={i}>
                <button
                  type="button"
                  role="menuitem"
                  disabled={entry.current}
                  className={`pick__opt${entry.danger ? " is-danger" : ""}${entry.current ? " is-current" : ""}`}
                  onClick={() => {
                    setOpen(false);
                    entry.onSelect();
                  }}
                >
                  {entry.dot ? <span className="pick__dot" style={{ background: entry.dot }} /> : entry.icon && <Icon name={entry.icon} />}
                  <span className="pick__label">{entry.label}</span>
                  {entry.current && <Icon name="check" className="pick__tick" />}
                </button>
              </li>
            ),
          )}
        </ul>
      )}
    </div>
  );
}
