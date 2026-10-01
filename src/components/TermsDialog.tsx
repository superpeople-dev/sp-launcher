import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Terms } from "../types";

interface Props {
  terms: Terms;
  /** Why it is open again, when the terms changed while the player read them. */
  note: string | null;
  onClose: () => void;
  /** The terms as they are now, after Accept: accepted, or new ones to read. */
  onResult: (terms: Terms) => void;
}

const dateOf = (version: string) => {
  const date = new Date(`${version}T00:00:00Z`);
  return Number.isNaN(date.getTime())
    ? version
    : date.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
};

/**
 * The Terms of Service and the Privacy Policy, before the first Play and again
 * whenever they change (auth.rs terms). The tick unlocks once the player has
 * scrolled to the end, and Accept once it is ticked. The parent keys this on
 * the version, so new terms start again from the top.
 */
export function TermsDialog({ terms, note, onClose, onResult }: Props) {
  const body = useRef<HTMLDivElement>(null);
  const [atEnd, setAtEnd] = useState(false);
  const [agreed, setAgreed] = useState(false);
  const [saving, setSaving] = useState(false);
  // Shown in the dialog: the launcher's toast sits behind it.
  const [problem, setProblem] = useState("");

  const checkEnd = () => {
    const el = body.current;
    if (el && el.scrollTop + el.clientHeight >= el.scrollHeight - 12) setAtEnd(true);
  };
  // Text short enough to fit has no end to scroll to.
  useLayoutEffect(checkEnd, []);

  const close = () => {
    if (!saving) onClose();
  };

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  });

  const accept = async () => {
    setSaving(true);
    setProblem("");
    try {
      onResult(await invoke<Terms>("accept_terms", { version: terms.version }));
    } catch (e) {
      setProblem(String(e));
      setSaving(false);
    }
  };

  const open = (url: string) => openUrl(url).catch((e) => setProblem(`Could not open the page: ${String(e)}`));

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="terms-title" onMouseDown={close}>
      <div className="dialog__card terms" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title" id="terms-title">
          Before you play
        </h2>
        <p className="field__hint terms__lead">
          {note ??
            "Read our Terms of Service and Privacy Policy, and accept them to play. You only do this once, and again when they change."}
        </p>

        <div className="terms__body" ref={body} onScroll={checkEnd} tabIndex={0}>
          <p className="terms__date">Last updated {dateOf(terms.version)}</p>
          {terms.docs.map((doc) => (
            <section className="terms__doc" key={doc.url}>
              <h3 className="terms__title">{doc.title}</h3>
              <p>{doc.intro}</p>
              {doc.sections.map((section) => (
                <div key={section.title}>
                  <h4 className="terms__heading">{section.title}</h4>
                  <p>{section.body}</p>
                </div>
              ))}
              <a
                className="terms__link"
                href={doc.url}
                onClick={(e) => {
                  e.preventDefault();
                  void open(doc.url);
                }}
              >
                Read it on superpeople.dev
              </a>
            </section>
          ))}
        </div>

        <label className={`terms__agree${atEnd ? "" : " is-locked"}`}>
          <input
            type="checkbox"
            className="terms__check"
            checked={agreed}
            disabled={!atEnd || saving}
            onChange={(e) => setAgreed(e.target.checked)}
          />
          <span>I have read and accept the Terms of Service and the Privacy Policy.</span>
        </label>
        {!atEnd && <p className="field__hint terms__scroll">Scroll to the end to continue.</p>}
        {problem && <p className="field__hint is-warn terms__problem">{problem}</p>}

        <div className="dialog__actions">
          <button type="button" className="btn" disabled={saving} onClick={close}>
            Cancel
          </button>
          <button type="button" className="btn btn--primary" disabled={!agreed || saving} onClick={() => void accept()}>
            {saving ? "Saving..." : "Accept"}
          </button>
        </div>
      </div>
    </div>
  );
}
