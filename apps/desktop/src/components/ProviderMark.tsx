/**
 * A provider's mark at badge size: Claude Code's pixel mascot, Codex's cloud with a
 * prompt, Antigravity's arch. Drawn as glyphs rather than the app-icon tiles
 * (`ProviderLogo`), so they sit on the dark theme without a coloured square
 * around each, and read as one set: same box, same weight, each in its own
 * colour.
 */
import { useId } from "react";
import { PROVIDER_LABELS, type Provider } from "@thingmaker/contracts";

/** GitHub Copilot glyph from Simple Icons (CC0), shared by marks and tiles. */
export const COPILOT_LOGO_PATH = "M23.922 16.997C23.061 18.492 18.063 22.02 12 22.02 5.937 22.02.939 18.492.078 16.997A.641.641 0 0 1 0 16.741v-2.869a.883.883 0 0 1 .053-.22c.372-.935 1.347-2.292 2.605-2.656.167-.429.414-1.055.644-1.517a10.098 10.098 0 0 1-.052-1.086c0-1.331.282-2.499 1.132-3.368.397-.406.89-.717 1.474-.952C7.255 2.937 9.248 1.98 11.978 1.98c2.731 0 4.767.957 6.166 2.093.584.235 1.077.546 1.474.952.85.869 1.132 2.037 1.132 3.368 0 .368-.014.733-.052 1.086.23.462.477 1.088.644 1.517 1.258.364 2.233 1.721 2.605 2.656a.841.841 0 0 1 .053.22v2.869a.641.641 0 0 1-.078.256Zm-11.75-5.992h-.344a4.359 4.359 0 0 1-.355.508c-.77.947-1.918 1.492-3.508 1.492-1.725 0-2.989-.359-3.782-1.259a2.137 2.137 0 0 1-.085-.104L4 11.746v6.585c1.435.779 4.514 2.179 8 2.179 3.486 0 6.565-1.4 8-2.179v-6.585l-.098-.104s-.033.045-.085.104c-.793.9-2.057 1.259-3.782 1.259-1.59 0-2.738-.545-3.508-1.492a4.359 4.359 0 0 1-.355-.508Zm2.328 3.25c.549 0 1 .451 1 1v2c0 .549-.451 1-1 1-.549 0-1-.451-1-1v-2c0-.549.451-1 1-1Zm-5 0c.549 0 1 .451 1 1v2c0 .549-.451 1-1 1-.549 0-1-.451-1-1v-2c0-.549.451-1 1-1Zm3.313-6.185c.136 1.057.403 1.913.878 2.497.442.544 1.134.938 2.344.938 1.573 0 2.292-.337 2.657-.751.384-.435.558-1.15.558-2.361 0-1.14-.243-1.847-.705-2.319-.477-.488-1.319-.862-2.824-1.025-1.487-.161-2.192.138-2.533.529-.269.307-.437.808-.438 1.578v.021c0 .265.021.562.063.893Zm-1.626 0c.042-.331.063-.628.063-.894v-.02c-.001-.77-.169-1.271-.438-1.578-.341-.391-1.046-.69-2.533-.529-1.505.163-2.347.537-2.824 1.025-.462.472-.705 1.179-.705 2.319 0 1.211.175 1.926.558 2.361.365.414 1.084.751 2.657.751 1.21 0 1.902-.394 2.344-.938.475-.584.742-1.44.878-2.497Z";

function Claude() {
  // Claude Code's pixel mascot, on its own 32-unit grid: a body with two eye
  // holes, arms either side and four legs.
  return (
    <g fill="#d97757" transform="translate(0 -10)">
      <path d="M64 106H448V362H64ZM128 170V234H160V170ZM352 170V234H384V170Z" fillRule="evenodd" />
      <rect height="64" width="512" x="0" y="234" />
      <rect height="64" width="32" x="96" y="362" />
      <rect height="64" width="32" x="160" y="362" />
      <rect height="64" width="32" x="320" y="362" />
      <rect height="64" width="32" x="384" y="362" />
    </g>
  );
}

