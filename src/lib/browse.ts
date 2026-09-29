import { open } from "@tauri-apps/plugin-dialog";

/**
 * Opens the OS folder picker for choosing the game's install directory.
 * Shared by the Download tab's own Browse button and the Settings tab's, so
 * picking a folder looks and behaves the same from either place. It opens in
 * `current` (the Game folder so far), so Browse shows where the game is.
 * Returns the picked path, or null if the user cancelled.
 */
export async function pickInstallFolder(current?: string): Promise<string | null> {
  const picked = await open({
    directory: true,
    multiple: false,
    title: "Choose install folder",
    defaultPath: current?.trim() || undefined,
  });
  return typeof picked === "string" ? picked : null;
}
