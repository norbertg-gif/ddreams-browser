import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type FullImageInfo = import("../bindings/FullImageInfo").FullImageInfo;

type FullFrame = Pick<FullImageInfo, "width" | "height" | "originalWidth" | "originalHeight" | "source"> & {
  url: string;
};

const settleDelay = 300;

export function createFullImageQueue() {
  let pending = Promise.resolve();
  return () => {
    const previous = pending;
    let release!: () => void;
    pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    return previous.then(() => release);
  };
}

export function useFullImage(
  fileKey: string | null,
  assetId: number | null,
  ready: boolean,
  rawDecode: boolean,
  queue?: ReturnType<typeof createFullImageQueue>,
) {
  const [displayed, setDisplayed] = useState<{ fileKey: string; frame: FullFrame } | null>(null);
  const [pending, setPending] = useState<{ fileKey: string; rawDecode: boolean } | null>(null);
  const displayedUrl = displayed?.frame.url;
  useEffect(
    () => () => {
      if (displayedUrl) URL.revokeObjectURL(displayedUrl);
    },
    [displayedUrl],
  );

  useEffect(() => {
    setDisplayed((current) => (current?.fileKey === fileKey ? current : null));
    setPending(null);
    if (fileKey === null || assetId === null || !ready) return;
    const request = { fileKey, rawDecode };
    const args = { assetId, rawDecode };
    let active = true;
    let timer: number;
    let release: (() => void) | undefined;

    const load = async () => {
      setPending(request);
      let url: string | undefined;
      let retry = false;
      try {
        const info = await invoke<FullImageInfo | null>("prepare_full_image", args);
        if (!active) return;
        if (!info) {
          // A previous decode may still be finishing. Only the visible request retries.
          retry = true;
          timer = window.setTimeout(load, settleDelay);
          return;
        }
        const parts: ArrayBuffer[] = [];
        for (let offset = 0; offset < info.length;) {
          const bytes = await invoke<ArrayBuffer>("read_full_image_chunk", { ...args, key: info.key, offset });
          if (!active) return;
          if (bytes.byteLength !== Math.min(info.chunkSize, info.length - offset))
            throw new Error("Image file changed");
          parts.push(bytes);
          offset += bytes.byteLength;
        }
        url = URL.createObjectURL(new Blob(parts, { type: info.contentType }));
        const image = new Image();
        image.src = url;
        await image.decode();
        if (!active) return;
        const { naturalWidth: width, naturalHeight: height } = image;
        const isOriginal = info.source === "original";
        setDisplayed({
          fileKey,
          frame: {
            url,
            width,
            height,
            source: info.source,
            originalWidth: isOriginal ? width : info.originalWidth,
            originalHeight: isOriginal ? height : info.originalHeight,
          },
        });
        url = undefined;
      } catch (error) {
        if (active) console.warn("Plné rozlíšenie sa nepodarilo načítať", error);
      } finally {
        if (url) URL.revokeObjectURL(url);
        if (!retry) {
          release?.();
          if (active) setPending(null);
        }
      }
    };

    timer = window.setTimeout(async () => {
      if (queue) release = await queue();
      if (active) await load();
      else release?.();
    }, settleDelay);
    return () => {
      active = false;
      window.clearTimeout(timer);
      release?.();
    };
  }, [fileKey, assetId, ready, rawDecode, queue]);

  return {
    frame: displayed?.fileKey === fileKey ? displayed.frame : null,
    loading: pending?.fileKey === fileKey && pending.rawDecode === rawDecode,
  };
}
