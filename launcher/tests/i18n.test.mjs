// node --test tests/  (Node 22.18+ strips the TypeScript types of core.ts)
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createTranslator, html } from "../src/i18n/core.ts";

const bundle = (name) => JSON.parse(readFileSync(new URL(`../copy/${name}.json`, import.meta.url), "utf8"));
const en = bundle("en");
const it = bundle("it");

const sample = {
  en: {
    hi: "Hello {name}",
    rounds: { one: "One round", other: "{count} rounds" },
    only: { en: "English only" },
    list: ["a", "b"],
    server: { official: "official server" },
    on: "Playing on the {server}",
  },
  it: { hi: "Ciao {name}", rounds: { one: "Un round", other: "{count} round" }, list: ["x", "y"], server: { official: "server ufficiale" }, on: "Si gioca sul {server}" },
};

test("placeholders are filled, unknown ones stay", () => {
  const i = createTranslator(sample, "en", true);
  assert.equal(i.t("hi", { name: "Ann" }), "Hello Ann");
  assert.equal(i.t("hi"), "Hello {name}");
  i.setLang("it");
  assert.equal(i.t("hi", { name: "Ann" }), "Ciao Ann");
});

test("th escapes parameters and text, html() passes markup through", () => {
  const i = createTranslator(sample, "en", true);
  assert.equal(i.th("hi", { name: "<b>&" }), "Hello &lt;b&gt;&amp;");
  assert.equal(i.th("hi", { name: html("<code>x</code>") }), "Hello <code>x</code>");
  assert.equal(i.t("hi", { name: "<b>" }), "Hello <b>");
});

test("plurals follow the count in each language", () => {
  const i = createTranslator(sample, "en", true);
  assert.equal(i.t("rounds", { count: 1 }), "One round");
  assert.equal(i.t("rounds", { count: 3 }), "3 rounds");
  assert.equal(i.t("rounds", { count: 0 }), "0 rounds");
  i.setLang("it");
  assert.equal(i.t("rounds", { count: 1 }), "Un round");
  assert.equal(i.t("rounds", { count: 2 }), "2 round");
});

test("a missing key is a visible marker in development", () => {
  const i = createTranslator(sample, "en", true);
  i.setLang("it");
  assert.equal(i.t("only.en"), "⟦only.en⟧");
  assert.equal(i.t("nope"), "⟦nope⟧");
  assert.equal(i.th("nope"), "⟦nope⟧");
});

test("a missing key falls back to English in release", () => {
  const i = createTranslator(sample, "en", false);
  i.setLang("it");
  assert.equal(i.t("only.en"), "English only");
  assert.equal(i.t("nope"), "nope");
});

test("back-end messages: key with nested and typed parameters, English fallback", () => {
  const i = createTranslator(sample, "en", false);
  i.setLang("it");
  assert.equal(i.tr({ key: "on", params: { server: { key: "server.official" } }, message: "x" }), "Si gioca sul server ufficiale");
  assert.equal(i.tr({ key: "unknown.key", message: "English text" }), "English text");
  assert.equal(i.tr({ key: "", message: "[emulator] raw line" }), "[emulator] raw line");
  assert.equal(i.tr("plain"), "plain");
  assert.equal(i.t("hi", { name: { bytes: 2 ** 30 * 1.5 } }), "Ciao 1,5 GB");
  i.setLang("en");
  assert.equal(i.t("hi", { name: { bytes: 2 ** 30 * 1.5 } }), "Hello 1.5 GB");
  assert.equal(i.t("hi", { name: { seconds: 125 } }), "Hello 2 min 05 s");
  assert.equal(i.t("hi", { name: { list: [{ key: "server.official" }, { message: "raw" }] } }), "Hello official server, raw");
});

test("numbers use the language's decimal separator", () => {
  const i = createTranslator(sample, "en", true);
  assert.equal(i.number(0.07, 2), "0.07");
  i.setLang("it");
  assert.equal(i.number(0.07, 2), "0,07");
  assert.equal(i.size(43050220347), "40,1 GB");
  assert.equal(i.size(52596), "51 KB");
  assert.equal(i.size(1.5 * 2 ** 20), "1,5 MB");
});

test("lists come from the bundle", () => {
  const i = createTranslator({ en, it }, "en", true);
  assert.equal(i.list("nav.wizardSteps").length, 5);
  i.setLang("it");
  assert.equal(i.list("nav.wizardSteps")[0], "Lingua del launcher");
});

function leaves(node, path = "", out = new Map()) {
  if (typeof node === "string") out.set(path, node);
  else if (Array.isArray(node)) node.forEach((v, i) => leaves(v, `${path}.${i}`, out));
  else for (const [k, v] of Object.entries(node)) leaves(v, path ? `${path}.${k}` : k, out);
  return out;
}

const placeholders = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

test("the two bundles have the same keys, plural forms aside", () => {
  const norm = (m) => new Set([...m.keys()].map((k) => k.replace(/\.(one|other)$/, "")));
  const e = norm(leaves(en));
  const i = norm(leaves(it));
  assert.deepEqual([...e].filter((k) => !i.has(k)), [], "keys missing in it.json");
  assert.deepEqual([...i].filter((k) => !e.has(k)), [], "keys missing in en.json");
});

test("the two bundles use the same placeholders for each key", () => {
  const group = (m) => {
    const g = new Map();
    for (const [k, v] of m) {
      const base = k.replace(/\.(one|other)$/, "");
      g.set(base, [...new Set([...(g.get(base) ?? []), ...placeholders(v)])].sort());
    }
    return g;
  };
  const e = group(leaves(en));
  const i = group(leaves(it));
  const diff = [...e].filter(([k, p]) => JSON.stringify(p) !== JSON.stringify(i.get(k) ?? [])).map(([k]) => k);
  assert.deepEqual(diff, []);
});

test("no English text uses a dash aside where the Italian uses other punctuation", () => {
  const e = leaves(en);
  const i = leaves(it);
  const bad = [...e].filter(([k, v]) => v.includes(" — ") && !(i.get(k) ?? "").includes(" — ")).map(([k]) => k);
  assert.deepEqual(bad, []);
});