function Codex({ id }: { id: string }) {
  const bumps = Array.from({ length: 8 }, (_, index) => (index * Math.PI) / 4);
  return (
    <>
      <defs>
        <linearGradient id={id} x1="0" x2="1" y1="0" y2="1">
          <stop offset="0" stopColor="#97a4ff" />
          <stop offset="1" stopColor="#5d66f2" />
        </linearGradient>
      </defs>
      <g fill={`url(#${id})`}>
        <circle cx="12" cy="12" r="7" />
        {bumps.map((angle, index) => (
          <circle cx={12 + Math.cos(angle) * 5.8} cy={12 + Math.sin(angle) * 5.8} key={index} r="4.3" />
        ))}
      </g>
      <g fill="none" stroke="#14151c" strokeLinecap="round" strokeLinejoin="round" strokeWidth="2.4">
        <polyline points="7.8,9.2 10.8,12 7.8,14.8" />
        <line x1="12.8" x2="16.4" y1="15" y2="15" />
      </g>
    </>
  );
}

function Antigravity({ id }: { id: string }) {
  return (
    <>
      <defs>
        <linearGradient id={id} x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor="#f0544a" />
          <stop offset="0.3" stopColor="#f6b21b" />
          <stop offset="0.62" stopColor="#4c8df6" />
          <stop offset="1" stopColor="#3a74e8" />
        </linearGradient>
      </defs>
      <path
        d="M1.8 21C5.2 21 7.2 2.6 12 2.6C16.8 2.6 18.8 21 22.2 21C20.2 21 18.4 20 17.2 17.4C15.6 13.8 14.4 10.6 12 10.6C9.6 10.6 8.4 13.8 6.8 17.4C5.6 20 3.8 21 1.8 21Z"
        fill={`url(#${id})`}
      />
    </>
  );
}

/** Each provider's glyph. Exhaustive, so a new provider cannot fall back to
 *  another's mark. */
function glyph(provider: Provider, id: string) {
  switch (provider) {
    case "claude":
      return <Claude />;
    case "codex":
      return <Codex id={`${id}-codex`} />;
    case "gemini":
      return <Antigravity id={`${id}-agy`} />;
    case "copilot":
      // GitHub Copilot glyph from Simple Icons (CC0).
      return <path fill="#a879e8" d={COPILOT_LOGO_PATH} />;
    case "local":
      return <Local />;
    default: {
      const unknown: never = provider;
      return unknown;
    }
  }
}

/** Local Models: a chip, in the tile's green. */
function Local() {
  return (
    <g fill="none" stroke="#5fbf7a" strokeLinecap="round" strokeWidth="1.8">
      <rect height="11" rx="2" width="11" x="6.5" y="6.5" />
      <rect fill="#5fbf7a" height="4" rx="0.8" stroke="none" width="4" x="10" y="10" />
      <path d="M9.5 3.5v3M14.5 3.5v3M9.5 17.5v3M14.5 17.5v3M3.5 9.5h3M3.5 14.5h3M17.5 9.5h3M17.5 14.5h3" />
    </g>
  );
}

export function ProviderMark({ provider, size = 16 }: { provider: Provider; size?: number }) {
  // React's ids carry characters an SVG `url(#…)` reference cannot.
  const id = `pm${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  return (
    <svg aria-hidden="true" className="provider-mark-glyph" height={size} viewBox={provider === "claude" ? "0 0 512 512" : "0 0 24 24"} width={size}>
      {glyph(provider, id)}
    </svg>
  );
}

/** The mark in a small tile, for a row: the badge that replaces CC / CX. */
export function ProviderBadge({ provider, title }: { provider: Provider; title?: string }) {
  const label = provider === "gemini" ? "Gemini (Antigravity)" : PROVIDER_LABELS[provider];
  return (
    <span aria-label={label} className={`provider-mark provider-${provider}`} role="img" title={title ?? label}>
      <ProviderMark provider={provider} />
    </span>
  );
}
