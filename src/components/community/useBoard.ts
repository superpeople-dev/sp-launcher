import { useCallback, useEffect, useState } from "react";
import { applyVote, cachedItems, castVote, loadItems, storeItems, type Board } from "../../lib/community";
import type { CommunityItem, Vote } from "../../types";

/** A page's items: what the launcher already has at once, then refreshed
 * quietly (lib/community.ts); votes show at once and are put back if the
 * website refuses them. */
export function useBoard(board: Board, onError: (message: string) => void) {
  const [items, setItems] = useState<CommunityItem[] | null>(() => cachedItems(board));

  useEffect(() => {
    let alive = true;
    loadItems(board)
      .then((fresh) => alive && setItems(fresh))
      .catch((e) => {
        if (!alive) return;
        // With something already on screen, a failed refresh says nothing.
        if (cachedItems(board) === null) {
          setItems((list) => list ?? []);
          onError(String(e));
        }
      });
    return () => {
      alive = false;
    };
  }, [board, onError]);

  // Votes and new comments made here are kept for the next visit too.
  useEffect(() => {
    if (items) storeItems(board, items);
  }, [board, items]);

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

  return { items, vote, replace };
}
