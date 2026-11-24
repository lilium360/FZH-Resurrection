import banner from "../assets/banner.jpg";
import { CONTROLLER_BUTTONS, type DumpInfo, type Friend, type Msg, type PickKind, type Relay } from "../backend";
import { escapeHtml, has, html, number, size, t, th, tr } from "../i18n";
import { I } from "./icons";
import { KEYS, M, fileName, type SettingKey } from "./meta";
import { APP, CACHE, CHECK, F, PR, RELAYS, SAVED, UPD, app, installing, missing, needsInstall, pkgChosen, S, type AvatarChoice } from "./state";

export const esc = escapeHtml;

/** A back-end message as HTML. */
export const trh = (m: Msg | string | null | undefined): string => esc(tr(m));

export const ini = (n: string): string => (n || "?").slice(0, 2).toUpperCase();

export const AV = ["c1", "c2", "c3", "c4", "c5"];

/** The label of a setting value: an audio device, a game language or an `option.*` text. */
export function lab(v: string, k?: SettingKey): string {
  const d = [...app.devices.inputs, ...app.devices.outputs].find((x) => x.id === v);
  if (d) return d.label;
  if (k === "gameLanguage") return t(`gameLang.${v}`);
  if (k === "pixelDensity" && v === "default") return t("option.pixelDensityDefault");
  if (k === "shadowQuality" && v === "0") return t("option.off");
  return has(`option.${v}`) ? t(`option.${v}`) : v;
}

function meta(k: SettingKey) {
  const m = M[k];
  if (!m) throw new Error(`No metadata for setting ${k}`);
  return m;
}

const label = (k: SettingKey): string => th(`meta.${k}.label`);

export function options(k: SettingKey): readonly string[] {
  const withCurrent = (list: string[]) => {
    const v = String(S[k]);
    return list.includes(v) ? list : [...list, v];
  };
  if (k === "microphone") return withCurrent(["off", "default", ...app.devices.inputs.map((d) => d.id)]);
  if (k === "audioOutput") return withCurrent(["default", ...app.devices.outputs.map((d) => d.id)]);
  return meta(k).o ?? [];
}

export function fmt(k: SettingKey): string {
  const m = meta(k);
  const v = +S[k];
  const step = m.a?.[2] ?? 1;
  return number(v, step < 1 ? 2 : 0) + (m.u ?? "");
}

function ctl(k: SettingKey): string {
  const m = meta(k);
  const val = S[k];
  if (m.t === "tog")
    return `<button class="tog ${val ? "on" : ""}" data-t="${k}" role="switch" aria-checked="${!!val}" aria-label="${label(k)}"><i></i></button>`;
  if (m.t === "seg")
    return `<div class="seg" role="group" aria-label="${label(k)}">${options(k)
      .map((v) => `<button class="${val === v ? "on" : ""}" data-k="${k}" data-v="${v}">${esc(lab(v, k))}</button>`)
      .join("")}</div>`;
  if (m.t === "sel")
    return `<select class="sel" data-k="${k}" aria-label="${label(k)}">${options(k)
      .map((v) => `<option value="${esc(v)}" ${val === v ? "selected" : ""}>${esc(lab(v, k))}</option>`)
      .join("")}</select>`;
  if (m.t === "sld" && m.a) {
    const [a, b, s] = m.a;
    const n = +val;
    return `<div class="sl"><input type="range" data-k="${k}" min="${a}" max="${b}" step="${s}" value="${n}" aria-label="${label(k)}" style="--v:${(((n - a) / (b - a)) * 100).toFixed(1)}%"><output data-out="${k}">${esc(fmt(k))}</output></div>`;
  }
  return `<label class="fld"><input data-k="${k}" value="${esc(val)}" aria-label="${label(k)}" spellcheck="false"></label>`;
}

export const row = (k: SettingKey, off = false): string => {
  const m = meta(k);
  return `<div class="row ${m.st ? "stack" : ""} ${off ? "off" : ""}"><div class="rl"><b>${label(k)}</b>${m.x ? `<span class="chip">${th(`option.${m.x}`)}</span>` : ""}<span class="n">${th(`meta.${k}.note`)}</span></div><div class="rc">${ctl(k)}</div></div>`;
};

/** `t` is already HTML (from `th`). */
export const box = (t: string, ks: SettingKey[], off = false): string =>
  `<div class="b-rows"><h6>${t}</h6>${ks.map((k) => row(k, off)).join("")}</div>`;

