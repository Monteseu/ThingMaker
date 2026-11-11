/**
 * Local Models in the Providers panel: Ollama's state, the models installed,
 * downloads, and how private mode is kept. It walks through setup one step
 * at a time: get Ollama, start it, get a model.
 */
import { useCallback, useEffect, useState } from "react";
import { type LocalStatus, type ProviderInfo } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { bytes, fitLabel } from "../localModels";
import { ProviderLogo } from "./ProviderLogo";
import { t } from "../i18n";

type Download = { status: string; completed?: number; total?: number; error?: string };

function percent(download: Download): number | null {
  return download.total && download.completed !== undefined ? Math.min(100, Math.round((download.completed / download.total) * 100)) : null;
}

export function LocalModelsCard({ info }: { info: ProviderInfo }) {
  const loadProviders = useStore((s) => s.loadProviders);
  const loadProviderModels = useStore((s) => s.loadProviderModels);
  const openUrl = useStore((s) => s.openUrl);
  const [status, setStatus] = useState<LocalStatus | null>(null);
  const [starting, setStarting] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [downloads, setDownloads] = useState<Record<string, Download>>({});
  const [other, setOther] = useState("");

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.localStatus());
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String((error as { message?: string })?.message ?? error));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const changed = async () => {
    await refresh();
    void loadProviders();
    void loadProviderModels("local", true);
  };

  const start = async () => {
    setStarting(true);
    setProblem(null);
    try {
      await api.localStart();
      await changed();
    } catch (error) {
      setProblem(String((error as { message?: string })?.message ?? error));
    } finally {
      setStarting(false);
    }
  };

  const download = async (name: string) => {
    const model = name.trim();
    if (!model || downloads[model]) return;
    setDownloads((all) => ({ ...all, [model]: { status: t("starting") } }));
    const forget = () =>
      setDownloads((all) => {
        const { [model]: _gone, ...rest } = all;
        return rest;
      });
    try {
      await api.localPull(model, (event) => {
        if (event.kind === "progress")
          setDownloads((all) => ({ ...all, [model]: { status: event.status, ...(event.completed != null ? { completed: event.completed } : {}), ...(event.total != null ? { total: event.total } : {}) } }));
        else if (event.kind === "failed") setDownloads((all) => ({ ...all, [model]: { status: "", error: event.message } }));
        else {
          forget();
          void changed();
        }
      });
      setOther("");
    } catch (error) {
      setDownloads((all) => ({ ...all, [model]: { status: "", error: String((error as { message?: string })?.message ?? error) } }));
    }
  };

  const allowLoopback = async () => {
    try {
      const allowed = await api.localAllowLoopback();
      if (!allowed) setProblem(t("Windows did not allow it, so local agents cannot start yet. Chats still work."));
      await refresh();
    } catch (error) {
      setProblem(String((error as { message?: string })?.message ?? error));
    }
  };

  const remove = async (name: string) => {
    const confirmed = await api.confirmDialog({
      title: t("Remove {model}?", { model: name }),
      message: t("It is deleted from Ollama on this computer. Sessions and teams that use it will need another model."),
      okLabel: t("Remove"),
      cancelLabel: t("Keep it"),
      warning: true,
    });
    if (!confirmed) return;
    try {
      await api.localDelete(name);
      await changed();
    } catch (error) {
      setProblem(String((error as { message?: string })?.message ?? error));
    }
  };

  const running = status?.running ?? info.auth?.loggedIn === true;
  const installed = status?.installed ?? Boolean(info.resolved);
  const state = !installed ? t("Not installed") : running ? t("Running") : t("Ollama is stopped");
  const tone = !installed ? "off" : running ? "ok" : "warn";
  const toolModels = status?.models.filter((model) => model.capabilities.includes("tools")).length ?? 0;

  return (
    <li className="provider-row local-models">
      <div className="provider-row-main">
        <ProviderLogo provider="local" size={38} />
        <div className="provider-who">
          <div className="provider-name-line">
            <strong>{info.label}</strong>
            <span className={`provider-status ${tone}`}>
              <i />
              {state}
            </span>
            <span className="chip small chip-private">{t("free · private")}</span>
          </div>
          <div className="provider-sub">
            <span>{status?.version ? t("Ollama {version} · runs models on this computer", { version: status.version }) : t("Runs models on this computer through Ollama")}</span>
          </div>
        </div>
        <div className="provider-actions">
          {!installed ? (
            <button className="button button-primary button-small" onClick={() => void openUrl("https://ollama.com/download")} type="button">
              {t("Get Ollama")}
            </button>
          ) : !running ? (
            <button className="button button-primary button-small" disabled={starting} onClick={() => void start()} type="button">
              {starting ? t("Starting…") : t("Start Ollama")}
            </button>
          ) : (
            <button className="button button-small" onClick={() => void refresh()} type="button">
              {t("Check again")}
            </button>
          )}
        </div>
      </div>

      {problem && <p className="small local-warn">{problem}</p>}
      {!installed && <p className="small muted local-line">{t("Install Ollama, and ThingMaker finds it. There is nothing else to set up.")}</p>}

      {installed && running && status && (
        <div className="local-body">
          {status.privateMode === "app_container" && status.loopbackAllowed === false && (
            <div className="local-get">
              <div className="local-get-text">
                <strong>{t("Allow local agents to reach Ollama")}</strong>
                <span className="small muted">{t("Local agents run in a Windows AppContainer, which can only reach this PC. To talk to Ollama it needs your permission once, and Windows asks for an administrator's approval. Chats work without it.")}</span>
              </div>
              <button className="button button-primary button-small" onClick={() => void allowLoopback()} type="button">
                {t("Allow")}
              </button>
            </div>
          )}

          {!status.agentsAvailable && <p className="small muted local-line">{t("Local agents run on Claude Code's adapter, which is not installed. Chats work without it.")}</p>}

          <div className="local-section">
            <span className="local-cap">{t("Installed")}</span>
            {status.models.length === 0 ? (
              <p className="small muted local-line">{t("No models yet. Get one below.")}</p>
            ) : (
              <table className="local-table">
                <thead>
                  <tr>
                    <th>{t("Model")}</th>
                    <th>{t("Size")}</th>
                    <th>{t("Parameters")}</th>
                    <th>{t("Can be")}</th>
                    <th>{t("Here")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {status.models.map((model) => (
                    <tr key={model.name}>
                      <td className="mono">{model.name}</td>
                      <td>{bytes(model.sizeBytes)}</td>
                      <td>{[model.parameterSize, model.quantization].filter(Boolean).join(" · ")}</td>
                      <td>{model.capabilities.includes("tools") ? t("Chat · Agent") : t("Chat only")}</td>
                      <td>{fitLabel(model.fit) && <span className={`local-fit local-fit-${model.fit}`}>{fitLabel(model.fit)}</span>}</td>
                      <td>
                        <button className="button button-small" onClick={() => void remove(model.name)} type="button">
                          {t("Remove")}
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {toolModels > 0 && <p className="small muted local-line">{t("Chats answer in seconds. Agents are slow on a laptop: each step reads the agent's instructions again, so a small task can take several minutes.")}</p>}
            {status.models.length > 0 && toolModels === 0 && <p className="small muted local-line">{t("None of these can use tools, so none can be an agent. They can all chat.")}</p>}
          </div>

          <div className="local-section">
            <span className="local-cap">{status.memoryBytes ? t("Get a model · picked for {memory}", { memory: bytes(status.memoryBytes) }) : t("Get a model")}</span>
            {status.recommended
              .filter((model) => !model.installed)
              .map((model) => {
                const running = downloads[model.name];
                const progress = running ? percent(running) : null;
                return (
                  <div className="local-get" key={model.name}>
                    <div className="local-get-text">
                      <strong className="mono">{model.name}</strong>
                      <span className="small muted">
                        {model.purpose} · {t("about {size}", { size: bytes(model.sizeBytes) })}
                      </span>
                      {running && !running.error && (
                        <span className="local-progress">
                          <span style={{ width: `${progress ?? 4}%` }} />
                        </span>
                      )}
                      {running?.error && <span className="small local-warn">{running.error}</span>}
                    </div>
                    {fitLabel(model.fit) && <span className={`local-fit local-fit-${model.fit}`}>{model.fit === "too_big" ? t("Too big here") : fitLabel(model.fit)}</span>}
                    {running && !running.error ? (
                      <button className="button button-small" onClick={() => void api.localPullCancel(model.name)} type="button">
                        {progress !== null ? `${progress}% · ${t("Cancel")}` : t("Cancel")}
                      </button>
                    ) : (
                      <button className="button button-primary button-small" disabled={model.fit === "too_big"} onClick={() => void download(model.name)} type="button">
                        {t("Download")}
                      </button>
                    )}
                  </div>
                );
              })}
            <form
              className="local-other"
              onSubmit={(event) => {
                event.preventDefault();
                void download(other);
              }}
            >
              <input aria-label={t("Another model by name")} className="input" onChange={(event) => setOther(event.target.value)} placeholder={t("Another model by its Ollama name, like mistral:7b")} spellCheck={false} value={other} />
              <button className="button button-small" disabled={!other.trim() || Boolean(downloads[other.trim()])} type="submit">
                {t("Download")}
              </button>
            </form>
            {Object.entries(downloads)
              .filter(([name]) => !status.recommended.some((model) => model.name === name))
              .map(([name, running]) => (
                <div className="local-get" key={name}>
                  <div className="local-get-text">
                    <strong className="mono">{name}</strong>
                    {running.error ? (
                      <span className="small local-warn">{running.error}</span>
                    ) : (
                      <span className="local-progress">
                        <span style={{ width: `${percent(running) ?? 4}%` }} />
                      </span>
                    )}
                  </div>
                  {!running.error && (
                    <button className="button button-small" onClick={() => void api.localPullCancel(name)} type="button">
                      {t("Cancel")}
                    </button>
                  )}
                </div>
              ))}
            <span className="small muted">{t("The list is part of ThingMaker and filtered by this computer's memory; nothing is looked up online. Ollama's cloud models are not offered: they are neither free nor private.")}</span>
          </div>
        </div>
      )}
    </li>
  );
}
