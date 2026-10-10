import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface GpuCrash {
  kind: "device_lost" | "render_hang" | "video_memory";
  directx12: boolean;
}

interface Props {
  crash: GpuCrash;
  onClose: () => void;
}

const WHAT: Record<GpuCrash["kind"], string> = {
  device_lost: "Your graphics card stopped responding, and Windows reset its driver, which ends the game.",
  render_hang: "Your graphics card stopped answering for two minutes, so the game had to close.",
  video_memory: "The game ran out of graphics card memory.",
};

/**
 * The game closed on a graphics-card crash (gpu_crash.rs): "GPU Crashed or D3D Device Removed",
 * "timed out waiting for RenderThread" or "Out of video memory". Nothing in the game's files is
 * wrong; on DirectX 12 the window offers DirectX 11, the renderer that crashes least.
 */
export function GpuCrashDialog({ crash, onClose }: Props) {
  const [state, setState] = useState<"ask" | "done">("ask");
  const [failed, setFailed] = useState("");

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onClose]);

  const switchRenderer = () => {
    setFailed("");
    void invoke("switch_to_directx11")
      .then(() => setState("done"))
      .catch((e) => setFailed(String(e)));
  };

  return (
    <div className="dialog" role="dialog" aria-modal aria-labelledby="gpu-title" onMouseDown={onClose}>
      <div className="dialog__card sac" onMouseDown={(e) => e.stopPropagation()}>
        <h2 className="card__title" id="gpu-title">
          The game closed because of your graphics card
        </h2>
        <p className="field__hint replay__text">{WHAT[crash.kind]} This comes from the graphics driver, not from the game's files.</p>
        {state === "done" ? (
          <p className="field__hint replay__text">
            Done: the game starts with <b>DirectX 11</b> from now on. You can change it back in the game under{" "}
            <b>Settings &gt; Graphics &gt; DirectX Version</b>.
          </p>
        ) : (
          <>
            {crash.directx12 && (
              <p className="field__hint replay__text">
                The game ran with <b>DirectX 12</b>, where these crashes happen most. <b>DirectX 11</b> is more stable on
                most PCs. NVIDIA DLSS and Intel XeSS need DirectX 12, so they are off with it; AMD FSR still works.
              </p>
            )}
            <ul className="sac__steps">
              {crash.kind === "video_memory" && (
                <li>
                  Lower <b>Texture Quality</b> or the resolution in Settings &gt; Graphics, and close other programs that use
                  the graphics card.
                </li>
              )}
              <li>Update your graphics driver.</li>
              <li>Turn off overclocking or undervolting of the graphics card if you use it.</li>
            </ul>
          </>
        )}
        {failed && <p className="field__hint is-warn replay__text">{failed}</p>}

        <div className="dialog__actions">
          <button type="button" className="btn" onClick={onClose} autoFocus={state === "done" || !crash.directx12}>
            {state === "done" || !crash.directx12 ? "Close" : "Keep DirectX 12"}
          </button>
          {state === "ask" && crash.directx12 && (
            <button type="button" className="btn btn--primary" autoFocus onClick={switchRenderer}>
              Switch to DirectX 11
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
