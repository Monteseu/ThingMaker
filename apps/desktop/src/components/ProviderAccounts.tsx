/**
 * A provider's accounts in the Providers panel (Claude Code, Codex): which
 * one is active, who each is signed in as, and adding, renaming, removing
 * and signing in to them. Every account shares settings, MCP servers,
 * skills and history; only the sign-in differs.
 */
import { useEffect, useState } from "react";
import { type AccountView, type Provider } from "@thingmaker/contracts";
import { api } from "../ipc";
import { useStore } from "../store";
import { t } from "../i18n";

/** What an account is called: its name, else (the default) "Main account". */
export function accountName(account: AccountView): string {
  return account.isDefault ? t("Main account") : account.label;
}

export function ProviderAccounts({ provider }: { provider: Provider }) {
  const accounts = useStore((s) => s.accounts[provider]);
  const loadAccounts = useStore((s) => s.loadAccounts);
  const addAccount = useStore((s) => s.addAccount);
  const renameAccount = useStore((s) => s.renameAccount);
  const removeAccount = useStore((s) => s.removeAccount);
  const setActiveAccount = useStore((s) => s.setActiveAccount);
  const startSignIn = useStore((s) => s.startSignIn);
  const signIn = useStore((s) => s.signIn);
  const [adding, setAdding] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; label: string } | null>(null);

  useEffect(() => {
    void loadAccounts(provider);
  }, [provider, loadAccounts]);

  const remove = async (account: AccountView) => {
    const confirmed = await api.confirmDialog({
      title: t("Remove the account “{name}”?", { name: account.label }),
      message: t("It is signed out and removed from ThingMaker. Your settings, MCP servers, skills and conversation history stay, because every account shares them."),
      okLabel: t("Remove"),
      cancelLabel: t("Keep it"),
      warning: true,
    });
    if (confirmed) await removeAccount(provider, account.id);
  };

  const list = accounts ?? [];
  return (
    <div className="provider-accounts">
      {list.length > 1 && <span className="local-cap">{t("Accounts")}</span>}
      {list.length > 1 &&
        list.map((account) => {
          const signedIn = account.auth?.loggedIn === true;
          const busy = signIn?.provider === provider && signIn.account === account.id && (signIn.state === "starting" || signIn.state === "running");
          return (
            <div className={`provider-account ${account.active ? "on" : ""}`} key={account.id}>
              <label className="provider-account-pick">
                <input checked={account.active} name={`account-${provider}`} onChange={() => void setActiveAccount(provider, account.id)} type="radio" />
                {renaming?.id === account.id ? (
                  <input
                    aria-label={t("Account name")}
                    autoFocus
                    className="input provider-account-name"
                    onBlur={() => {
                      if (renaming.label.trim()) void renameAccount(provider, account.id, renaming.label);
                      setRenaming(null);
                    }}
                    onChange={(event) => setRenaming({ id: account.id, label: event.target.value })}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") (event.target as HTMLInputElement).blur();
                      if (event.key === "Escape") setRenaming(null);
                    }}
                    value={renaming.label}
                  />
                ) : (
                  <span className="provider-account-text">
                    <strong>{accountName(account)}</strong>
                    <span className="small muted">
                      {signedIn ? [account.auth?.account, account.auth?.plan].filter(Boolean).join(" · ") || t("Signed in") : account.auth?.problem ? t("Could not be asked") : t("Not signed in")}
                    </span>
                  </span>
                )}
              </label>
              {!signedIn && (
                <button className="button button-small" disabled={busy} onClick={() => void startSignIn(provider, account.isDefault ? undefined : account.id)} type="button">
                  {busy ? t("Signing in…") : t("Sign in")}
                </button>
              )}
              {!account.isDefault && renaming?.id !== account.id && (
                <>
                  <button className="button button-small" onClick={() => setRenaming({ id: account.id, label: account.label })} type="button">
                    {t("Rename")}
                  </button>
                  <button className="button button-small" onClick={() => void remove(account)} type="button">
                    {t("Remove")}
                  </button>
                </>
              )}
            </div>
          );
        })}
      {adding === null ? (
        <button className="button button-small provider-account-add" onClick={() => setAdding("")} type="button">
          {t("+ Add account")}
        </button>
      ) : (
        <form
          className="provider-account-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (!adding.trim()) return;
            void addAccount(provider, adding);
            setAdding(null);
          }}
        >
          <input aria-label={t("Account name")} autoFocus className="input" onChange={(event) => setAdding(event.target.value)} onKeyDown={(event) => event.key === "Escape" && setAdding(null)} placeholder={t("A name, like Work or Personal")} value={adding} />
          <button className="button button-primary button-small" disabled={!adding.trim()} type="submit">
            {t("Add and sign in")}
          </button>
          <button className="button button-small" onClick={() => setAdding(null)} type="button">
            {t("Cancel")}
          </button>
        </form>
      )}
    </div>
  );
}
