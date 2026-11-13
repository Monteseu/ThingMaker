/**
 * MCP servers in Settings › Integrations: what each provider already has,
 * and, when asked for, a short catalog to install from in one click, a box to
 * paste any config or command into, and a test start before anything is
 * written.
 *
 * Installing runs each provider's own `mcp add`, so the server lands where
 * that provider reads it — in ThingMaker's sessions and in the provider's own
 * app alike — and applies to sessions started afterwards.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { PROVIDER_LABELS, type InstalledMcpServer, type McpCatalogEntry, type McpInstallOutcome, type McpOverview, type McpProbe, type McpScope, type McpServerSpec, type Provider } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { t } from "../i18n";
import { ProviderBadge, ProviderMark } from "./ProviderMark";
import { Fold, FoldItem } from "./settings/controls";

const PROVIDER_NAME: Record<Provider, string> = { ...PROVIDER_LABELS, gemini: "Antigravity (Gemini)" };

/** A server as editable text: the command line or the URL, and its pairs. */
type Draft = { name: string; kind: "stdio" | "http"; target: string; pairs: { key: string; value: string }[] };

function quote(word: string): string {
  return /[\s"']/.test(word) ? `"${word.replace(/"/g, '\\"')}"` : word;
}

function splitWords(text: string): string[] {
  const words: string[] = [];
  let current = "";
  let quoteChar: string | null = null;
  let any = false;
  for (const c of text) {
    if (quoteChar) {
      if (c === quoteChar) quoteChar = null;
      else current += c;
    } else if (c === '"' || c === "'") {
      quoteChar = c;
      any = true;
    } else if (/\s/.test(c)) {
      if (current || any) words.push(current);
      current = "";
      any = false;
    } else current += c;
  }
  if (current || any) words.push(current);
  return words;
}

function toDraft(spec: McpServerSpec): Draft {
  if (spec.transport.type === "http") {
    return { name: spec.name, kind: "http", target: spec.transport.url, pairs: Object.entries(spec.transport.headers).map(([key, value]) => ({ key, value })) };
  }
  return {
    name: spec.name,
    kind: "stdio",
    target: [spec.transport.command, ...spec.transport.args].map(quote).join(" "),
    pairs: Object.entries(spec.transport.env).map(([key, value]) => ({ key, value })),
  };
}

function fromDraft(draft: Draft): McpServerSpec {
  const pairs = Object.fromEntries(draft.pairs.filter((pair) => pair.key.trim()).map((pair) => [pair.key.trim(), pair.value]));
  if (draft.kind === "http") return { name: draft.name.trim(), transport: { type: "http", url: draft.target.trim(), headers: pairs } };
  const [command = "", ...args] = splitWords(draft.target.trim());
  return { name: draft.name.trim(), transport: { type: "stdio", command, args, env: pairs } };
}

/** `${NAME}` references in a draft's values. */
function referencesOf(draft: Draft): string[] {
  const names = new Set<string>();
  for (const text of [draft.target, ...draft.pairs.map((pair) => pair.value)]) for (const match of text.matchAll(/\$\{([A-Za-z0-9_]+)\}/g)) names.add(match[1] as string);
  return [...names];
}

function InstallCard({
  initial,
  overview,
  workspaceId,
  runtimeNames,
  needs,
  onDone,
  onCancel,
}: {
  initial: McpServerSpec;
  overview: McpOverview;
  workspaceId: string | null;
  runtimeNames: string[];
  needs?: string | undefined;
  onDone: () => void;
  onCancel: () => void;
}) {
  const setError = useStore((s) => s.setError);
  const [draft, setDraft] = useState<Draft>(() => toDraft(initial));
  const available = overview.providers.filter((entry) => entry.available).map((entry) => entry.provider);
  const [providers, setProviders] = useState<Provider[]>(() => available.filter((provider) => provider !== "gemini"));
  const [scope, setScope] = useState<McpScope>("user");
  const [probe, setProbe] = useState<{ busy: boolean; result?: McpProbe; error?: string }>({ busy: false });
  const [installing, setInstalling] = useState(false);
  const [outcomes, setOutcomes] = useState<McpInstallOutcome[] | null>(null);

  const references = referencesOf(draft);
  const missing = references.filter((name) => !runtimeNames.includes(name));
  const codexUnsupported = draft.kind === "http" && draft.pairs.some((pair) => pair.key.trim() && !(pair.key.trim().toLowerCase() === "authorization" && /^Bearer \$\{[A-Za-z0-9_]+\}$/.test(pair.value.trim())));
  const nameOk = /^[A-Za-z0-9_-]{1,64}$/.test(draft.name.trim());
  const exists = overview.servers.some((server) => server.name === draft.name.trim());

  const patch = (change: Partial<Draft>) => {
    setDraft({ ...draft, ...change });
    setProbe({ busy: false });
  };

  const test = async () => {
    setProbe({ busy: true });
    try {
      setProbe({ busy: false, result: await api.mcpTest(workspaceId, fromDraft(draft)) });
    } catch (error) {
      setProbe({ busy: false, error: (error as { message?: string }).message ?? String(error) });
    }
  };

  const install = async () => {
    setInstalling(true);
    try {
      const results = await api.mcpInstall(workspaceId, fromDraft(draft), providers, scope);
      setOutcomes(results);
      if (results.every((result) => result.ok)) onDone();
    } catch (error) {
      setError(error);
    } finally {
      setInstalling(false);
    }
  };

  return (
    <div className="card mcp-install">
      <div className="row wrap">
        <label className="mcp-field">
          <span className="small">{t("Name")}</span>
          <input aria-label={t("Server name")} className="input mono" onChange={(event) => patch({ name: event.target.value })} value={draft.name} />
        </label>
        <label className="mcp-field">
          <span className="small">{t("Runs as")}</span>
          <select aria-label={t("Runs as")} className="select" onChange={(event) => patch({ kind: event.target.value as Draft["kind"] })} value={draft.kind}>
            <option value="stdio">{t("a program (stdio)")}</option>
            <option value="http">{t("a URL (HTTP)")}</option>
          </select>
        </label>
      </div>
      {!nameOk && <p className="small chip-warn">{t("Letters, digits, - and _ only.")}</p>}
      {exists && <p className="small muted">{t("A server with this name exists already; installing replaces it for the providers you choose.")}</p>}
      <label className="mcp-field">
        <span className="small">{draft.kind === "http" ? "URL" : t("Command")}</span>
        <input aria-label={draft.kind === "http" ? t("Server URL") : t("Server command")} className="input mono" onChange={(event) => patch({ target: event.target.value })} value={draft.target} />
      </label>
      <div className="mcp-pairs">
        <span className="small">{draft.kind === "http" ? t("Headers") : t("Environment")}</span>
        {draft.pairs.map((pair, index) => (
          <div className="row" key={index}>
            <input aria-label={t("Key")} className="input mono" onChange={(event) => patch({ pairs: draft.pairs.map((entry, at) => (at === index ? { ...entry, key: event.target.value } : entry)) })} placeholder={draft.kind === "http" ? "Authorization" : "API_KEY"} value={pair.key} />
            <input aria-label={t("Value")} className="input mono" onChange={(event) => patch({ pairs: draft.pairs.map((entry, at) => (at === index ? { ...entry, value: event.target.value } : entry)) })} placeholder={draft.kind === "http" ? "Bearer ${TOKEN}" : "${API_KEY}"} value={pair.value} />
            <button aria-label={t("Remove")} className="button button-small" onClick={() => patch({ pairs: draft.pairs.filter((_, at) => at !== index) })} type="button">
              ×
            </button>
          </div>
        ))}
        <button className="link small" onClick={() => patch({ pairs: [...draft.pairs, { key: "", value: "" }] })} type="button">
          {draft.kind === "http" ? t("+ add a header") : t("+ add a variable")}
        </button>
        <span className="small muted">
          {t("Keep secrets out of config files: store them under")} <strong>{t("Secrets")}</strong> {t("below and write")}{" "}
          <span className="mono">{"${NAME}"}</span> {t("here. Claude Code reads the reference itself; for Codex the variable is forwarded from the session.")}
        </span>
        {missing.length > 0 && <span className="small chip-warn">{t("Not in the runtime environment yet: {names}.", { names: missing.join(", ") })}</span>}
      </div>

      <div className="mcp-targets">
        <span className="small">{t("Install for")}</span>
        {overview.providers.map(({ provider, available: ok, problem }) => {
          const blocked = !ok || (provider === "codex" && codexUnsupported);
          return (
            <label className="odyssey-toggle" key={provider} title={problem ?? undefined}>
              <input
                checked={providers.includes(provider) && !blocked}
                disabled={blocked}
                onChange={(event) => setProviders(event.target.checked ? [...providers, provider] : providers.filter((entry) => entry !== provider))}
                type="checkbox"
              />
              <ProviderBadge provider={provider} />
              <span className="small">
                {PROVIDER_NAME[provider]}
                {!ok && <span className="muted"> — {t("not installed")}</span>}
                {ok && provider === "codex" && codexUnsupported && <span className="muted"> — {t("takes HTTP credentials only as {header}", { header: "Authorization: Bearer ${VARIABLE}" })}</span>}
                {ok && provider === "gemini" && <span className="muted"> — {t("Antigravity reads values as written")}</span>}
              </span>
            </label>
          );
        })}
        {providers.includes("claude") && workspaceId && (
          <label className="mcp-field">
            <span className="small">{t("Claude Code scope")}</span>
            <select aria-label={t("Claude Code scope")} className="select" onChange={(event) => setScope(event.target.value as McpScope)} value={scope}>
              <option value="user">{t("Every project")}</option>
              <option value="local">{t("This project only")}</option>
            </select>
          </label>
        )}
      </div>

      {needs && (
        <p className="small muted">
          {t("Starts with {tool}; the first start downloads the package.", { tool: needs === "uv" ? t("uvx, from uv (astral.sh/uv)") : t("npx, from Node.js") })}
        </p>
      )}

      {probe.busy && <p className="small muted">{t("Starting it the way a session would…")}</p>}
      {probe.result && (
        <p className="small">
          <span className="chip small chip-active">{t("works")}</span> {probe.result.server ?? draft.name}
          {probe.result.version ? ` ${probe.result.version}` : ""} — {probe.result.tools.length === 1 ? t("1 tool") : t("{count} tools", { count: probe.result.tools.length })}
          {probe.result.tools.length > 0 && <span className="muted">: {probe.result.tools.slice(0, 12).join(", ")}{probe.result.tools.length > 12 ? ", …" : ""}</span>}
        </p>
      )}
      {probe.error && <pre className="small chip-warn mcp-error">{probe.error}</pre>}
      {outcomes && (
        <ul className="call-list">
          {outcomes.map((outcome) => (
            <li className={`small ${outcome.ok ? "" : "chip-warn"}`} key={outcome.provider}>
              {PROVIDER_NAME[outcome.provider]}: {outcome.message}
            </li>
          ))}
        </ul>
      )}
      <div className="row wrap">
        <button className="button button-primary" disabled={!nameOk || !draft.target.trim() || providers.length === 0 || installing} onClick={() => void install()} type="button">
          {installing ? t("Installing…") : providers.length === 1 ? t("Install for 1 provider") : t("Install for {count} providers", { count: providers.length })}
        </button>
        <button className="button" disabled={!draft.target.trim() || probe.busy} onClick={() => void test()} type="button">
          {t("Test it")}
        </button>
        <button className="button" onClick={onCancel} type="button">
          {t("Cancel")}
        </button>
        <span className="small muted">{t("Applies to sessions started afterwards.")}</span>
      </div>
    </div>
  );
}

export function McpSection({ workspaceId }: { workspaceId: string | null }) {
  const setError = useStore((s) => s.setError);
  const [overview, setOverview] = useState<McpOverview | null>(null);
  const [catalog, setCatalog] = useState<McpCatalogEntry[]>([]);
  const [runtimeNames, setRuntimeNames] = useState<string[]>([]);
  const [paste, setPaste] = useState("");
  const [pending, setPending] = useState<{ spec: McpServerSpec; needs?: string }[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const load = useCallback(async () => {
    try {
      const [loaded, env] = await Promise.all([api.mcpOverview(workspaceId), api.runtimeEnvList().catch(() => null)]);
      setOverview(loaded);
      setRuntimeNames(env?.names ?? []);
    } catch (error) {
      setError(error);
    }
  }, [workspaceId, setError]);

  useEffect(() => {
    void load();
    void api
      .mcpCatalog()
      .then(setCatalog)
      .catch(() => setCatalog([]));
  }, [load]);

  const grouped = useMemo(() => {
    const byName = new Map<string, InstalledMcpServer[]>();
    for (const server of overview?.servers ?? []) byName.set(server.name, [...(byName.get(server.name) ?? []), server]);
    return [...byName.entries()];
  }, [overview]);

  const fromCatalog = (entry: McpCatalogEntry) => {
    const spec: McpServerSpec =
      entry.spec.transport.type === "stdio"
        ? { ...entry.spec, transport: { ...entry.spec.transport, args: entry.spec.transport.args.map((arg) => arg.replace("{folder}", overview?.folder ?? ".")) } }
        : entry.spec;
    setPending([{ spec, ...(entry.needs ? { needs: entry.needs } : {}) }]);
    setAdding(false);
  };

  const read = async () => {
    try {
      const specs = await api.mcpParse(paste);
      setPending(specs.map((spec) => ({ spec })));
      setPaste("");
      setAdding(false);
    } catch (error) {
      setError(error);
    }
  };

  /** Takes a server out of every provider that has it in a place ThingMaker can write. */
  const remove = async (name: string, servers: InstalledMcpServer[]) => {
    const removable = servers.filter((server) => server.scope !== "project");
    if (removable.length === 0) return;
    const providers = removable.map((server) => PROVIDER_NAME[server.provider]).join(", ");
    const confirmed = await api.confirmDialog({
      title: t("Remove {name}?", { name }),
      message: t("It is taken out of {providers} with each one's own mcp remove. Sessions started afterwards no longer have it.", { providers }),
      okLabel: t("Remove"),
      cancelLabel: t("Keep it"),
      warning: true,
    });
    if (!confirmed) return;
    try {
      for (const server of removable) await api.mcpRemove(workspaceId, server.provider, server.name, server.scope);
      setNote(t("Removed {name}.", { name }));
      await load();
    } catch (error) {
      setError(error);
    }
  };

  const installedNames = new Set(overview?.servers.map((server) => server.name));
  const disabled = (overview?.servers ?? []).filter((server) => !server.enabled).length;

  return (
    <Fold
      hint={t("Outside tools agents can use, like a browser or a design file")}
      name={t("MCP servers")}
      summary={grouped.length === 1 ? t("1 server") : t("{n} servers", { n: grouped.length })}
    >
      {grouped.map(([name, servers]) => {
        const scopes = new Set(servers.map((server) => server.scope));
        const keys = [...new Set(servers.flatMap((server) => [...server.envKeys, ...server.headerKeys]))];
        return (
          <FoldItem
            actions={
              servers.some((server) => server.scope !== "project") && (
                <button className="set-link set-link-danger" onClick={() => void remove(name, servers)} type="button">
                  {t("Remove")}
                </button>
              )
            }
            key={name}
          >
            <span className={`set-dot ${servers.every((server) => server.enabled) ? "set-dot-ok" : "set-dot-warn"}`} title={servers.every((server) => server.enabled) ? t("Enabled") : t("Disabled for some providers")} />
            <span className="set-item-text mono" title={servers[0]?.summary}>
              {name}
            </span>
            <span className="set-marks">
              {servers.map((server) => (
                <span key={`${server.provider}-${server.scope}`} title={PROVIDER_NAME[server.provider]}>
                  <ProviderMark provider={server.provider} size={14} />
                </span>
              ))}
            </span>
            <span className="set-chip">{scopes.has("user") ? t("Every project") : scopes.has("local") ? t("This project") : ".mcp.json"}</span>
            {keys.length > 0 && <span className="set-item-note">{t("uses {keys}", { keys: keys.join(", ") })}</span>}
          </FoldItem>
        );
      })}
      {grouped.length === 0 && <p className="set-item-note">{t("No servers yet.")}</p>}
      {disabled > 0 && <p className="set-item-note">{t("{n} turned off in a provider's own settings.", { n: disabled })}</p>}
      {note && (
        <p className="set-item-note">
          {note}{" "}
          <button className="set-link" onClick={() => setNote(null)} type="button">
            {t("dismiss")}
          </button>
        </p>
      )}

      {pending.length > 0 && overview ? (
        <div className="mcp-pending">
          {pending.map((entry, index) => (
            <InstallCard
              initial={entry.spec}
              key={`${entry.spec.name}-${index}`}
              needs={entry.needs}
              onCancel={() => setPending(pending.filter((_, at) => at !== index))}
              onDone={() => {
                setNote(t("Installed {name}. New sessions have it.", { name: entry.spec.name }));
                setPending(pending.filter((_, at) => at !== index));
                void load();
              }}
              overview={overview}
              runtimeNames={runtimeNames}
              workspaceId={workspaceId}
            />
          ))}
        </div>
      ) : adding ? (
        <div className="set-add">
          {catalog.map((entry) => (
            <FoldItem
              actions={
                <>
                  <button className="set-link" onClick={() => void api.openExternal(entry.homepage)} type="button">
                    {t("About")}
                  </button>
                  <button className="set-button" onClick={() => fromCatalog(entry)} type="button">
                    {installedNames.has(entry.spec.name) ? t("Reinstall…") : t("Install…")}
                  </button>
                </>
              }
              key={entry.id}
            >
              <span className="set-item-text">{entry.title}</span>
              <span className="set-item-note" title={entry.secrets.length > 0 ? t("Needs {list}.", { list: entry.secrets.map(([name, where]) => `${name} (${where})`).join(", ") }) : undefined}>
                {entry.description}
              </span>
            </FoldItem>
          ))}
          <div className="set-paste">
            <textarea
              aria-label={t("Paste an MCP config or command")}
              className="set-textarea mono"
              onChange={(event) => setPaste(event.target.value)}
              placeholder={t("Or paste one from its README: {config}, {claude}, {codex} or {npx}", {
                config: '{"mcpServers": {…}}',
                claude: '"claude mcp add …"',
                codex: '"codex mcp add …"',
                npx: '"npx -y some-mcp-server"',
              })}
              rows={2}
              value={paste}
            />
            <div className="set-actions">
              <button className="set-button" disabled={!paste.trim()} onClick={() => void read()} type="button">
                {t("Read it")}
              </button>
              <button className="set-link" onClick={() => setAdding(false)} type="button">
                {t("Cancel")}
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div className="set-actions">
          <button className="set-button" onClick={() => setAdding(true)} type="button">
            {t("Add a server")}
          </button>
          <span className="set-item-note">{t("Installed with each provider's own mcp add, so it works in their apps too")}</span>
        </div>
      )}
    </Fold>
  );
}
