import { th } from "../i18n";
import { avatar, esc, ini, presenceText } from "./components";
import { I } from "./icons";
import { MENU } from "./meta";
import { SCREENS } from "./screens";
import { CACHE, DP, F, PR, SAVED, app, canPlay, dirty } from "./state";

const saveBar = (): string =>
  dirty()
    ? `<div class="b-save"><div><span>${I("alert", 15)}${th("shell.unsaved")}</span><button class="btn" data-discard>${th("dialog.unsavedChanges.discard")}</button><button class="btn fill" data-save>${th("shell.saveChanges")}</button></div></div>`
    : "";

const screenParam = (id: string) => ({ screen: { key: `nav.menu.${id}` } });

export function shell(): string {
  const pub = app.pub;
  const srv: [string, string] = SAVED.usePrivateServer
    ? app.host
      ? ["host", th("shell.srvHosting")]
      : ["up", th("shell.srvPrivate", { address: SAVED.privateServer })]
    : pub.online
      ? ["up", pub.players === null ? th("shell.srvOfficialOnline") : th("shell.srvOfficialPlayers", { count: pub.players })]
      : ["", th("shell.srvOfficialOffline")];
  const total = app.friends.friends.length;
  const requests = PR.in ? app.friends.incoming.length : 0;
  const badge = requests ? `<span class="chip acc" style="margin-left:auto" aria-label="${th("shell.friendRequests", { count: requests })}">${requests}</span>` : "";
  const on = PR.in ? app.friends.friends.filter((f) => f.online) : [];
  const friends = `<div class="b-fon"><h6>${PR.in ? th("shell.friendsOnlineCount", { count: on.length }) : th("shell.friendsOnline")}</h6>${
    !PR.in
      ? `<p class="b-note">${th("screen.friends.notSignedInNote")}</p>`
      : on.length
        ? on
            .slice(0, 3)
            .map(
              (f) =>
                `<button class="b-fi" data-go="friends"><span class="av sm">${esc(ini(f.onlineId))}</span><span><b>${esc(f.onlineId)}</b><small>${esc(presenceText(f))}</small></span></button>`,
            )
            .join("")
        : `<p class="b-note">${th("shell.noFriendsOnline")}</p>`
  }${PR.in ? `<button class="b-all" data-go="friends">${on.length > 3 ? `${th("shell.more", { count: on.length - 3 })} · ` : ""}${th("shell.allFriends", { count: total })}${requests ? ` · ${th("shell.friendRequests", { count: requests })}` : ""} ${I("arrow", 13)}</button>` : ""}</div>`;
  const tip = app.run
    ? ""
    : CACHE.status === "warming"
      ? th("shell.tipWarmup", screenParam("graphics"))
      : F.prep !== 2
        ? F.prep === 1
          ? th("gameSteps.preparingText")
          : th("shell.tipChooseFolder", screenParam("play"))
        : !PR.in
          ? th("shell.tipSignIn", screenParam("profile"))
          : PR.elsewhere
            ? th("shell.tipAccountInUse")
            : "";
  const side = `<h6 class="b-mh">${th("shell.mainMenu")}</h6><nav class="b-nav">${MENU.map(
    (n) => `${n[2] ? `<h6>${th(`nav.menuGroup.${n[2]}`)}</h6>` : ""}<button class="${app.scr === n[0] ? "on" : ""}" data-go="${n[0]}">${I(n[1], 17)}${th(`nav.menu.${n[0]}`)}${n[0] === "friends" ? badge : ""}</button>`,
  ).join("")}</nav>
    <div class="b-foot">${friends}
      <div class="b-chips">
        <button class="b-chip" data-go="profile" data-tip="${PR.in ? th(app.run ? "shell.inGame" : "presence.online") : th("shell.signInFromProfile")}">${PR.in ? avatar(DP) : `<span class="av">${I("user", 11)}</span>`}<span class="tx">${PR.in ? esc(DP.id || PR.id) : th("screen.profile.signInBtn")}</span></button>
        <button class="b-svc ${srv[0]}" data-go="advanced" data-tip="${srv[1]}" aria-label="${srv[1]}">${I("globe", 14)}</button>
      </div>
      <button class="b-go b-side-go ${app.run ? "run" : PR.in && PR.elsewhere ? "hold" : ""}" data-act="play" ${app.run || canPlay() ? "" : "disabled"} data-tip="${tip}">${I(app.run ? "stop" : "play", 17)}${th(app.run ? "shell.stop" : "shell.play")}</button>
    </div>`;
  return `<div class="b-body"><aside class="b-side">${side}</aside><main class="b-main">${SCREENS[app.scr]()}${saveBar()}</main></div>`;
}
