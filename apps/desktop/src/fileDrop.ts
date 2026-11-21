import { useEffect, useState, type ClipboardEvent, type DragEvent } from "react";
import type { AttachmentSnapshot } from "@thingmaker/contracts";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api } from "./ipc";
import { IS_REMOTE } from "./remote/mode";
import { useStore } from "./store";

function hasFiles(transfer: DataTransfer): boolean {
  return Array.from(transfer.types ?? []).includes("Files") || transfer.files?.length > 0;
}

export function droppedPaths(transfer: DataTransfer): string[] {
  if (IS_REMOTE) return [];
  return (transfer.getData?.("text/uri-list") ?? "").split(/\r?\n/).flatMap((line) => {
    if (!line.startsWith("file:")) return [];
    try {
      const url = new URL(line);
      const path = decodeURIComponent(url.pathname);
      if (url.hostname && url.hostname !== "localhost") return [`//${url.hostname}${path}`];
      return [path.replace(/^\/([A-Za-z]:\/)/, "$1")];
    } catch {
      return [];
    }
  });
}

function base64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  }
  return btoa(binary);
}

/** Files stay in attachment storage while the composer keeps the person's words. */
export function useFileDrop(onAttachments: (files: AttachmentSnapshot[]) => void, enabled: boolean) {
  const setError = useStore((state) => state.setError);
  const [dropping, setDropping] = useState(false);
  const [pending, setPending] = useState(0);

  const receive = async (operation: () => Promise<AttachmentSnapshot[]>) => {
    setPending((count) => count + 1);
    try {
      onAttachments(await operation());
    } catch (error) {
      setError(error);
    } finally {
      setPending((count) => count - 1);
    }
  };

  const attachFiles = (files: File[]) => receive(async () => {
    if (files.length > 8) throw new Error("At most eight attachments per prompt.");
    return Promise.all(files.map(async (file) => {
      if (file.size > 10 * 1024 * 1024) throw new Error(`${file.name}: attachment exceeds 10 MiB`);
      return api.attachmentAddBytes(file.name || "pasted-file", base64(await file.arrayBuffer()));
    }));
  });

  useEffect(() => {
    if (!enabled || IS_REMOTE) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    try {
      void getCurrentWebview().onDragDropEvent((event) => {
        if (disposed) return;
        if (event.payload.type === "enter" || event.payload.type === "over") setDropping(true);
        else if (event.payload.type === "leave") setDropping(false);
        else if (event.payload.type === "drop") {
          setDropping(false);
          setPending((count) => count + 1);
          void api.attachmentAddPaths(event.payload.paths)
            .then(onAttachments)
            .catch(setError)
            .finally(() => setPending((count) => count - 1));
        }
      }).then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      }).catch(() => undefined);
    } catch {
      // Browser clients use File objects from HTML drag events.
    }
    return () => { disposed = true; unlisten?.(); setDropping(false); };
  }, [enabled, onAttachments, setError]);

  const onDragOverCapture = (event: DragEvent<HTMLElement>) => {
    if (!hasFiles(event.dataTransfer) && !Array.from(event.dataTransfer.types ?? []).includes("text/uri-list")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = enabled ? "copy" : "none";
    if (enabled) setDropping(true);
  };
  const onDropCapture = (event: DragEvent<HTMLElement>) => {
    const paths = droppedPaths(event.dataTransfer);
    if (!hasFiles(event.dataTransfer) && paths.length === 0) return;
    event.preventDefault();
    event.stopPropagation();
    setDropping(false);
    if (!enabled) return;
    const files = Array.from(event.dataTransfer.files ?? []);
    if (files.length > 0) void attachFiles(files);
    else if (paths.length > 0) void receive(() => api.attachmentAddPaths(paths));
  };
  const onDragLeaveCapture = (event: DragEvent<HTMLElement>) => {
    if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setDropping(false);
  };
  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = Array.from(event.clipboardData?.files ?? []);
    if (files.length === 0) return;
    event.preventDefault();
    if (enabled) void attachFiles(files);
  };
  return { dropping: enabled && dropping, uploading: pending > 0, onPaste, handlers: { onDragOverCapture, onDropCapture, onDragLeaveCapture } };
}
