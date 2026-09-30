import { useCallback, useEffect, useState } from "react";
import { applyVote, castVote, listItems, type Board } from "../../lib/community";
import type { CommunityItem, Vote } from "../../types";

/** A page's items, with votes shown at once and put back if the website
 * refuses them. */
export function useBoard(board: Board, onError: (message: string) => void) {
  const [items, setItems] = useState<CommunityItem[] | null>(null);

  const load = useCallback(() => {
    listItems(board)
      .then(setItems)
      .catch((e) => {
        setItems((list) => list ?? []);
        onError(String(e));
      });
  }, [board, onError]);

  useEffect(load, [load]);

  const replace = useCallback((next: CommunityItem) => {
    setItems((list) => list?.map((i) => (i.id === next.id ? next : i)) ?? list);
  }, []);

  const vote = useCallback(
    (item: CommunityItem, v: Vote | null) => {
      // Taking a vote back is pressing the same arrow again.
      const direction = v ?? item.myVote;
      if (!direction) return;
      replace(applyVote(item, v));
      castVote(item.id, direction)
        .then(({ score, myVote }) => replace({ ...item, score, myVote }))
        .catch((e) => {
          replace(item);
          onError(String(e));
        });
    },
    [replace, onError],
  );

  const add = useCallback((item: CommunityItem) => setItems((list) => [item, ...(list ?? [])]), []);

  return { items, vote, replace, add, reload: load };
}
