import { useEffect, useState } from "react";
import { LIMITS, editItem, loadMeta } from "../../lib/community";
import type { Choice, CommunityItem } from "../../types";
import { Picker } from "./Picker";

interface Props {
  item: CommunityItem;
  onClose: () => void;
  /** Saved on the website; the tags follow when the pages load again. */
  onSaved: (fields: Pick<CommunityItem, "title" | "description" | "typeId" | "platformId">) => void;
  onError: (message: string) => void;
}

const NONE = "none";

/** An admin's edit of an item, as on the website: its title, details, type
 * and platform. */
export function EditDialog({ item, onClose, onSaved, onError }: Props) {
  const [title, setTitle] = useState(item.title);
  const [description, setDescription] = useState(item.description);
  const [typeId, setTypeId] = useState(item.typeId ?? NONE);
  const [platformId, setPlatformId] = useState(item.platformId ?? NONE);
  const [types, setTypes] = useState<Choice[]>([]);
  const [platforms, setPlatforms] = useState<Choice[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    loadMeta()
      .then((meta) => {
        setTypes(meta.types);
        setPlatforms(meta.platforms);
      })
      .catch(() => {});
  }, []);

  const [min] = LIMITS.title;
  const ready = title.trim().length >= min && !busy;

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    const fields = {
      title: title.trim(),
      description: description.trim(),
      typeId: typeId === NONE ? null : typeId,
      platformId: platformId === NONE ? null : platformId,
    };
    try {
      await editItem(item.id, { title: fields.title, description: fields.description, typeId: fields.typeId, categoryId: fields.platformId });
      onSaved(fields);
    } catch (e) {
      onError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="edit-title" onMouseDown={onClose}>
      <form
        className="dialog__card"
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <h2 className="card__title" id="edit-title">
          Edit post
        </h2>
        <div className="dialog__pickers">
          <div className="field">
            <span className="field__label">Type</span>
            <Picker
              value={typeId}
              onChange={setTypeId}
              options={[{ id: NONE, label: "No type" }, ...types.map((t) => ({ id: t.id, label: t.name }))]}
            />
          </div>
          <div className="field">
            <span className="field__label">Platform</span>
            <Picker
              value={platformId}
              onChange={setPlatformId}
              options={[{ id: NONE, label: "No platform" }, ...platforms.map((p) => ({ id: p.id, label: p.name }))]}
            />
          </div>
        </div>
        <label className="field">
          <span className="field__label">
            Title <span className="field__count">{title.length}/{LIMITS.titleAdmin}</span>
          </span>
          <input className="input" value={title} maxLength={LIMITS.titleAdmin} autoFocus onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label className="field">
          <span className="field__label">
            Details <span className="field__count">{description.length}/{LIMITS.description}</span>
          </span>
          <textarea
            className="input dialog__text"
            value={description}
            maxLength={LIMITS.description}
            rows={8}
            onChange={(e) => setDescription(e.target.value)}
          />
        </label>
        <div className="dialog__actions">
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" className="btn btn--primary" disabled={!ready}>
            {busy ? "Saving…" : "Save changes"}
          </button>
        </div>
      </form>
    </div>
  );
}
