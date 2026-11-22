import en from "../../copy/en.json";
import it from "../../copy/it.json";
import { createTranslator, type Bundle } from "./core";

export { escapeHtml, html, type Msg, type Params } from "./core";

/** The launcher languages with a bundle; `copy/` is the single source of the texts. */
export const LANGS = ["en", "it"] as const;
export type Lang = (typeof LANGS)[number];

const i18n = createTranslator({ en: en as Bundle, it: it as Bundle }, "en", import.meta.env.DEV);

export const { t, th, tr, list, has, size, number, duration } = i18n;
export const lang = (): Lang => i18n.lang() as Lang;

export function setLang(l: string): void {
  i18n.setLang(l);
  document.documentElement.lang = i18n.lang();
}

/** The first-start language when the back end has no saved one: Italian for an Italian system, else English. */
export const systemLang = (): Lang => (navigator.languages.some((l) => l.toLowerCase().startsWith("it")) ? "it" : "en");