export const head = (t: string, s: string): string => `<h1>${t}</h1><p class="sub">${s}</p>`;

export const btnRow = (t: string, n: string, btn: string): string =>
  `<div class="row"><div class="rl"><b>${t}</b><span class="n">${n}</span></div><div class="rc">${btn}</div></div>`;

export const avatar = (o: { id: string; av: AvatarChoice; img: string | null }, cls = ""): string =>
  o.av === "img" && o.img
    ? `<span class="av pic ${cls}" style="background-image:url(${esc(o.img)})"></span>`
    : `<span class="av ${cls} ${AV[typeof o.av === "number" ? o.av : 0] ?? AV[0]}">${esc(ini(o.id))}</span>`;

/** The banner of the Game screen. */
export const art = (): string =>
  `<div class="art pic" role="img" aria-label="${th("screen.play.artLabel")}" style="background-image:url(${banner})"></div>`;

export const logLines = (): string =>
  app.log.length
    ? app.log.map((l) => `<div><s>${esc(l[0])}</s> ${trh(l[1])}</div>`).join("")
    : `<div class="idle">${th("log.idle")}</div>`;

export const logBox = (): string =>
  `<div class="b-log"><div class="hd">${I("term", 15)}<b>${th("log.title")}</b><span>${th(app.run ? "log.running" : "log.idleState")}</span><span class="sp"></span><button data-report>${I("flag", 13)}${th("log.reportBtn")}</button><button data-copy>${I("copy", 13)}${th("log.copyBtn")}</button><button data-clear>${th("log.clearBtn")}</button></div><div class="log" data-log>${logLines()}</div></div>`;

const minutes = (s: number): number => Math.max(1, Math.round(s / 60));

/** The `warmupBlocker.*` key of why Warm up cannot start now, or "" when it can. */
export function warmupBlocker(): string {
  if (!CACHE.roundSeconds) return "warmupBlocker.scriptMissing";
  if (app.run) return "warmupBlocker.gameRunning";
  if (!F.base || !F.update) return "warmupBlocker.noFolder";
  if (installing()) return "warmupBlocker.installing";
  if (needsInstall()) return "warmupBlocker.needsInstall";
  if (F.prep === 1) return "warmupBlocker.preparing";
  if (!PR.in) return "warmupBlocker.notSignedIn";
  if (!app.pub.online) return "warmupBlocker.serverOffline";
  return "";
}

export function cacheBox(): string {
  const s = CACHE.status;
  const round = minutes(CACHE.roundSeconds);
  const label =
    s === "ready" ? th("cache.ready") : s === "empty" ? th("cache.empty") : th("cache.warming", { round: CACHE.round, rounds: CACHE.rounds });
  const title = `<h6>${th("cache.title")}</h6>`;
  if (s === "warming")
    return `<div class="b-rows b-pad">${title}<div class="b-cache"><i class="dot"></i><b>${label}</b></div><p class="b-note">${th("cache.warmingNote", { minutes: round })}</p><div class="b-acts"><button class="btn" data-cache="cancel">${th("cache.cancelBtn")}</button><button class="btn" data-cache="clear" disabled>${th("cache.clearBtn")}</button></div></div>`;
  const total = CACHE.roundSeconds
    ? t("cache.totalTime", { round, min: minutes(CACHE.minRounds * CACHE.roundSeconds), max: minutes(CACHE.rounds * CACHE.roundSeconds) })
    : "";
  const what = (s === "empty" ? th("cache.emptyNote") + " " : "") + th("cache.generalNote", { total });
  const blocker = warmupBlocker();
  return `<div class="b-rows b-pad">${title}<div class="b-cache"><i class="dot ${s === "ready" ? "ok" : ""}"></i><b>${label}</b></div><p class="b-note">${what}</p>${blocker ? `<p class="b-note">${th(blocker)}</p>` : ""}<div class="b-acts"><button class="btn fill" data-cache="warm" ${blocker ? "disabled" : ""}>${th("cache.warmUpBtn")}</button><button class="btn" data-cache="clear" ${s === "ready" && !app.run ? "" : "disabled"}>${th("cache.clearBtn")}</button></div></div>`;
}

