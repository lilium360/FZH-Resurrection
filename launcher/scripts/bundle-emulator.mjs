// Fills src-tauri/bundle/ (bundled as app resources by tauri.conf.json) with the emulator build,
// the Visual C++ runtime DLLs it imports, its commit and the licence texts, the project's LICENSE and
// THIRD-PARTY-NOTICES.md among them. Runs before `tauri build`.
//
// FZH_EMULATOR_BUILD   folder with shadps4.exe (default <repo>/emulator/build/win)
// FZH_EMULATOR_SOURCE  the emulator source tree (default <repo>/emulator)
// FZH_VC_CRT           folder with the x64 Visual C++ runtime DLLs (default: newest Microsoft.VC14*.CRT
//                      of a local Visual Studio installation)
import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(app, "..");
const build = resolve(process.env.FZH_EMULATOR_BUILD || join(repo, "emulator", "build", "win"));
const source = resolve(process.env.FZH_EMULATOR_SOURCE || join(repo, "emulator"));
const out = join(app, "src-tauri", "bundle");
const emuOut = join(out, "emulator");
const licOut = join(out, "licences");

function fail(message) {
  console.error(`bundle-emulator: ${message}`);
  process.exit(1);
}

function peImports(file) {
  const data = readFileSync(file);
  const pe = data.readUInt32LE(0x3c);
  const sections = data.readUInt16LE(pe + 6);
  const optSize = data.readUInt16LE(pe + 20);
  const opt = pe + 24;
  const table = [];
  for (let i = 0; i < sections; i++) {
    const o = opt + optSize + i * 40;
    table.push({ va: data.readUInt32LE(o + 12), size: Math.max(data.readUInt32LE(o + 8), data.readUInt32LE(o + 16)), raw: data.readUInt32LE(o + 20) });
  }
  const off = (rva) => {
    const s = table.find((t) => rva >= t.va && rva < t.va + t.size);
    if (!s) throw new Error(`bad RVA ${rva}`);
    return rva - s.va + s.raw;
  };
  const cstr = (o) => data.toString("latin1", o, data.indexOf(0, o));
  const names = [];
  for (const [index, step, nameAt] of [[1, 20, 12], [13, 32, 4]]) {
    const rva = data.readUInt32LE(opt + 112 + index * 8);
    if (!rva) continue;
    for (let o = off(rva); ; o += step) {
      const name = data.readUInt32LE(o + nameAt);
      if (!name) break;
      names.push(cstr(off(name)));
    }
  }
  return names;
}

function findCrt() {
  if (process.env.FZH_VC_CRT) return resolve(process.env.FZH_VC_CRT);
  const found = [];
  for (const root of [process.env["ProgramFiles(x86)"], process.env.ProgramFiles].filter(Boolean)) {
    const vs = join(root, "Microsoft Visual Studio");
    if (!existsSync(vs)) continue;
    for (const year of readdirSync(vs)) {
      for (const edition of safeList(join(vs, year))) {
        const redist = join(vs, year, edition, "VC", "Redist", "MSVC");
        for (const version of safeList(redist)) {
          for (const crt of safeList(join(redist, version, "x64"))) {
            if (/^Microsoft\.VC14\d\.CRT$/.test(crt)) found.push({ version, path: join(redist, version, "x64", crt) });
          }
        }
      }
    }
  }
  const cmp = (a, b) => a.version.localeCompare(b.version, undefined, { numeric: true });
  return found.sort(cmp).pop()?.path;
}

