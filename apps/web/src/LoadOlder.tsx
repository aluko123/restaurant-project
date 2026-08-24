import { useEffect, useRef } from "react";

/**
 * One consistent way to page every growing list in the app.
 *
 * Renders a real, focusable button (keyboard and screen-reader friendly)
 * that an IntersectionObserver also triggers automatically when it scrolls
 * into view — so lists extend seamlessly while staying accessible.
 */
export function LoadOlder({
  label,
  loadingLabel = "Loading…",
  loading = false,
  onMore,
}: {
  label: string;
  loadingLabel?: string;
  loading?: boolean;
  onMore: () => void;
}) {
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const moreRef = useRef(onMore);
  moreRef.current = onMore;
  const busyRef = useRef(loading);
  busyRef.current = loading;

  useEffect(() => {
    const node = buttonRef.current;
    if (!node) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting) && !busyRef.current) {
          moreRef.current();
        }
      },
      { rootMargin: "320px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="load-older">
      <button
        ref={buttonRef}
        type="button"
        className="text-button"
        disabled={loading}
        onClick={onMore}
      >
        {loading ? loadingLabel : label}
      </button>
    </div>
  );
}
