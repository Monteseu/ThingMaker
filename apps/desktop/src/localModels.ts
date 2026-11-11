/**
 * Local Models in the renderer: telling a local chat from a local agent, the
 * words for each, and whether a team keeps its work on this computer.
 * Pure functions; the screens are the Providers card, the model menu and the
 * team chart.
 */
import { parseLocalModel, type LocalFit, type LocalKind, type Provider } from "@thingmaker/contracts";
import { t } from "./i18n";

/** The agent name ThingMaker's own chat agent reports. */
export const LOCAL_CHAT_AGENT = "thingmaker-local-chat";

/** Whether a session is a local chat: no tools, no team, no Big Thing. */
export function isLocalChat(snapshot: { provider?: Provider; capabilities?: { agentName?: string } | null } | undefined): boolean {
  return snapshot?.provider === "local" && snapshot.capabilities?.agentName === LOCAL_CHAT_AGENT;
}

export function isLocalModelId(id: string | undefined): boolean {
  return !!id && (id.startsWith("@chat/") || id.startsWith("@agent/"));
}

export function localKindLabel(kind: LocalKind): string {
  return kind === "chat" ? t("Chat") : t("Agent");
}

export function localKindNote(kind: LocalKind): string {
  return kind === "chat" ? t("Just talk: fast, no tools, nothing leaves this computer.") : t("Works on the code in a sandbox that reaches only this computer. Slower to start.");
}

/** A local model's name without how it runs. */
export function localName(id: string | undefined): string {
  return parseLocalModel(id).name;
}

/** The catalog's description without its leading "Chat · " or "Agent · ". */
export function localFacts(description: string | undefined): string | undefined {
  return description?.replace(/^(Chat|Agent) · /, "");
}

export function fitLabel(fit: LocalFit | null | undefined): string | null {
  switch (fit) {
    case "fits":
      return t("Fits");
    case "tight":
      return t("Tight");
    case "too_big":
      return t("Too big");
    default:
      return null;
  }
}

/** A helper's privacy under a lead: a local helper working for a cloud lead
 *  is free but not private, because its task and results pass through the
 *  cloud lead. */
export function freeNotPrivate(lead: Provider, helper: Provider): boolean {
  return helper === "local" && lead !== "local";
}

/** A window in tokens as people say it: 32768 → "32k". */
export function windowLabel(tokens: number): string {
  return tokens >= 1024 ? `${Math.round(tokens / 1024)}k` : String(tokens);
}

export function bytes(size: number): string {
  return size >= 1e9 ? `${(size / 1e9).toFixed(1)} GB` : `${Math.round(size / 1e6)} MB`;
}