export function checkBox(): string {
  const rows = CONTROLLER_BUTTONS.map((b) => {
    const n = CHECK.presses[b] ?? 0;
    return `<div class="b-key"><span>${th(`check.button.${b}`)}</span><i class="dot ${n ? "ok" : ""}"></i><b>${n ? th("check.pressed", { count: n }) : th("check.notPressed")}</b></div>`;
  }).join("");
  const profile = CHECK.profile ? `<p class="b-note">${th("check.profile", { profile: CHECK.profile.replace("/interaction_profiles/", "") })}</p>` : "";
  return `<div class="b-rows b-pad" style="margin-top:12px"><div class="b-calh"><h6>${th("check.title")}</h6><button class="btn sm" data-check>${th("check.refresh")}</button></div><p class="b-note">${th("check.note")}</p>${rows}${profile}</div>`;
}

export const keysBox = (): string =>
  `<div class="b-rows b-pad"><h6>${th("screen.settings.shortcuts")}</h6><p class="b-note">${th("keyNote")}</p>${KEYS.map(
    ([k, d]) => `<div class="b-key"><span>${th(`key.${d}`)}</span>${k.map((x) => `<kbd>${x}</kbd>`).join("+")}</div>`,
  ).join("")}</div>`;

export const shortVersion = (v: string): string => v.replace(/^0+(?=\d)/, "");

export const merged = (): boolean => !!F.base && !!F.update && F.base.path === F.update.path;

export const gb = (bytes: number): string => size(bytes);

const kindOf = (): string => {
  const pkgs = [F.base, F.update].filter((d) => d?.source === "pkg").length;
  if (pkgs === 2) return t("recognised.pkgFiles");
  if (pkgs === 1) return t("recognised.pkgAndFolder");
  return t(merged() ? "recognised.mergedFolder" : "recognised.twoFolders");
};

const REGIONS: Record<string, string> = { Europe: "europe", USA: "usa", Japan: "japan", Asia: "asia" };

/** A region name from the back end ("Europe") in the launcher language. */
export const regionName = (r: string): string => t(`dump.region.${REGIONS[r] ?? "unknown"}`);

/** What the chosen folder was recognised as, e.g. "CUSA11182 · Europe · base 1.00 + update 1.43 · PKG files". */
export function recognised(): string {
  const d: DumpInfo | null = F.base ?? F.update;
  if (!d) return "";
  const p = { title: d.titleId, region: regionName(d.region) };
  if (F.base && F.update)
    return t("recognised.both", { ...p, base: shortVersion(F.base.version), update: shortVersion(F.update.version), kind: kindOf() });
  const pkg = d.source === "pkg" ? ` · ${t("recognised.pkgFile")}` : "";
  return F.base
    ? `${t("recognised.baseOnly", { ...p, version: shortVersion(F.base.version) })}${pkg} · ${t("recognised.updateMissing")}`
    : `${t("recognised.updateOnly", { ...p, version: shortVersion(d.version) })}${pkg} · ${t("recognised.baseMissing")}`;
}

const clock = (secs: number): string => {
  const m = Math.max(1, Math.round(secs / 60));
  return m < 60 ? t("progress.timeMinutes", { count: m }) : t("progress.timeHours", { hours: Math.floor(m / 60), minutes: m % 60 });
};

/** "12.3 of 40.1 GB · 31% · about 4 min left". */
export function progressText(): string {
  const p = F.prog;
  if (!p) return t("progress.starting");
  const pct = p.total ? Math.floor((p.done / p.total) * 100) : 0;
  let left = "";
  const s = F.progStart;
  if (s && p.done > s.done && Date.now() - s.t > 3000) {
    const rate = (p.done - s.done) / ((Date.now() - s.t) / 1000);
    left = ` · ${clock((p.total - p.done) / rate)}`;
  }
  return t("progress.text", { done: gb(p.done), total: gb(p.total), pct }) + left;
}

const progressPct = (): number => (F.prog?.total ? (F.prog.done / F.prog.total) * 100 : 0);

/** Updates the progress elements in place; false when none is on screen. */
export function updateProgress(): boolean {
  const texts = document.querySelectorAll<HTMLElement>("[data-iprog]");
  texts.forEach((t) => (t.textContent = progressText()));
  document.querySelectorAll<HTMLElement>("[data-ibar]").forEach((b) => (b.style.width = `${progressPct().toFixed(1)}%`));
  document.querySelectorAll<HTMLElement>("[data-ifile]").forEach((f) => (f.textContent = F.prog?.file ?? ""));
  return texts.length > 0;
}

