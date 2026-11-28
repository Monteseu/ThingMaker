/**
 * A provider's logo as a small app-icon tile. PNGs and the inline Copilot SVG
 * share a rounded-square mask so they read as a set wherever they appear.
 */
import { PROVIDER_LABELS, type Provider } from "@thingmaker/contracts";
import { t } from "../i18n";
import { COPILOT_LOGO_PATH } from "./ProviderMark";

export function ProviderLogo({ provider, size = 40 }: { provider: Provider; size?: number }) {
  if (provider === "copilot") {
    return (
      <svg
        aria-label={t("{provider} logo", { provider: PROVIDER_LABELS[provider] })}
        className="provider-logo"
        height={size}
        role="img"
        style={{ borderRadius: Math.round(size * 0.225) }}
        viewBox="0 0 24 24"
        width={size}
      >
        <rect width="24" height="24" fill="#0d1117" />
        <g transform="translate(2.4 2.4) scale(0.8)">
          <path d={COPILOT_LOGO_PATH} fill="#ffffff" />
        </g>
      </svg>
    );
  }

  return (
    <img
      alt={t("{provider} logo", { provider: PROVIDER_LABELS[provider] })}
      className="provider-logo"
      draggable={false}
      height={size}
      src={`${import.meta.env.BASE_URL}providers/${provider}.png`}
      style={{ borderRadius: Math.round(size * 0.225) }}
      width={size}
    />
  );
}
