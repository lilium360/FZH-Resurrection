import { th } from "../i18n";
import {
  art,
  AV,
  avatar,
  box,
  btnRow,
  cacheBox,
  checkBox,
  esc,
  friendRows,
  gameSteps,
  head,
  ini,
  installedRow,
  leftoverRows,
  keysBox,
  logBox,
  recognised,
  relaysBox,
  row,
  trh,
  updAction,
  updBanner,
} from "./components";
import { I } from "./icons";
import { GRIP_KEYS, SERVICES, licences, type ScreenId } from "./meta";
import { APP, DP, F, PR, S, SAVED, UPD, app, srvName } from "./state";

/** The recovery email is hidden until shadNet can use it; the code path stays. */
export const SHOW_EMAIL = false;

const btn = (attrs: string, key: string, cls = "btn") => `<button class="${cls}" ${attrs}>${th(key)}</button>`;

function updateRow(): string {
  const version = { version: UPD.next };
  switch (UPD.phase) {
    case "checking":
      return btnRow(th("screen.about.launcherVersion", { version: UPD.cur }), th("screen.about.checking"), btn('data-upd="check" disabled', "screen.about.checkForUpdates"));
    case "latest":
      return btnRow(th("screen.about.latestVersion"), th("screen.about.updateCurrent", { version: UPD.cur }), btn('data-upd="check"', "screen.about.checkForUpdates"));
    case "failed":
      return btnRow(th("screen.about.updateFailed"), trh(UPD.err), btn('data-upd="check"', "screen.about.tryAgain"));
    case "available":
    case "downloading":
      return btnRow(th("screen.about.versionAvailable", version), th("screen.about.updateSeeChanges"), btn('data-upd="notes"', "screen.about.whatsNew") + updAction("btn fill"));
    case "ready":
      return btnRow(th("screen.about.readyTitle", version), th("screen.about.readyNote"), updAction("btn fill"));
    default:
      return btnRow(th("screen.about.launcherVersion", { version: UPD.cur }), th("screen.about.updateIdle"), btn('data-upd="check"', "screen.about.checkForUpdates"));
  }
}