export const progressBar = (): string =>
  `<span class="no" data-iprog>${esc(progressText())}</span><span class="b-prog"><i data-ibar style="width:${progressPct().toFixed(1)}%"></i></span><code data-ifile>${esc(F.prog?.file ?? "")}</code>`;

const drive = (path: string): string => /^[A-Za-z]:/.exec(path)?.[0] ?? t("install.thisDrive");

/** The space line under the install location. */
export function planText(): string {
  const p = F.plan;
  if (F.planErr) return `<span class="bad">${trh(F.planErr)}</span>`;
  if (!p) return `<span class="no">${th("install.checkingSpace")}</span>`;
  if (p.error) return `<span class="bad">${trh(p.error)}</span>`;
  const parts = [th("install.needs", { size: gb(p.required) })];
  if (p.free !== null) parts.push(th("install.freeOnDrive", { free: gb(p.free), drive: drive(p.target) }));
  if (p.present > 0) parts.push(th("install.alreadyExtracted", { size: gb(p.present) }));
  return `<span class="no">${parts.join(" · ")}</span>`;
}

const busyGame = (): boolean => app.run || F.prep === 1 || installing();

/** Installation from PKG files: location, space, progress. Null when no PKG file is chosen. */
function installStep(n: number): string {
  if (!pkgChosen()) return "";
  const ready = !!F.base && !!F.update;
  const inst = F.inst;
  const title = th("gameSteps.installTheGame");
  if (inst?.complete)
    return step(n, title, "done", `<span class="okt">${th("wizard.step1.installDone", { size: gb(inst.size) })}</span><code>${esc(inst.target)}</code>`, "");
  if (installing()) return step(n, title, "cur", progressBar(), `<button class="btn" data-install="cancel">${th("install.cancel")}</button>`);
  const target = F.plan?.target ?? inst?.target ?? "";
  const sub = ready
    ? `<span class="no">${th("install.pkgNote")}</span><span class="b-loc"><b>${th("install.installLocation")}</b><code>${esc(target)}</code></span>${planText()}${F.instErr ? `<span class="bad">${trh(F.instErr)}</span>` : ""}`
    : `<span class="no">${th("install.waitForRecognition")}</span>`;
  const blocked = !ready || !F.plan || !!F.plan.error || busyGame();
  const btns = ready
    ? `<span class="b-btns"><button class="btn" data-install="location" ${busyGame() ? "disabled" : ""}>${th("install.change")}</button><button class="btn fill" data-install="start" ${blocked ? "disabled" : ""}>${th(F.plan?.present ? "install.resume" : "install.installBtn")}</button></span>`
    : "";
  return step(n, title, F.instErr ? "err" : ready ? "cur" : "lock", sub, btns);
}

/** Folders extracted earlier that the current choice does not use, with a Delete button each. */
export function leftoverRows(): string {
  if (!F.leftovers.length) return "";
  return `<div class="b-rows" style="margin-top:12px"><h6>${th("install.leftoverTitle")}</h6>${F.leftovers
    .map((l) =>
      btnRow(
        esc(gb(l.size)),
        th("install.leftoverNote", { target: l.target }),
        `<button class="btn" data-uninstall="${esc(l.target)}" data-size="${l.size}" ${busyGame() ? "disabled" : ""}>${th("install.deleteBtn")}</button>`,
      ),
    )
    .join("")}</div>`;
}

/** The installed copy of the current game, with the button that frees its space. */
export function installedRow(): string {
  const i = F.inst;
  if (!i?.complete) return "";
  return `<div class="b-rows" style="margin-top:12px">${btnRow(
    th("install.installedCopy"),
    th("install.installedCopyNote", { target: html(`<code>${esc(i.target)}</code>`), size: gb(i.size) }),
    `<button class="btn" data-uninstall="${esc(i.target)}" data-size="${i.size}" ${busyGame() ? "disabled" : ""}>${th("install.deleteInstalledCopy")}</button>`,
  )}</div>`;
}

export const step = (n: number, t: string, st: string, sub: string, btn: string): string =>
  `<div class="b-stp ${st}"><span class="nb">${st === "done" ? I("check", 13) : st === "err" ? I("x", 13) : n}</span><div class="bd"><b>${t}</b>${sub}</div>${btn}</div>`;

