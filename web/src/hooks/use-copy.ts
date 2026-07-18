import { useCallback, useRef, useState } from "react";

import { copyText } from "@/lib/copy";

export function useCopy(durationMs = 1600) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);

  const copy = useCallback(
    async (text: string) => {
      await copyText(text);
      setCopied(true);
      if (timer.current != null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => {
        setCopied(false);
        timer.current = null;
      }, durationMs);
    },
    [durationMs],
  );

  return { copied, copy };
}
