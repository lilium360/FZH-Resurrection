import { setLang, th } from "../i18n";
import { esc, logLines } from "./components";
import { I } from "./icons";
import { shell } from "./shell";
import { REC, S, app, dirty } from "./state";

const byId = (id: string): HTMLElement => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`Missing #${id}`);
  return el;
};

export const root = byId("root");
export const win = byId("win");
export const toastsEl = document.createElement("div");
toastsEl.className = "b-toasts";
toastsEl.setAttribute("role", "status");
toastsEl.setAttribute("aria-live", "polite");
export const modalEl = document.createElement("div");
win.append(modalEl, toastsEl);

let wizardRenderer: () => void = () => {};
export function setWizardRenderer(fn: () => void): void {
  wizardRenderer = fn;
}

/** Redraws the screen in the launcher language being edited, so a change shows at once. */
export function render(): void {
  setLang(S.launcherLanguage);
  root.className = "p pb";
  root.innerHTML = shell();
  toastsEl.style.bottom = dirty() ? "96px" : "";
  const l = root.querySelector<HTMLElement>("[data-log]");
  if (l) l.scrollTop = l.scrollHeight;
  try {
    history.replaceState(null, "", "#" + app.scr);
  } catch {
    return;
  }
}

/** The setup assistant or the recovery code step owns the modal layer while either is open. */
const modalOwned = (): boolean => app.wz !== null || !!REC.code;

/** Redraws the screen and, while it is open, the setup assistant. */
export function refresh(): void {
  render();
  if (modalOwned()) wizardRenderer();
}

export function renderLog(): void {
  const l = root.querySelector<HTMLElement>("[data-log]");
  if (!l) return;
  l.innerHTML = logLines();
  l.scrollTop = l.scrollHeight;
}

export function toast(msg: string, err = false): void {
  const t = document.createElement("div");
  t.className = "b-toast" + (err ? " err" : "");
  t.innerHTML = `${I(err ? "alert" : "check", 15)}<span>${esc(msg)}</span>`;
  toastsEl.append(t);
  setTimeout(() => t.remove(), 2800);
}

/** `label`, and the title and text of `ask`, are HTML (from `th`). */
export interface ModalAction {
  label: string;
  cls?: string;
  fn?: (el: HTMLElement) => unknown;
}

export let MODAL: { actions: ModalAction[] } | null = null;

export function ask(title: string, text: string, actions: ModalAction[], body = "", wide = false): void {
  MODAL = { actions };
  modalEl.innerHTML = `<div class="b-modal"><div class="b-dlg ${wide ? "wide" : ""}" role="dialog" aria-modal="true" aria-labelledby="dlg-t"><h2 id="dlg-t">${title}</h2><p>${text}</p>${body}<div class="b-acts">${actions
    .map((a, i) => `<button class="btn ${a.cls ?? ""}" data-m="${i}">${a.label}</button>`)
    .join("")}</div></div></div>`;
  const first = modalEl.querySelector<HTMLElement>("input,textarea") ?? modalEl.querySelector<HTMLElement>(".btn.fill,.btn.danger");
  first?.focus();
}

export function closeModal(): void {
  MODAL = null;
  modalEl.innerHTML = "";
  if (modalOwned()) wizardRenderer();
}

export const cancelAction = (): ModalAction => ({ label: th("dialog.unsavedChanges.cancel") });

export const confirmIt = (title: string, text: string, label: string, fn: () => unknown, danger = true): void =>
  ask(title, text, [cancelAction(), { label, cls: danger ? "danger" : "fill", fn }]);
