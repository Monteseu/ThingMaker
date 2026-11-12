import { useEffect, useState } from "react";
import { PROVIDERS, type NotificationSettings, type Provider, type ProviderModel } from "@thingmaker/contracts";
import { api } from "../ipc";
import { t } from "../i18n";

const LABELS: Record<Provider, string> = { claude: "Claude", codex: "ChatGPT / Codex", gemini: "Antigravity", local: "Local Models" };

export function ReplySuggestionSettings({ settings, onChange }: { settings: NotificationSettings; onChange: (patch: Partial<NotificationSettings>) => void }) {
  const provider = settings.replySuggestionProvider ?? "claude";
  const model = settings.replySuggestionModel ?? "";
  const [catalog, setCatalog] = useState<{ provider: Provider; models: ProviderModel[]; problem?: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    setCatalog(null);
    void api.providerModels(provider).then((models) => {
      if (!cancelled) setCatalog({ provider, models: models.filter((choice) => provider !== "local" || !choice.id.startsWith("@agent/")) });
    }).catch((error: unknown) => {
      if (!cancelled) setCatalog({ provider, models: [], problem: error instanceof Error ? error.message : String(error) });
    });
    return () => { cancelled = true; };
  }, [provider]);
  const current = catalog?.provider === provider ? catalog : null;
  return <>
    <label className="field">
      <span>{t("Reply suggestion provider")}</span>
      <select className="input" value={provider} onChange={(event) => onChange({ replySuggestionProvider: event.target.value as Provider, replySuggestionModel: "" })}>
        {PROVIDERS.map((choice) => <option key={choice} value={choice}>{LABELS[choice]}</option>)}
      </select>
    </label>
    <label className="field">
      <span>{t("Reply suggestion model")}</span>
      <select className="input" disabled={!current} value={model} onChange={(event) => onChange({ replySuggestionProvider: provider, replySuggestionModel: event.target.value })}>
        <option value="">{current ? t("Disabled") : t("Loading models…")}</option>
        {model && !current?.models.some((choice) => choice.id === model) && <option value={model}>{model}</option>}
        {current?.models.map((choice) => <option key={choice.id} value={choice.id}>{choice.name}{choice.needsCredits ? " · " + t("uses credits") : ""}</option>)}
      </select>
      <span className="small muted">{t("Uses only the last AI reply, your last message and the previous AI reply. Tab accepts without sending; Esc dismisses.")}</span>
      {current?.problem && <span className="small muted" role="status">{t("Models unavailable: {problem}", { problem: current.problem })}</span>}
      {current && !current.problem && current.models.length === 0 && <span className="small muted">{t("No models available for this provider.")}</span>}
    </label>
  </>;
}