const chooseBtn = (k: PickKind, label: string, fill = false): string =>
  `<button class="btn${fill ? " fill" : ""}" data-choose="${k}" ${busyGame() ? "disabled" : ""}>${I("folder", 15)}${label}</button>`;

const paths = (): string => {
  const list = merged() || !F.update ? [F.base?.path] : !F.base ? [F.update.path] : [F.base.path, F.update.path];
  return list.map((p) => `<code>${esc(p ?? "")}</code>`).join("");
};

/** A rejected folder: the reason, and what it was recognised as. */
export const rejection = (): string => `<span class="bad">${trh(F.err)}</span>${F.found && tr(F.found) ? `<code>${trh(F.found)}</code>` : ""}`;

/** The game step: one folder, a second picker when only one half was found, then the automatic preparation. */
export function gameSteps(): string {
  const need = missing();
  const any = !!F.base || !!F.update;
  const sub1 =
    F.err && !need
      ? rejection()
      : any
        ? `<span class="okt">${esc(recognised())}</span>${paths()}`
        : `<span class="no">${th("gameSteps.noFolderChosen")}</span>`;
  let h = step(1, esc(fileName("game")), F.err && !need ? "err" : any ? "done" : "cur", sub1, chooseBtn("game", th(any ? "install.change" : "screen.play.chooseFolder")));
  if (need) {
    const sub = F.err
      ? rejection()
      : F.note
        ? `<span class="bad">${trh(F.note)}</span>`
        : `<span class="no">${th(need === "update" ? "gameSteps.needUpdatePkg" : "gameSteps.needBasePkg")}</span>`;
    h += step(2, esc(fileName(need)), F.err ? "err" : "cur", sub, chooseBtn(need, th("screen.play.chooseFolder"), true));
  }
  let n = need ? 3 : 2;
  const inst = installStep(n);
  if (inst) {
    h += inst;
    n++;
  }
  const ready = !!F.base && !!F.update && !needsInstall();
  const waiting = th(pkgChosen() ? "gameSteps.prepAutoWaitPkg" : "gameSteps.prepAutoWait");
  const sub3 =
    F.prep === 1
      ? `<span class="no">${th("gameSteps.preparingText")}</span>`
      : F.prepErr
        ? `<span class="bad">${trh(F.prepErr)}</span>`
        : `<span class="no">${ready ? th("gameSteps.prepAutoReady") : waiting}</span>`;
  const retry = F.prepErr && F.prep === 0 && ready ? `<button class="btn" data-prepare>${th("gameSteps.tryAgain")}</button>` : "";
  h += step(n, th("gameSteps.prepareTheGame"), F.prepErr ? "err" : F.prep === 1 ? "cur" : ready ? "cur" : "lock", sub3, retry);
  return h;
}

/** "Downloading… 42%" while the update downloads. */
export function updDownloading(): string {
  const p = UPD.prog;
  return p?.total ? th("screen.about.downloadingPercent", { percent: Math.floor((p.downloaded / p.total) * 100) }) : th("screen.about.downloading");
}

/** The button that moves the update on: download, progress or restart. */
export function updAction(cls: string): string {
  if (UPD.phase === "ready") return `<button class="${cls}" data-upd="restart">${th("screen.about.restart")}</button>`;
  if (UPD.phase === "downloading") return `<button class="${cls}" disabled>${updDownloading()}</button>`;
  return `<button class="${cls}" data-upd="download">${th("screen.about.download")}</button>`;
}

export const updBanner = (): string =>
  ["available", "downloading", "ready"].includes(UPD.phase) && !UPD.hidden
    ? `<div class="b-upd">${I("box", 16)}<span><b>${esc(APP)}</b> ${th(UPD.phase === "ready" ? "screen.about.readyTitle" : "screen.about.versionAvailable", { version: UPD.next })}</span><button class="b-link" data-upd="notes">${th("screen.about.whatsNew")}</button>${updAction("btn sm fill")}<button class="x" data-upd="hide" aria-label="${th("screen.about.hideUpdate")}" data-tip="${th("screen.about.hideUpdate")}">${I("x", 14)}</button></div>`
    : "";

