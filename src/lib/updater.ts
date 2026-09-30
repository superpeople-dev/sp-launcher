import { check, type Update } from "@tauri-apps/plugin-updater";

/**
 * Checks the endpoint configured in `tauri.conf.json`'s `plugins.updater`.
 * Resolves to `null` only when the launcher really is current, and throws on
 * anything else (endpoint down, 404, unparsable manifest). Deliberately does
 * not swallow failures: reporting a broken endpoint as "up to date" hides
 * exactly the problem the user needs to see. Callers decide whether to
 * surface it — the check on startup stays quiet, the Settings button doesn't.
 */
export async function checkForUpdate(): Promise<Update | null> {
  return await check();
}

/**
 * Downloads and installs the update. On Windows (the only target this app
 * ships for), `downloadAndInstall` launches the new NSIS installer and exits
 * the current process itself once it's handed off successfully — the
 * installer runs without a window (`installMode: "quiet"` in tauri.conf.json)
 * and then restarts the app, which brings itself to the front (App.tsx). So
 * there's nothing left to do after this resolves (if it resolves at all; a successful run typically ends the
 * process from underneath the caller).
 *
 * `onProgress` gets a running byte count as chunks arrive; the total isn't
 * always known ahead of time (depends on whether the server sends
 * Content-Length), so callers should treat it as "bytes so far", not a
 * percentage.
 */
export async function installUpdate(
  update: Update,
  onProgress?: (downloaded: number, total: number | null) => void
): Promise<void> {
  let downloaded = 0;
  let total: number | null = null;

  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        total = event.data.contentLength ?? null;
        onProgress?.(0, total);
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        onProgress?.(downloaded, total);
        break;
      case "Finished":
        onProgress?.(downloaded, total);
        break;
    }
  });
}
