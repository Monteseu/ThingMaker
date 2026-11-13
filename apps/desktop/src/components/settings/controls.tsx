/**
 * The pieces Settings is built from: one line per setting (name on the left,
 * control on the right), switches, compact menus that show a check on the
 * current choice, a fold for what is rarely needed, and an ⓘ that opens a
 * setting's longer explanation in place.
 */
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { t } from "../../i18n";
import { IconCheck, IconChevron } from "../icons";

/** A group of rows on one surface, with an optional small heading and action. */
export function SettingGroup({ title, action, children, note }: { title?: string; action?: ReactNode; children: ReactNode; note?: ReactNode }) {
  return (
    <section className="set-group">
      {(title || action) && (
        <div className="set-group-head">
          {title ? <h3>{title}</h3> : <span />}
          {action}
        </div>
      )}
      <div className="set-rows">{children}</div>
      {note && <p className="set-note">{note}</p>}
    </section>
  );
}

/**
 * One setting. `hint` is a short line under the name, kept for what prevents
 * a mistake; `info` is the longer explanation, shown only when its ⓘ is
 * pressed.
 */
export function SettingRow({ name, hint, info, sub, children }: { name: ReactNode; hint?: ReactNode; info?: ReactNode; sub?: boolean; children?: ReactNode }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <div className={`set-row ${sub ? "set-row-sub" : ""}`}>
      <div className="set-label">
        <span className="set-name">
          {name}
          {info && (
            <button aria-controls={id} aria-expanded={open} aria-label={t("More about this")} className="set-info" onClick={() => setOpen(!open)} type="button">
              i
            </button>
          )}
        </span>
        {hint && <span className="set-hint">{hint}</span>}
        {info && open && (
          <span className="set-more" id={id}>
            {info}
          </span>
        )}
      </div>
      {children !== undefined && <div className="set-controls">{children}</div>}
    </div>
  );
}

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (checked: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button aria-checked={checked} aria-label={label} className="set-switch" disabled={disabled} onClick={() => onChange(!checked)} role="switch" type="button" />
  );
}

export type Choice<T extends string> = { value: T; label: string; hint?: string };

/**
 * A menu button sized to its text. The menu lists the choices with a check on
 * the current one; arrows move, Enter picks, Escape closes.
 */
export function Select<T extends string>({
  value,
  choices,
  onChange,
  label,
  placeholder,
  disabled,
}: {
  value: T | null;
  choices: Choice<T>[];
  onChange: (value: T) => void;
  label: string;
  placeholder?: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const current = choices.find((choice) => choice.value === value);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown, true);
    const selected = menu.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]') ?? menu.current?.querySelector<HTMLButtonElement>("button");
    selected?.focus();
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, [open]);

  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      setOpen(false);
      root.current?.querySelector<HTMLButtonElement>(".set-select")?.focus();
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const items = [...(menu.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    items[(at + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
  };

  return (
    <span className="set-select-wrap" onKeyDown={onKey} ref={root}>
      <button aria-expanded={open} aria-haspopup="listbox" aria-label={label} className="set-select" disabled={disabled} onClick={() => setOpen(!open)} title={label} type="button">
        <span className={current ? "" : "muted"}>{current?.label ?? placeholder ?? ""}</span>
        <IconChevron open size={12} />
      </button>
      {open && (
        <div aria-label={label} className="set-menu" ref={menu} role="listbox">
          {choices.map((choice) => (
            <button
              aria-selected={choice.value === value}
              key={choice.value}
              onClick={() => {
                setOpen(false);
                if (choice.value !== value) onChange(choice.value);
              }}
              role="option"
              type="button"
            >
              <span className="set-menu-text">
                <span>{choice.label}</span>
                {choice.hint && <span className="set-menu-hint">{choice.hint}</span>}
              </span>
              {choice.value === value && <IconCheck size={14} />}
            </button>
          ))}
        </div>
      )}
    </span>
  );
}

/** Two or three choices side by side. */
export function Segmented<T extends string>({ value, choices, onChange, label }: { value: T; choices: Choice<T>[]; onChange: (value: T) => void; label: string }) {
  return (
    <div aria-label={label} className="set-seg" role="group">
      {choices.map((choice) => (
        <button aria-pressed={choice.value === value} key={choice.value} onClick={() => onChange(choice.value)} title={choice.hint} type="button">
          {choice.label}
        </button>
      ))}
    </div>
  );
}

/** A row that opens: for what most people never need to see. */
export function Fold({ name, hint, summary, defaultOpen, children }: { name: ReactNode; hint?: ReactNode; summary?: ReactNode; defaultOpen?: boolean; children: ReactNode }) {
  return (
    <details className="set-fold" open={defaultOpen}>
      <summary>
        <span className="set-label">
          <span className="set-name">{name}</span>
          {hint && <span className="set-hint">{hint}</span>}
        </span>
        {summary !== undefined && <span className="set-summary">{summary}</span>}
        <span className="set-chevron" aria-hidden="true">
          <IconChevron size={14} />
        </span>
      </summary>
      <div className="set-fold-body">{children}</div>
    </details>
  );
}

/** One line inside a fold: a name and its actions. */
export function FoldItem({ children, actions }: { children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="set-item">
      <span className="set-item-main">{children}</span>
      {actions && <span className="set-item-actions">{actions}</span>}
    </div>
  );
}