function ago(iso: string | null): string {
  if (!iso) return t("presence.offline");
  const mins = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 60000));
  if (mins < 60) return mins <= 1 ? t("presence.lastSeenMinute") : t("presence.lastSeenMinutes", { count: mins });
  const h = Math.round(mins / 60);
  if (h < 24) return h === 1 ? t("presence.lastSeenHour") : t("presence.lastSeenHours", { count: h });
  const d = Math.round(h / 24);
  return d === 1 ? t("presence.lastSeenYesterday") : t("presence.lastSeenDays", { count: d });
}

export function presenceText(f: Friend): string {
  const p = f.presence;
  if (p.state === "in-game") return t("presence.inGame", { mode: p.mode });
  if (p.state === "menu") return t("presence.inMainMenu");
  if (p.state === "online") return t("presence.online");
  return ago(p.lastSeen);
}

/** The regions of the public list in the order they are shown; any other name follows as the server gives it. */
const RELAY_REGIONS = ["europe", "northAmerica", "southAmerica", "asia", "oceania", "africa"];

function relayRegions(relays: Relay[]): [string, Relay[]][] {
  const groups = new Map<string, { rank: number; label: string; relays: Relay[] }>();
  for (const r of relays) {
    const name = r.region.trim();
    const rank = RELAY_REGIONS.findIndex((k) => k.toLowerCase() === name.replace(/\s+/g, "").toLowerCase());
    const id = rank < 0 ? name : (RELAY_REGIONS[rank] ?? name);
    const g = groups.get(id) ?? { rank: rank < 0 ? RELAY_REGIONS.length : rank, label: rank < 0 ? name || t("relays.region.unnamed") : t(`relays.region.${id}`), relays: [] };
    g.relays.push(r);
    groups.set(id, g);
  }
  return [...groups.values()].sort((a, b) => a.rank - b.rank).map((g) => [g.label, g.relays]);
}

function relaysChecked(iso: string | null): string {
  const mins = iso ? Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 60000)) : NaN;
  if (Number.isNaN(mins)) return t("relays.notChecked");
  if (mins < 1) return t("relays.updatedNow");
  return mins < 60 ? t("relays.updatedMinutes", { count: mins }) : t("relays.updatedHours", { count: Math.round(mins / 60) });
}

/** The relay list of the server in use, or one line when there is none to show. */
export function relaysBody(): string {
  const list = RELAYS.data?.relays ?? [];
  const failed = RELAYS.failed ? `${th("relays.unavailable")} <button class="b-link" data-relays>${th("relays.retry")}</button>` : "";
  if (!list.length)
    return `<p class="b-note">${failed || th(RELAYS.data === undefined ? "relays.loading" : "relays.noList")}</p>`;
  const status = (r: Relay) =>
    r.online === null
      ? `<span class="st"><i class="dot"></i>${th("relays.unknown")}</span>`
      : `<span class="st ${r.online ? "ok" : "bad"}"><i class="dot ${r.online ? "ok" : "bad"}"></i>${th(r.online ? "relays.online" : "relays.offline")}</span>`;
  return (
    `<div class="b-two" style="gap:6px 40px;margin-top:0">${relayRegions(list)
      .map(([region, relays]) => `<div><h6>${esc(region)}</h6>${relays.map((r) => btnRow(esc(r.location), th("relays.hostedBy", { operator: r.operator }), status(r))).join("")}</div>`)
      .join("")}</div>` +
    `<p class="b-note b-sub">${failed || esc(relaysChecked(RELAYS.data?.checkedAt ?? null))} ${th("relays.hostQuestion")} <button class="b-link" data-link="relays">${th("relays.hostLink")}</button></p>`
  );
}

/** The relay section. Not shown while the switch differs from the saved choice, nor for a private server without a list. */
export const relaysBox = (): string =>
  S.usePrivateServer !== SAVED.usePrivateServer || (SAVED.usePrivateServer && !RELAYS.data?.relays.length)
    ? ""
    : `<div class="b-rows b-pad" style="margin-top:12px"><h6>${th("relays.title")}</h6><p class="b-note">${th("relays.intro")}</p><div data-relays-body>${relaysBody()}</div></div>`;

export interface FriendRow {
  id: string;
  online: boolean;
  status: string;
}

export function friendRows(): FriendRow[] {
  return [
    ...app.friends.friends.map((f) => ({
      id: f.onlineId,
      online: f.online,
      status: presenceText(f),
    })),
    ...app.friends.outgoing.map((id) => ({ id, online: false, status: t("screen.friends.requestSent") })),
  ];
}