export const SCREENS: Record<ScreenId, () => string> = {
  play() {
    const top = updBanner() + `<div class="b-banner">${art()}</div>`;
    if (F.prep === 2 && F.base && F.update)
      return (
        top +
        (PR.in && PR.elsewhere && !app.run
          ? `<div class="b-ready hold" role="status"><span class="nb">${I("alert", 14)}</span><div><b>${th("screen.play.accountInUse")}</b><small>${th("screen.play.accountInUseNote", { id: PR.id })}</small></div></div>`
          : "") +
        `<div class="b-ready"><span class="nb">${I("check", 14)}</span><div><b>${th("screen.play.ready")}</b><small>${esc(recognised())}</small></div>${btn(`data-files ${app.run ? "disabled" : ""}`, "screen.play.changeFolder", "btn sm")}</div>
    <p class="b-note" style="margin:10px 2px 0">${th("screen.play.firewallNote")}</p>${installedRow()}${leftoverRows()}${logBox()}`
      );
    return top + `<section class="b-dump"><p>${th("screen.play.dumpIntro")}</p>
    ${gameSteps()}</section>${leftoverRows()}${logBox()}`;
  },

  settings() {
    return (
      head(th("screen.settings.title"), th("screen.settings.subtitle")) +
      `<div class="b-two" style="margin-top:22px"><div class="b-col">${box(th("screen.settings.audio"), ["microphone", "audioOutput"])}${box(th("screen.settings.language"), ["launcherLanguage", "gameLanguage"])}
   <div class="b-rows"><h6>${th("screen.settings.setupAssistant")}</h6>${btnRow(th("screen.settings.setupRow"), th("screen.settings.setupNote"), btn("data-wizard", "screen.settings.setupBtn"))}</div></div>${keysBox()}</div>`
    );
  },

  graphics() {
    return (
      head(th("screen.graphics.title"), th("screen.graphics.subtitle")) +
      `<div class="b-two" style="margin-top:22px"><div class="b-col">${box(th("screen.graphics.smoothness"), ["frameRate", "asyncShaders"])}${box(th("screen.graphics.image"), ["pixelDensity", "sharpness", "fieldOfView"])}</div><div class="b-col">${box(th("screen.graphics.quality"), ["shadowQuality", "anisotropicFiltering", "antialiasing"])}${cacheBox()}${box(th("screen.graphics.diagnostics"), ["fpsCounter", "frameStats"])}</div></div>`
    );
  },

  gamepad() {
    const opt = (v: string, ic: string, title: string, note: string) =>
      `<button class="b-opt ${S.controller === v ? "on" : ""}" data-k="controller" data-v="${v}"><span class="ic">${I(ic, 20)}</span><span><b>${th(title)}</b><small>${th(note)}</small></span>${S.controller === v ? `<span class="chip acc" style="margin-left:auto">${th("screen.gamepad.inUse")}</span>` : ""}</button>`;
    const aim = S.controller === "aim";
    return (
      head(th("screen.gamepad.title"), th("screen.gamepad.subtitle")) +
      `<div class="b-pick">${opt("aim", "aim", "screen.gamepad.aim", "screen.gamepad.aimNote")}${opt("pad", "pad", "screen.gamepad.pad", "screen.gamepad.padNote")}</div>
   <div class="b-rows" style="margin-top:12px"><div class="b-calh"><h6>${th("screen.gamepad.calibration")}</h6>${btn(`data-grip ${aim ? "" : "disabled"}`, "screen.gamepad.reset", "btn sm")}</div>
   ${GRIP_KEYS.map((k) => row(k, !aim)).join("")}${aim ? "" : `<p class="b-note">${th("screen.gamepad.calibrationNote")}</p>`}</div>${checkBox()}`
    );
  },

  mods() {
    return (
      head(th("screen.mods.title"), th("screen.mods.subtitle")) +
      `<div class="b-two" style="margin-top:22px"><div class="b-col">${box(th("screen.mods.comfort"), ["modFreeTurn"])}</div></div>`
    );
  },

  advanced() {
    const priv = S.usePrivateServer,
      up = app.host;
    const svc = (k: keyof typeof SERVICES) =>
      `<div class="b-card"><div class="k">${I(SERVICES[k].ic, 15)}${th(`service.${k}.label`)}</div><div class="st ${up ? "on" : ""}" style="font-size:13px;font-weight:650"><i class="dot ${up ? "ok" : ""}"></i>${th(up ? "screen.advanced.runningOnThisPc" : "screen.advanced.stopped")}</div><p>${th(`service.${k}.note`)}</p></div>`;
    const pub = app.pub;
    const canHost = app.info.hosting;
    const status = `<b>${
      pub.online
        ? pub.players === null
          ? th("screen.advanced.online")
          : th("screen.advanced.onlineWithPlayers", { count: pub.players })
        : th(pub.network ? "screen.advanced.networkOnly" : "screen.advanced.offline")
    }</b>`;
    const hostLabel = th("screen.advanced.hostLabel");
    const hostRow = canHost
      ? btnRow(
          hostLabel,
          th("screen.advanced.hostOnNote"),
          `<button class="tog ${app.host ? "on" : ""}" data-host role="switch" aria-checked="${app.host}" aria-label="${hostLabel}"><i></i></button>`,
        )
      : btnRow(
          hostLabel,
          th("screen.advanced.hostUnavailable"),
          `<button class="tog" data-host role="switch" aria-checked="false" aria-label="${hostLabel}" disabled><i></i></button>`,
        );
    const saved = SAVED.usePrivateServer && SAVED.privateServer;
    const privStatus = saved
      ? `<div class="b-rows b-pad"><h6>${th("screen.advanced.privateServer")}</h6><div class="b-cache"><i class="dot ${pub.online ? "ok" : ""}"></i>${status}</div><p class="b-note">${th("screen.advanced.privateAskOwner", { address: SAVED.privateServer })}</p></div>`
      : `<div class="b-rows b-pad"><h6>${th("screen.advanced.privateServer")}</h6><p class="b-note">${th("screen.advanced.privateEnterAddress")}</p></div>`;
    return (
      head(th("screen.advanced.title"), th("screen.advanced.subtitle")) +
      `<div class="b-rows" style="margin:22px 0 12px">${row("usePrivateServer")}${priv ? row("privateServer") + hostRow : ""}</div>
   ${
     priv
       ? canHost
         ? `<div class="b-grid">${svc("network")}${svc("progression")}${svc("relay")}</div>`
         : privStatus
       : `<div class="b-rows b-pad"><h6>${th("screen.advanced.officialServer")}</h6><div class="b-cache"><i class="dot ${pub.online ? "ok" : ""}"></i>${status}</div><p class="b-note">${th(pub.online ? "screen.advanced.nothingToSetUp" : "screen.advanced.cannotReachOfficial")}</p></div>`
   }${relaysBox()}`
    );
  },

  about() {
    const full = app.info.emulatorCommit;
    const commit = /^[0-9a-f]{40}/.test(full) ? full.slice(0, 7) + full.slice(40) : full;
    return (
      head(th("screen.about.title"), th("screen.about.subtitle")) +
      `<div class="b-two" style="margin-top:22px"><div class="b-col">
    <div class="b-rows b-app"><span class="logo"></span><div><b>${esc(APP)}</b><small>${th("screen.about.launcherAndEmulator", { launcher: UPD.cur, commit })}</small></div></div>
    <div class="b-rows"><h6>${th("screen.about.updates")}</h6>${
      !app.info.updates
        ? btnRow(
            th("screen.about.launcherVersion", { version: UPD.cur }),
            th("screen.about.updateUnavailable"),
            btn('data-upd="check" disabled', "screen.about.checkForUpdates"),
          )
        : updateRow()
    }</div>
    <div class="b-rows b-pad"><h6>${th("screen.about.compatibility")}</h6><p class="b-note">${th("screen.about.compatibilityNote", { app: APP })}</p></div>
    <div class="b-rows"><h6>${th("screen.about.help")}</h6>${btnRow(th("screen.about.reportRow"), th("screen.about.reportNote"), btn("data-report", "screen.about.reportBtn"))}${btnRow(th("screen.about.setupRow"), th("screen.about.setupNote"), btn("data-wizard", "screen.about.setupBtn"))}</div></div>
   <div class="b-rows b-pad"><h6>${th("screen.about.openSource")}</h6>${licences()
     .map(([n, l]) => `<div class="b-lic"><b>${esc(n.replace("{commit}", commit))}</b><span>${esc(l)}</span></div>`)
     .join("")}
    <div class="b-acts">${btn("data-src", "screen.about.sourceCodeBtn")}${btn("data-gpl", "screen.about.licenceBtn")}${btn("data-lic", "screen.about.licenceTextsBtn")}</div></div></div>`
    );
  },

  profile() {
    const server = srvName();
    if (!PR.in)
      return (
        head(th("screen.profile.title"), th("screen.profile.subtitleOut", { server })) +
        `<div class="b-narrow"><div class="b-rows b-pad" style="margin-top:22px"><h6>${th("screen.profile.signInHeading")}</h6><div class="b-form">
     <label>${th("screen.profile.onlineId")}<span class="fld w"><input data-p="id" value="${esc(DP.id)}" spellcheck="false" autocomplete="off"></span></label>
     <label>${th("screen.profile.recoveryCode")}<span class="fld w"><input data-code spellcheck="false" autocomplete="off" placeholder="${th("screen.profile.recoveryPlaceholder")}"></span></label>
     <div class="b-acts">${btn("data-login", "screen.profile.signInBtn", "btn fill")}${btn("data-signup", "screen.profile.createAccountBtn")}</div>
     <p class="b-note">${th("screen.profile.recoveryNote")}</p></div></div></div>`
      );
    const avs = AV.map(
      (a, i) =>
        `<button class="${DP.av === i ? "on" : ""}" data-av="${i}" aria-label="${th("screen.profile.avatarOption", { number: i + 1 })}"><span class="av ${a}">${esc(ini(DP.id))}</span></button>`,
    ).join("");
    const img = DP.img
      ? `<span class="b-avimg"><button class="${DP.av === "img" ? "on" : ""}" data-av="img" aria-label="${th("screen.profile.yourImage")}"><span class="av pic" style="background-image:url(${esc(DP.img)})"></span></button><button class="b-avdel" data-avdel aria-label="${th("screen.profile.removeImage")}" data-tip="${th("screen.profile.removeImage")}">${I("x", 11)}</button></span>`
      : `<button data-upload aria-label="${th("screen.profile.uploadImage")}" data-tip="${th("screen.profile.uploadImage")}"><span class="av up">${I("plus", 14)}</span></button>`;
    return (
      head(th("screen.profile.title"), th("screen.profile.subtitleIn", { server })) +
      `<div class="b-narrow"><div class="b-rows b-prof">${avatar(DP, "lg")}<div><b>${esc(DP.id || PR.id)}</b><small><i class="dot ok"></i>${th("screen.profile.signedInStatus", { server })}</small></div></div>
   <div class="b-rows"><h6>${th("screen.profile.account")}</h6>
     <div class="row"><div class="rl"><b>${th("screen.profile.onlineId")}</b><span class="n">${th("screen.profile.onlineIdNote")}</span></div><div class="rc"><label class="fld"><input data-p="id" value="${esc(DP.id)}" aria-label="${th("screen.profile.onlineId")}" spellcheck="false" disabled></label></div></div>
     <div class="row"><div class="rl"><b>${th("screen.profile.avatar")}</b><span class="n">${th("screen.profile.avatarNote")}</span></div><div class="rc"><span class="b-avs">${avs}${img}</span><input type="file" id="av-file" accept="image/*" hidden></div></div></div>
   <div class="b-rows" style="margin-top:12px"><h6>${th("screen.profile.security")}</h6>
     ${SHOW_EMAIL ? btnRow(th("screen.profile.recoveryEmail"), PR.email ? esc(PR.email) : th("screen.profile.recoveryNotSet"), btn("data-email disabled", PR.email ? "screen.profile.changeEmail" : "screen.profile.addEmail")) : ""}
     ${btnRow(th("screen.profile.signOutBtn"), th("screen.profile.signOutNote"), btn("data-logout", "screen.profile.signOutBtn"))}</div></div>`
    );
  },

  friends() {
    if (!PR.in)
      return (
        head(th("screen.friends.title"), th("screen.friends.subtitle")) +
        `<div class="b-two" style="margin-top:22px"><div class="b-rows b-pad"><h6>${th("screen.friends.notSignedIn")}</h6><p class="b-note">${th("screen.friends.notSignedInNote")}</p><div class="b-acts">${btn('data-go="profile"', "screen.friends.goToProfile", "btn fill")}</div></div></div>`
      );
    const fr =
      friendRows()
        .map((f) => {
          const n = esc(f.id);
          return `<div class="b-fr"><span class="av">${esc(ini(f.id))}</span><div><b>${n}</b><small><i class="dot ${f.online ? "ok" : ""}"></i>${esc(f.status)}</small></div><div class="r"><button class="btn sm" data-rmf="${n}" aria-label="${th("screen.friends.removeName", { name: f.id })}" data-tip="${th("screen.friends.removeFriendTip")}">${I("x", 13)}</button></div></div>`;
        })
        .join("") || `<p class="b-note">${th("screen.friends.noFriends")}</p>`;
    const req = app.friends.incoming;
    const rq = req
      .map(
        (r) =>
          `<div class="b-fr"><span class="av">${esc(ini(r))}</span><div><b>${esc(r)}</b><small>${th("screen.friends.wantsFriend")}</small></div><div class="r"><button class="btn sm fill" data-acc="${esc(r)}">${th("screen.friends.accept")}</button><button class="btn sm" data-dec="${esc(r)}">${th("screen.friends.decline")}</button></div></div>`,
      )
      .join("");
    const dismiss = th("screen.friends.dismissInvite");
    const inv = app.invites
      .map(
        (n) =>
          `<div class="b-fr"><span class="av">${esc(ini(n))}</span><div><b>${th("screen.friends.invitedYou", { name: n })}</b></div><div class="r"><button class="btn sm" data-invx="${esc(n)}" aria-label="${dismiss}" data-tip="${dismiss}">${I("x", 13)}</button></div></div>`,
      )
      .join("");
    return (
      head(th("screen.friends.title"), th("screen.friends.subtitle")) +
      `<div class="b-two" style="margin-top:22px"><div class="b-col">
     ${inv ? `<div class="b-rows b-pad"><h6>${th("screen.friends.invitesHeading")}</h6>${inv}</div>` : ""}
     <div class="b-rows b-pad"><h6>${th("screen.friends.addFriend")}</h6><div class="b-add"><span class="fld w"><input data-p="newf" value="${esc(PR.newf)}" placeholder="${th("screen.friends.onlineIdPlaceholder")}" spellcheck="false"></span>${btn("data-addf", "screen.friends.sendRequest", "btn fill")}</div>
       ${rq ? `<h6 class="b-sub">${th("screen.friends.requestsHeading", { count: req.length })}</h6>${rq}` : ""}</div></div>
     <div class="b-rows b-pad"><h6>${th("screen.friends.friendsHeading", { count: app.friends.friends.length })}</h6>${fr}</div></div>`
    );
  },
};
