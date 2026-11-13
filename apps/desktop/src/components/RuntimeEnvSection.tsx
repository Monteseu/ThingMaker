/**
 * Secrets: runtime environment variables for agent launches (SEC-11, F13). Values are
 * written to the OS keychain once and never displayed again; only the names
 * are listed. They are injected into new Kit helpers and terminals on top of
 * the documented launch profile.
 */
import { useEffect, useState } from "react";
import type { RuntimeEnvInfo } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { t } from "../i18n";
import { Fold, FoldItem } from "./settings/controls";

export function RuntimeEnvSection() {
  const setError = useStore((s) => s.setError);
  const [info, setInfo] = useState<RuntimeEnvInfo | null>(null);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);

  const load = () => api.runtimeEnvList().then(setInfo).catch(setError);
  useEffect(() => {
    void load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const add = async () => {
    setBusy(true);
    try {
      const names = await api.runtimeEnvSet(name.trim(), value);
      setInfo((current) => (current ? { ...current, names } : current));
      setName("");
      setValue("");
    } catch (error) {
      setError(error);
    } finally {
      setBusy(false);
    }
  };

  const remove = async (n: string) => {
    try {
      const names = await api.runtimeEnvRemove(n);
      setInfo((current) => (current ? { ...current, names } : current));
    } catch (error) {
      setError(error);
    }
  };

  return (
    <Fold
      hint={t("API keys MCP servers and skills need, kept in the {backend}", { backend: info?.backend ?? t("OS keychain") })}
      name={t("Secrets")}
      summary={info ? (info.names.length === 1 ? t("1 secret") : t("{n} secrets", { n: info.names.length })) : ""}
    >
      {info && !info.available && <p className="set-item-note">{t("Secure storage for secrets is not available on this platform in this release.")}</p>}
      {info?.names.map((n) => (
        <FoldItem
          actions={
            <button className="set-link set-link-danger" onClick={() => void remove(n)} type="button">
              {t("Remove")}
            </button>
          }
          key={n}
        >
          <span className="set-item-text mono">{n}</span>
          <span className="set-item-note">••••••••</span>
        </FoldItem>
      ))}
      <form
        className="set-inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim() && value) void add();
        }}
      >
        <input aria-label={t("Secret name")} autoCapitalize="characters" className="set-input mono" disabled={!info?.available} onChange={(e) => setName(e.target.value)} placeholder="OPENAI_API_KEY" spellCheck={false} value={name} />
        <input aria-label={t("Secret value")} autoComplete="off" className="set-input" disabled={!info?.available} onChange={(e) => setValue(e.target.value)} placeholder={t("value, never shown again")} type="password" value={value} />
        <button className="set-button" disabled={busy || !name.trim() || !value || !info?.available} type="submit">
          {info?.names.includes(name.trim()) ? t("Replace") : t("Add")}
        </button>
      </form>
      <p className="set-item-note">
        {t("Agents and the terminal start with only PATH, locale and proxies. Write {ref} in a server's config to use a secret; it applies to sessions started afterwards.", { ref: "${NAME}" })}
      </p>
    </Fold>
  );
}