function safeList(dir) {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

function copy(from, to) {
  mkdirSync(dirname(to), { recursive: true });
  copyFileSync(from, to);
}

const exe = join(build, "shadps4.exe");
if (!existsSync(exe)) fail(`${exe} not found`);
rmSync(out, { recursive: true, force: true });
mkdirSync(emuOut, { recursive: true });
mkdirSync(licOut, { recursive: true });

copy(exe, join(emuOut, "shadps4.exe"));

const crt = findCrt();
if (!crt) fail("no Visual C++ runtime folder found (set FZH_VC_CRT)");
const crtFiles = new Map(readdirSync(crt).map((f) => [f.toLowerCase(), f]));
const runtime = peImports(exe)
  .map((n) => crtFiles.get(n.toLowerCase()))
  .filter(Boolean);
for (const dll of runtime) copy(join(crt, dll), join(emuOut, dll));

const git = (...args) => execFileSync("git", ["-C", source, ...args], { encoding: "utf8" }).trim();
const commit = git("rev-parse", "HEAD");
const dirty = git("status", "--porcelain", "--untracked-files=no", "--", ".") !== "";
writeFileSync(join(emuOut, "commit.txt"), `${commit}${dirty ? "-dirty" : ""}\n`);

for (const f of ["LICENSE", "THIRD-PARTY-NOTICES.md"]) if (!existsSync(join(repo, f))) fail(`${join(repo, f)} not found`);
copy(join(repo, "LICENSE"), join(licOut, "LICENSE.txt"));
copy(join(repo, "THIRD-PARTY-NOTICES.md"), join(licOut, "THIRD-PARTY-NOTICES.md"));
const emuLic = join(licOut, "emulator");
copy(join(source, "LICENSE"), join(emuLic, "LICENSE.txt"));
if (existsSync(join(source, "THIRD-PARTY-NOTICES.md"))) copy(join(source, "THIRD-PARTY-NOTICES.md"), join(emuLic, "THIRD-PARTY-NOTICES.md"));
for (const f of safeList(join(source, "LICENSES"))) copy(join(source, "LICENSES", f), join(emuLic, "LICENSES", f));
const externals = [];
for (const dep of safeList(join(source, "externals")).sort()) {
  const dir = join(source, "externals", dep);
  if (!statSync(dir).isDirectory()) continue;
  const files = safeList(dir).filter((f) => /^(licen[cs]e|copying|notice)/i.test(f) && statSync(join(dir, f)).isFile());
  for (const f of files) copy(join(dir, f), join(emuLic, "externals", dep, f));
  if (files.length) externals.push(dep);
}
for (const font of ["barlow", "lekton"]) {
  const f = join(app, "node_modules", "@fontsource", font, "LICENSE");
  if (existsSync(f)) copy(f, join(licOut, "launcher", "fonts", `${font}-LICENSE.txt`));
}

writeFileSync(
  join(licOut, "README.txt"),
  [
    "FZH:Resurrection: licences of the bundled software",
    "",
    "FZH:Resurrection: GPL-2.0-or-later (LICENSE.txt). Its components and their licences: THIRD-PARTY-NOTICES.md.",
    "",
    `Emulator: shadPS4 fork (Farpoint PC VR tree), commit ${commit}${dirty ? " with local changes" : ""}.`,
    "Licence: GPL-2.0-or-later (emulator\\LICENSE.txt).",
    "Licences of its components: emulator\\LICENSES\\ and emulator\\externals\\<component>\\",
    `(${externals.length} components of the source tree, also some that are not used by the Windows build).`,
    "",
    `Visual C++ runtime (${runtime.join(", ")}): Microsoft redistributable files, from ${crt.split(/[\\/]/).slice(-3).join("\\")}.`,
    "Fonts of the launcher (Barlow, Lekton): SIL Open Font License 1.1, launcher\\fonts\\.",
    "Launcher: Tauri (MIT or Apache-2.0) and other Rust and npm packages under their own licences.",
    "",
    "Corresponding source: https://github.com/lilium360/FZH-Resurrection",
    "",
  ].join("\r\n"),
);

const size = (dir) => readdirSync(dir, { recursive: true }).reduce((n, f) => n + (statSync(join(dir, f)).isFile() ? statSync(join(dir, f)).size : 0), 0);
console.log(`bundle-emulator: commit ${commit}${dirty ? "-dirty" : ""}, runtime ${runtime.join(" ")}`);
console.log(`bundle-emulator: emulator ${(size(emuOut) / 1048576).toFixed(1)} MB, licences ${(size(licOut) / 1024).toFixed(0)} KB`);
