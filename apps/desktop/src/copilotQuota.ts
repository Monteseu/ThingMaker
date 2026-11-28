import { t } from "./i18n";

/** SDK bucket names shared by the account summary and usage bars. */
export function copilotQuotaLabel(kind: string): string {
  switch (kind) {
    case "premium_interactions": return t("Premium interactions");
    case "chat": return t("Chat messages");
    case "completions": return t("Code completions");
    case "premium_interactions_unlimited": return t("Unlimited premium interactions");
    case "chat_unlimited": return t("Unlimited chat messages");
    case "completions_unlimited": return t("Unlimited code completions");
    default: return kind.replace(/_/g, " ");
  }
}
