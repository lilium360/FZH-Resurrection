export type Bundle = { [key: string]: unknown };
export type Params = Record<string, unknown>;

/** A text from the back end: an i18n key with parameters, and its English text as a fallback. */
export interface Msg {
  key?: string;
  params?: Params;
  message?: string;
}

/** A parameter inserted into HTML as it is (already escaped markup). */
export class Html {
  readonly html: string;
  constructor(markup: string) {
    this.html = markup;
  }
}

export const html = (markup: string): Html => new Html(markup);

export const escapeHtml = (v: unknown): string =>
  String(v).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] ?? c);

export interface Translator {
  lang(): string;
  setLang(lang: string): void;
  has(key: string): boolean;
  /** Plain text: for toasts, text nodes, the log. */
  t(key: string, params?: Params): string;
  /** HTML: the text and every parameter escaped, except `html()` parameters. */
  th(key: string, params?: Params): string;
  /** A list from the bundle, e.g. the wizard steps. */
  list(key: string): unknown[];
  /** A back-end text: translated when its key exists, else its English text. */
  tr(m: Msg | string | null | undefined): string;
  size(bytes: number): string;
  number(n: number, digits?: number): string;
  duration(seconds: number): string;
}

function lookup(tree: unknown, key: string): unknown {
  let node = tree;
  for (const part of key.split(".")) {
    if (node === null || typeof node !== "object") return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return node;
}

const isPlural = (v: unknown): v is Record<string, string> =>
  !!v && typeof v === "object" && !Array.isArray(v) && typeof (v as Record<string, unknown>).other === "string";

/**
 * `dev`: a missing key shows as ⟦key⟧ so it stands out; otherwise the English text is used.
 * Bundles: the first one is the fallback language.
 */
export function createTranslator(bundles: Record<string, Bundle>, fallback: string, dev: boolean): Translator {
  let current = fallback;
  const plural = new Map<string, Intl.PluralRules>();
  const rules = (l: string) => {
    let r = plural.get(l);
    if (!r) plural.set(l, (r = new Intl.PluralRules(l)));
    return r;
  };

  const raw = (key: string): { value: unknown; lang: string } | null => {
    const own = lookup(bundles[current], key);
    if (own !== undefined) return { value: own, lang: current };
    if (dev) return null;
    const en = lookup(bundles[fallback], key);
    return en === undefined ? null : { value: en, lang: fallback };
  };

  const template = (key: string, params: Params): string | null => {
    const found = raw(key);
    if (!found) return null;
    let v = found.value;
    if (isPlural(v)) {
      const n = Number(params.count ?? 0);
      v = v[rules(found.lang).select(n)] ?? v.other;
    }
    return typeof v === "string" ? v : null;
  };

  const marker = (key: string) => `⟦${key}⟧`;

  const value = (v: unknown, escape: boolean): string => {
    if (v instanceof Html) return v.html;
    if (v && typeof v === "object") {
      const o = v as Record<string, unknown>;
      if (typeof o.bytes === "number") return wrap(api.size(o.bytes), escape);
      if (typeof o.seconds === "number") return wrap(api.duration(o.seconds), escape);
      if (Array.isArray(o.list)) return wrap(o.list.map((x) => api.tr(x as Msg)).join(", "), escape);
      if ("key" in o || "message" in o) return wrap(api.tr(o as Msg), escape);
    }
    return wrap(String(v ?? ""), escape);
  };
  const wrap = (s: string, escape: boolean) => (escape ? escapeHtml(s) : s);

  const fill = (text: string, params: Params, escape: boolean): string => {
    const parts = text.split(/(\{\w+\})/);
    return parts
      .map((p) => {
        const m = /^\{(\w+)\}$/.exec(p);
        if (m && m[1] !== undefined && m[1] in params) return value(params[m[1]], escape);
        return wrap(p, escape);
      })
      .join("");
  };

  const api: Translator = {
    lang: () => current,
    setLang(l) {
      if (bundles[l]) current = l;
    },
    has: (key) => typeof lookup(bundles[current], key) === "string" || isPlural(lookup(bundles[current], key)),
    t(key, params = {}) {
      const text = template(key, params);
      return text === null ? (dev ? marker(key) : key) : fill(text, params, false);
    },
    th(key, params = {}) {
      const text = template(key, params);
      return text === null ? escapeHtml(dev ? marker(key) : key) : fill(text, params, true);
    },
    list(key) {
      const v = raw(key)?.value;
      return Array.isArray(v) ? v : [];
    },
    tr(m) {
      if (m === null || m === undefined) return "";
      if (typeof m === "string") return m;
      if (m.key && raw(m.key)) return api.t(m.key, m.params ?? {});
      if (m.key && dev) return marker(m.key);
      return m.message ?? "";
    },
    number(n, digits = 0) {
      return new Intl.NumberFormat(current, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(n);
    },
    size(bytes) {
      if (bytes < 2 ** 20) return `${api.number(Math.max(1, Math.round(bytes / 2 ** 10)))} KB`;
      if (bytes < 2 ** 30) return `${api.number(bytes / 2 ** 20, 1)} MB`;
      return `${api.number(bytes / 2 ** 30, 1)} GB`;
    },
    duration(seconds) {
      const s = Math.max(0, Math.round(seconds));
      if (s < 60) return `${s} s`;
      return `${Math.floor(s / 60)} min ${String(s % 60).padStart(2, "0")} s`;
    },
  };
  return api;
}
