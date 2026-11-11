/**
 * The composer's line for a Local Models session: how private it is, and,
 * in a chat, a way on to an agent on the same model.
 */
import { asConfigOptions, localModelId } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { t } from "../i18n";

export function LocalBadge({ sessionId, localChat, configOptions }: { sessionId: string; localChat: boolean; configOptions: unknown }) {
  const catalog = useStore((s) => s.providerModels.local);
  const pending = useStore((s) => s.pendingProvider[sessionId]);
  const setPending = useStore((s) => s.setPendingProvider);
  const current = asConfigOptions(configOptions).find((option) => option.configId === "model")?.currentValue;
  const name = typeof current === "string" ? current : "";
  const agent = localModelId("agent", name);
  const canBeAgent = localChat && name !== "" && (catalog ?? []).some((model) => model.id === agent);

  const toAgent = async () => {
    const confirmed = await api.confirmDialog({
      title: t("Continue as an agent?"),
      message: t("Your next message starts an agent on {model}, with this conversation carried over. Unlike a chat, an agent can read and change files in this folder. It runs in a sandbox that reaches only this computer.", { model: name }),
      okLabel: t("Continue as an agent"),
      cancelLabel: t("Stay in the chat"),
    });
    if (confirmed) setPending(sessionId, { provider: "local", model: agent });
  };

  return (
    <>
      <span className="chip small chip-private" title={localChat ? t("This chat only talks to Ollama on this computer") : t("This agent runs in a sandbox that can only reach this computer")}>
        {localChat ? t("Private · on this computer · free") : t("Private · sandboxed · free")}
      </span>
      {canBeAgent && !pending && (
        <button className="button button-small" onClick={() => void toAgent()} title={t("Work on the code with this model, as an agent")} type="button">
          {t("Continue as an agent")}
        </button>
      )}
      {pending?.provider === "local" && pending.model === agent && (
        <button className="button button-small" onClick={() => setPending(sessionId, null)} type="button">
          {t("Stay in the chat")}
        </button>
      )}
    </>
  );
}
