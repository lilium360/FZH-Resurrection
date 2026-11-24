import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { BackendError, type Backend, type BackendErrorCode, type BackendEvents } from "./backend";
import type { Params } from "./i18n";

function toBackendError(e: unknown): BackendError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    const o = e as { code: unknown; key?: unknown; params?: unknown; message: unknown };
    const params = o.params && typeof o.params === "object" ? (o.params as Params) : {};
    return new BackendError(String(o.code) as BackendErrorCode, typeof o.key === "string" ? o.key : "", String(o.message), params);
  }
  const detail = typeof e === "string" ? e : String(e);
  return new BackendError("internal", "backend.internal", detail, { detail });
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (e) {
    throw toBackendError(e);
  }
}

export function createTauriBackend(): Backend {
  return {
    kind: "tauri",

    getAppInfo: () => call("get_app_info"),
    getLauncherState: () => call("get_launcher_state"),
    setSetupCompleted: (done) => call("set_setup_completed", { done }),
    checkForUpdate: () => call("check_for_update"),
    downloadUpdate: () => call("download_update"),
    installUpdate: () => call("install_update"),
    openLink: (target) => call("open_link", { target }),
    saveProblemReport: (report, title) => call("save_problem_report", { report, title }),

    loadSettings: () => call("load_settings"),
    saveSettings: (settings) => call("save_settings", { settings }),
    listAudioDevices: () => call("list_audio_devices"),

    getGameFiles: () => call("get_game_files"),
    pickFolder: (kind, title) => call("pick_folder", { kind, title }),
    selectGameFolder: (path, expect) => call("select_game_folder", { path, expect }),
    resetGameFiles: () => call("reset_game_files"),
    prepareGame: () => call("prepare_game"),
    getInstallPlan: () => call("get_install_plan"),
    setInstallLocation: (path) => call("set_install_location", { path }),
    installGame: () => call("install_game"),
    cancelInstall: () => call("cancel_install"),
    deleteInstall: (target) => call("delete_install", { target }),

    getGameState: () => call("get_game_state"),
    launchGame: () => call("launch_game"),
    stopGame: () => call("stop_game"),

    getControllerCheck: () => call("get_controller_check"),

    getShaderCache: () => call("get_shader_cache"),
    startShaderWarmup: () => call("start_shader_warmup"),
    cancelShaderWarmup: () => call("cancel_shader_warmup"),
    clearShaderCache: () => call("clear_shader_cache"),

    getAccount: () => call("get_account"),
    signIn: (onlineId, recoveryCode) => call("sign_in", { onlineId, recoveryCode }),
    register: (onlineId) => call("register_account", { onlineId }),
    signOut: () => call("sign_out"),
    updateProfile: (profile) => call("update_profile", { profile }),
    uploadAvatar: (image) => call("upload_avatar", { image }),
    setRecoveryEmail: (email) => call("set_recovery_email", { email }),

    getFriends: () => call("get_friends"),
    sendFriendRequest: (onlineId) => call("send_friend_request", { onlineId }),
    acceptFriendRequest: (onlineId) => call("accept_friend_request", { onlineId }),
    declineFriendRequest: (onlineId) => call("decline_friend_request", { onlineId }),
    removeFriend: (onlineId) => call("remove_friend", { onlineId }),

    getServerStatus: () => call("get_server_status"),
    getRelays: () => call("get_relays"),
    getHosting: () => call("get_hosting"),
    startHosting: () => call("start_hosting"),
    stopHosting: () => call("stop_hosting"),

    on<K extends keyof BackendEvents>(event: K, handler: (payload: BackendEvents[K]) => void) {
      return listen<BackendEvents[K]>(event, (e) => handler(e.payload));
    },
  };
}
