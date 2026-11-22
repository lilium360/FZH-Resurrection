// Checks the texts of the launcher:
//  1. every i18n key named in the front end (src/) or the back end (src-tauri/src/) exists in both
//     copy/en.json and copy/it.json; keys built at run time (`nav.menu.${id}`) count as patterns;
//  2. no file under src/ui (and src/main.ts) holds English text for the player outside the bundles:
//     text between HTML tags, visible attributes (title, aria-label, placeholder, data-tip, alt) and
//     string literals that read like a sentence;
//  3. lists the bundle keys nothing uses.
// Exit code 1 when 1 or 2 finds something. Limits: a text assembled from single-word literals, or
// one passed through a variable named like a key, is not recognised as English.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const bundles = { en: JSON.parse(readFileSync(join(app, "copy/en.json"), "utf8")), it: JSON.parse(readFileSync(join(app, "copy/it.json"), "utf8")) };

function leaves(node, path = "", out = new Set()) {
  if (typeof node === "string" || Array.isArray(node) || (node && typeof node === "object" && "other" in node)) out.add(path);
  if (Array.isArray(node)) node.forEach((v, i) => leaves(v, `${path}.${i}`, out));
  else if (node && typeof node === "object" && !("other" in node)) for (const [k, v] of Object.entries(node)) leaves(v, path ? `${path}.${k}` : k, out);
  return out;
}
const keys = { en: leaves(bundles.en), it: leaves(bundles.it) };
const roots = new Set(Object.keys(bundles.en));

const files = (dir, ext) =>
  readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? files(p, ext) : ext.some((e) => n.endsWith(e)) ? [p] : [];
  });

const front = files(join(app, "src"), [".ts"]);
const back = files(join(app, "src-tauri/src"), [".rs"]);

const used = new Set();
const patterns = [];
const missing = [];
const keyLike = /^[a-zA-Z]+(\.[A-Za-z0-9_]+)+$/;

for (const file of [...front, ...back]) {
  const text = readFileSync(file, "utf8").replace(/^\s*\/\/.*$/gm, "");
  for (const m of text.matchAll(/"([a-zA-Z]+(?:\.[A-Za-z0-9_]+)+)"/g)) {
    const s = m[1];
    if (!keyLike.test(s) || !roots.has(s.split(".")[0])) continue;
    if (!keys.en.has(s) && /\.(json|ts|rs|txt|exe|pkg|bin|xml|sfo|pak)$/.test(s)) continue;
    used.add(s);
    for (const lang of ["en", "it"]) if (!keys[lang].has(s)) missing.push(`${relative(app, file)}: "${s}" not in ${lang}.json`);
  }
  for (const k of roots) if (!k.includes(".") && keys.en.has(k) && text.includes(`"${k}"`)) used.add(k);
  for (const m of text.matchAll(/`([a-zA-Z]+(?:\.[A-Za-z0-9_]*|\$\{[^}]+\})+[A-Za-z0-9_.]*)`/g)) {
    const s = m[1];
    if (!s.includes("${") || !roots.has(s.split(/[.$]/)[0])) continue;
    const re = new RegExp("^" + s.split(/\$\{[^}]+\}/).map((p) => p.replace(/[.]/g, "\\.")).join("[^.]+") + "$");
    patterns.push({ re, src: `${relative(app, file)}: \`${s}\`` });
  }
}
for (const p of patterns) if (![...keys.en].some((k) => p.re.test(k))) missing.push(`${p.src} matches no key`);

const isUsed = (k) => used.has(k) || patterns.some((p) => p.re.test(k)) || [...used].some((u) => k.startsWith(u + "."));
const unused = [...keys.en].filter((k) => !isUsed(k) && !/\.\d+$/.test(k));

// ---------- leftover English in the UI ----------
const ui = [...files(join(app, "src/ui"), [".ts"]), join(app, "src/main.ts")];
const leftovers = [];
const word = /[A-Za-z]{2,}/;
const allowed = new Set(["GB", "MB", "PLAY", "STOP", "F2", "F10"]);

function literals(src) {
  const out = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "/" && src[i + 1] === "/") {
      i = src.indexOf("\n", i);
      if (i < 0) break;
      continue;
    }
    if (c === "/" && src[i + 1] === "*") {
      i = src.indexOf("*/", i) + 2;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      let j = i + 1,
        s = "",
        depth = 0;
      while (j < src.length) {
        const d = src[j];
        if (d === "\\") {
          s += src[j + 1];
          j += 2;
          continue;
        }
        if (c === "`" && d === "$" && src[j + 1] === "{") {
          depth = 1;
          s += "\u0000";
          j += 2;
          while (j < src.length && depth) {
            if (src[j] === "{") depth++;
            else if (src[j] === "}") depth--;
            j++;
          }
          continue;
        }
        if (d === c) break;
        s += d;
        j++;
      }
      if (!/Error\($/.test(src.slice(Math.max(0, i - 6), i))) out.push({ s, line: src.slice(0, i).split("\n").length, quote: c });
      i = j + 1;
      continue;
    }
    i++;
  }
  return out;
}

const visibleAttr = /\b(?:title|aria-label|placeholder|data-tip|alt)="([^"]*)"/g;

for (const file of ui) {
  const src = readFileSync(file, "utf8");
  for (const { s, line } of literals(src)) {
    const found = [];
    if (s.includes("<") || s.includes(">")) {
      for (const m of s.matchAll(/>([^<]*)</g)) found.push(m[1]);
      for (const m of s.matchAll(visibleAttr)) found.push(m[1]);
    } else if (/^[A-Z][a-z']+[ ,:][\s\S]*[a-z]/.test(s) || /\b[a-z]+ [a-z]+ [a-z]+\b/.test(s.replace(/\b(btn|sm|fill|danger|on|off|ok|warn|idle|ic|dot|av|chip|acc|row|stack|rl|rc|sel|seg|tog|fld|w|ta|pic|up|lg|host|run)\b/g, ""))) {
      found.push(s);
    }
    for (const f of found) {
      const text = f.replace(/\u0000/g, " ").trim();
      if (!text || !word.test(text)) continue;
      if (text.split(/\s+/).every((w) => allowed.has(w) || !word.test(w))) continue;
      leftovers.push(`${relative(app, file)}:${line}: ${JSON.stringify(text.slice(0, 80))}`);
    }
  }
}

const show = (title, list) => console.log(`${title}: ${list.length}${list.length ? "\n  " + list.join("\n  ") : ""}`);
show("Keys used in code but missing from a bundle", missing);
show("Possible English left in src/ui", leftovers);
show("Bundle keys not used by the code", unused);
process.exit(missing.length || leftovers.length ? 1 : 0);
