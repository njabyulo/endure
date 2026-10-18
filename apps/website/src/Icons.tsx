import type { CSSProperties } from "react";

export function Mark({ className = "" }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 48 48"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M24 3 42.2 13.5v21L24 45 5.8 34.5v-21L24 3Z"
        stroke="currentColor"
        strokeWidth="1.6"
      />
      <path
        d="m24 11 11.3 6.5v13L24 37 12.7 30.5v-13L24 11Z"
        stroke="currentColor"
        strokeWidth="1.6"
      />
      <path
        d="m5.8 13.5 18.2 10.6 18.2-10.6M24 24v21M12.7 30.5 24 24l11.3 6.5M24 3v8"
        stroke="currentColor"
        strokeWidth="1.6"
      />
      <circle cx="24" cy="24" r="3" fill="currentColor" />
    </svg>
  );
}
export function Icon({
  name,
  size = 16,
  style,
}: {
  name:
    | "arrow"
    | "play"
    | "bolt"
    | "reset"
    | "copy"
    | "check"
    | "shield"
    | "menu"
    | "close"
    | "source";
  size?: number;
  style?: CSSProperties;
}) {
  const paths = {
    arrow: (
      <>
        <path d="M4 12h16M14 6l6 6-6 6" />
      </>
    ),
    play: <path d="m9 5 11 7-11 7V5Z" />,
    bolt: <path d="m13 2-9 12h7l-1 8 10-13h-7l1-7Z" />,
    reset: (
      <>
        <path d="M4 10a8 8 0 1 1 1 8M4 4v6h6" />
      </>
    ),
    copy: (
      <>
        <rect x="8" y="8" width="12" height="13" rx="2" />
        <path d="M16 8V3H3v13h5" />
      </>
    ),
    check: <path d="m5 12 4 4L19 6" />,
    shield: (
      <>
        <path d="m12 2 8 4v6c0 5-8 10-8 10S4 17 4 12V6l8-4Z" />
        <path d="m8 12 3 3 5-6" />
      </>
    ),
    menu: (
      <>
        <path d="M3 8h18M3 16h18" />
      </>
    ),
    source: (
      <>
        <circle cx="6" cy="5" r="2" />
        <circle cx="6" cy="19" r="2" />
        <circle cx="18" cy="6" r="2" />
        <path d="M6 7v10M18 8a8 8 0 0 1-8 8H6" />
      </>
    ),
    close: (
      <>
        <path d="m6 6 12 12M6 18 18 6" />
      </>
    ),
  };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={style}
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}
