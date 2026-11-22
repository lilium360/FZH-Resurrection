// SPDX-FileCopyrightText: Copyright 2019-2026 rpcs3 Project
// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <algorithm>
#include <chrono>
#include <cstring>
#include <map>
#include <httplib.h>
#include <nlohmann/json.hpp>
#include "common/logging/log.h"
#include "core/emulator_settings.h"
#include "core/libraries/invitation_dialog/invitation_dialog.h"
#include "core/libraries/np/np_handler/np_handler.h"
#include "core/libraries/system/systemservice.h"
#include "imgui/invitation_prompt_layer.h"

namespace Libraries::Np {

namespace {

using json = nlohmann::json;

// A title that never sends a WebAPI request still gets its fetched invitations, this late.
constexpr auto kTitleOnlineFallback = std::chrono::seconds(90);

struct WebApiSession {
    httplib::Client client;
    httplib::Headers headers;

    WebApiSession(const std::string& base_url, const std::string& token)
        : client(base_url), headers{{"Authorization", "Bearer " + token}} {
        client.set_connection_timeout(5);
        client.set_read_timeout(10);
    }

    std::optional<json> Get(const std::string& path) {
        const auto res = client.Get(path, headers);
        if (!res || res->status != 200) {
            return std::nullopt;
        }
        json body = json::parse(res->body, nullptr, false);
        if (body.is_discarded() || !body.is_object()) {
            return std::nullopt;
        }
        return body;
    }
};

std::string JsonString(const json& object, const char* key) {
    const auto it = object.find(key);
    return it != object.end() && it->is_string() ? it->get<std::string>() : std::string{};
}

bool JsonBool(const json& object, const char* key) {
    const auto it = object.find(key);
    return it != object.end() && it->is_boolean() && it->get<bool>();
}

} // namespace

// Minimal JSON string escaper for the invitation-request body (npids are safe but the user message
// can contain quotes/backslashes/control chars).
std::string JsonEscape(const std::string& in) {
    std::string out;
    out.reserve(in.size() + 8);
    for (const char c : in) {
        switch (c) {
        case '"':
            out += "\\\"";
            break;
        case '\\':
            out += "\\\\";
            break;
        case '\n':
            out += "\\n";
            break;
        case '\r':
            out += "\\r";
            break;
        case '\t':
            out += "\\t";
            break;
        default:
            if (static_cast<unsigned char>(c) < 0x20) {
                char buf[8];
                std::snprintf(buf, sizeof(buf), "\\u%04x", c);
                out += buf;
            } else {
                out += c;
            }
        }
    }
    return out;
}

bool NpHandler::SendSessionInvitation(s32 user_id, const std::string& session_id,
                                      const std::vector<std::string>& to,
                                      const std::string& message) {
    if (session_id.empty() || to.empty()) {
        LOG_ERROR(NpHandler, "empty session_id or recipient list");
        return false;
    }
    const std::string base_url = EmulatorSettings.GetShadNetWebApiServer();
    if (base_url.empty()) {
        LOG_ERROR(NpHandler, "WebAPI server address is empty");
        return false;
    }
    const std::string token = GetBearerToken(user_id);
    if (token.empty()) {
        LOG_ERROR(NpHandler, "no bearer token for user_id={}", user_id);
        return false;
    }

    // invitation-request JSON: {"to":[...],"message":"..."}
    std::string json = "{\"to\":[";
    for (size_t i = 0; i < to.size(); ++i) {
        if (i != 0) {
            json += ',';
        }
        json += '"';
        json += JsonEscape(to[i]);
        json += '"';
    }
    json += ']';
    if (!message.empty()) {
        json += ",\"message\":\"";
        json += JsonEscape(message);
        json += '"';
    }
    json += '}';

    // multipart/mixed with a single JSON part; shadNet keys on the Content-Description.
    const std::string boundary = "shadps4invite" + std::to_string(user_id);
    std::string body;
    body += "--" + boundary + "\r\n";
    body += "Content-Type: application/json; charset=utf-8\r\n";
    body += "Content-Description: invitation-request\r\n\r\n";
    body += json;
    body += "\r\n--" + boundary + "--\r\n";

    const std::string path = "/v1/sessions/" + session_id + "/invitations";
    const std::string content_type = "multipart/mixed; boundary=" + boundary;

    httplib::Client cli(base_url);
    cli.set_connection_timeout(5);
    cli.set_read_timeout(10);
    const httplib::Headers headers = {{"Authorization", "Bearer " + token}};
    const auto res = cli.Post(path.c_str(), headers, body, content_type.c_str());
    if (!res) {
        LOG_ERROR(NpHandler, "POST {} failed (no response)", path);
        return false;
    }
    if (res->status != 200 && res->status != 204) {
        LOG_ERROR(NpHandler, "POST {} -> HTTP {}", path, res->status);
        return false;
    }
    LOG_INFO(NpHandler, "sent invite to {} recipient(s) for session '{}'", to.size(), session_id);
    return true;
}

void NpHandler::PostSessionInvitationEvent(s32 user_id, const std::string& session_id,
                                           const std::string& invitation_id,
                                           const std::string& accepter_online_id,
                                           const std::string& inviter_online_id,
                                           OrbisNpAccountId inviter_account_id) {
    using Libraries::InvitationDialog::ORBIS_NP_SESSION_INVITATION_EVENT_FLAG_INVITATION;
    using Libraries::InvitationDialog::OrbisNpSessionInvitationEventParam;

    Libraries::SystemService::OrbisSystemServiceEvent event{};
    event.event_type = Libraries::SystemService::OrbisSystemServiceEventType::SessionInvitation;

    auto* param = reinterpret_cast<OrbisNpSessionInvitationEventParam*>(event.param);
    std::memset(param, 0, sizeof(*param));
    std::strncpy(param->sessionId.data, session_id.c_str(), sizeof(param->sessionId.data) - 1);
    if (!invitation_id.empty()) {
        std::strncpy(param->invitationId.data, invitation_id.c_str(),
                     sizeof(param->invitationId.data) - 1);
        param->flag = ORBIS_NP_SESSION_INVITATION_EVENT_FLAG_INVITATION;
    } else {
        param->flag = 0; // join from session info (no invitation id in the push)
    }
    // data[16] is followed by its own 'term' byte, so a full 16-char online id must not be cut.
    std::strncpy(param->onlineId.data, accepter_online_id.c_str(), sizeof(param->onlineId.data));
    param->userId = user_id;
    std::strncpy(param->referralOnlineId.data, inviter_online_id.c_str(),
                 sizeof(param->referralOnlineId.data));
    param->referralAccountId = inviter_account_id;

    Libraries::SystemService::PushSystemServiceEvent(event);
    LOG_INFO(NpHandler,
             "Posted SESSION_INVITATION user_id={} session='{}' invitation='{}' flag={} "
             "onlineId='{}' referral='{}'({})",
             user_id, session_id, invitation_id, param->flag, accepter_online_id, inviter_online_id,
             inviter_account_id);
}

std::vector<NpHandler::PendingInvitation> NpHandler::GetPendingInvitations(s32 user_id) const {
    std::lock_guard lk(m_mutex_pending_invites);
    const auto it = m_pending_invites.find(user_id);
    if (it == m_pending_invites.end()) {
        return {};
    }
    const auto now = std::chrono::duration_cast<std::chrono::milliseconds>(
                         std::chrono::system_clock::now().time_since_epoch())
                         .count();
    std::vector<PendingInvitation> out;
    out.reserve(it->second.size());
    for (const auto& p : it->second) {
        if (p.valid_until == 0 || now <= p.valid_until) {
            out.push_back(p);
        }
    }
    return out;
}

bool NpHandler::AcceptSessionInvitation(s32 user_id, const std::string& invitation_id) {
    // Pull the matching stashed invite (and drop it).
    PendingInvitation inv;
    bool found = false;
    {
        std::lock_guard lk(m_mutex_pending_invites);
        m_answered_invites.insert(invitation_id);
        const auto mit = m_pending_invites.find(user_id);
        if (mit != m_pending_invites.end()) {
            auto& v = mit->second;
            for (auto it = v.begin(); it != v.end(); ++it) {
                if (it->invitation_id == invitation_id) {
                    inv = *it;
                    v.erase(it);
                    found = true;
                    break;
                }
            }
        }
    }
    // Whatever surface handled it (RECV dialog or the emulator prompt), retire the other one.
    ImGui::InvitationPrompt::Dismiss(invitation_id);
    if (!found) {
        LOG_ERROR(NpHandler, "no pending invite '{}' for user_id={}", invitation_id, user_id);
        return false;
    }
    // Raise the join event now that the user has explicitly accepted (via the RECV dialog or the
    // emulator's system-UI equivalent).
    PostSessionInvitationEvent(user_id, inv.session_id, invitation_id, inv.to_npid, inv.from_npid,
                               inv.from_account_id);
    LOG_INFO(NpHandler, "accepted '{}' session='{}'", invitation_id, inv.session_id);
    return true;
}

void NpHandler::DeclineSessionInvitation(s32 user_id, const std::string& invitation_id) {
    ImGui::InvitationPrompt::Dismiss(invitation_id);
    std::string session_id;
    {
        std::lock_guard lock(m_mutex_pending_invites);
        m_answered_invites.insert(invitation_id);
        auto& v = m_pending_invites[user_id];
        const auto it = std::find_if(v.begin(), v.end(), [&](const PendingInvitation& p) {
            return p.invitation_id == invitation_id;
        });
        if (it == v.end()) {
            return;
        }
        session_id = it->session_id;
        v.erase(it);
    }
    LOG_INFO(NpHandler, "dismissed '{}' for user_id={}", invitation_id, user_id);
    std::thread([this, user_id, session_id] { ConsumeSessionInvitations(user_id, session_id); })
        .detach();
}

void NpHandler::ConsumeSessionInvitations(s32 user_id, const std::string& session_id) {
    const std::string base_url = EmulatorSettings.GetShadNetWebApiServer();
    const std::string token = GetBearerToken(user_id);
    if (base_url.empty() || token.empty()) {
        return;
    }
    WebApiSession api(base_url, token);
    const auto list = api.Get("/v1/users/me/invitations?fields=sessionId");
    if (!list || !list->contains("invitations") || !list->at("invitations").is_array()) {
        LOG_WARNING(NpHandler, "declined invitations to '{}' not marked used: no list", session_id);
        return;
    }
    int marked = 0;
    for (const auto& e : list->at("invitations")) {
        if (!e.is_object() || JsonBool(e, "usedFlag") || JsonString(e, "sessionId") != session_id) {
            continue;
        }
        const auto res = api.client.Put("/v1/users/me/invitations/" + JsonString(e, "invitationId"),
                                        api.headers, R"({"usedFlag":true})", "application/json");
        if (res && res->status == 204) {
            ++marked;
        }
    }
    LOG_INFO(NpHandler, "declined: {} invitation(s) to '{}' marked used", marked, session_id);
}

void NpHandler::FetchPendingInvitations(s32 user_id, const std::string& own_npid) {
    const std::string base_url = EmulatorSettings.GetShadNetWebApiServer();
    const std::string token = GetBearerToken(user_id);
    if (base_url.empty() || token.empty()) {
        LOG_WARNING(NpHandler, "pending invitations not fetched for user_id={}: no WebAPI token",
                    user_id);
        return;
    }
    WebApiSession api(base_url, token);
    const auto list = api.Get("/v1/users/me/invitations?fields=sessionId,receivedDate,expired,fromUser");
    if (!list || !list->contains("invitations") || !list->at("invitations").is_array()) {
        LOG_WARNING(NpHandler, "pending invitations not fetched for user_id={}: list failed",
                    user_id);
        return;
    }

    // Several unused invitations to one session (INVITE pressed again) make one prompt.
    const json& entries = list->at("invitations");
    std::map<std::string, std::pair<std::string, PendingInvitation>> newest;
    for (const auto& e : entries) {
        if (!e.is_object() || JsonBool(e, "usedFlag") || JsonBool(e, "expired")) {
            continue;
        }
        PendingInvitation inv;
        inv.invitation_id = JsonString(e, "invitationId");
        inv.session_id = JsonString(e, "sessionId");
        inv.to_npid = own_npid;
        if (const auto from = e.find("fromUser"); from != e.end() && from->is_object()) {
            inv.from_npid = JsonString(*from, "onlineId");
            inv.from_account_id =
                std::strtoull(JsonString(*from, "accountId").c_str(), nullptr, 10);
        }
        if (inv.invitation_id.empty() || inv.session_id.empty()) {
            continue;
        }
        const std::string received = JsonString(e, "receivedDate");
        const auto it = newest.find(inv.session_id);
        if (it == newest.end() || it->second.first <= received) {
            newest[inv.session_id] = {received, std::move(inv)};
        }
    }

    // The list keeps invitations to deleted sessions; the single-invitation view omits
    // `session` for those.
    int shown = 0;
    int gone = 0;
    for (auto& [session_id, entry] : newest) {
        const auto info =
            api.Get("/v1/users/me/invitations/" + entry.second.invitation_id + "?fields=session");
        if (!info || !info->contains("session")) {
            ++gone;
            continue;
        }
        StashFetchedInvitation(user_id, std::move(entry.second));
        ++shown;
    }
    LOG_INFO(NpHandler,
             "pending invitations for user_id={}: {} listed, {} to raise, {} for ended sessions",
             user_id, entries.size(), shown, gone);
    if (shown == 0) {
        return;
    }

    const auto deadline = std::chrono::steady_clock::now() + kTitleOnlineFallback;
    while (m_initialized && std::chrono::steady_clock::now() < deadline) {
        {
            std::lock_guard lk(m_mutex_pending_invites);
            if (m_title_online.contains(user_id)) {
                return;
            }
        }
        std::this_thread::sleep_for(std::chrono::seconds(1));
    }
    if (m_initialized) {
        OnTitleWebApiRequest(user_id);
    }
}

void NpHandler::StashFetchedInvitation(s32 user_id, PendingInvitation inv) {
    bool raise = false;
    {
        std::lock_guard lk(m_mutex_pending_invites);
        auto& v = m_pending_invites[user_id];
        const bool known = m_answered_invites.contains(inv.invitation_id) ||
                           std::any_of(v.begin(), v.end(), [&](const PendingInvitation& p) {
                               return p.invitation_id == inv.invitation_id;
                           });
        if (known) {
            return;
        }
        v.push_back(inv);
        raise = m_title_online.contains(user_id);
        if (!raise) {
            m_held_invites[user_id].push_back(inv);
        }
    }
    if (raise) {
        ImGui::InvitationPrompt::Push(user_id, inv.invitation_id, inv.session_id, inv.from_npid);
    }
}

void NpHandler::OnTitleWebApiRequest(s32 user_id) {
    std::vector<PendingInvitation> raise;
    {
        std::lock_guard lk(m_mutex_pending_invites);
        if (!m_title_online.insert(user_id).second) {
            return;
        }
        const auto held = m_held_invites.find(user_id);
        if (held == m_held_invites.end()) {
            return;
        }
        const auto& pending = m_pending_invites[user_id];
        for (auto& inv : held->second) {
            if (std::any_of(pending.begin(), pending.end(), [&](const PendingInvitation& p) {
                    return p.invitation_id == inv.invitation_id;
                })) {
                raise.push_back(std::move(inv));
            }
        }
        m_held_invites.erase(held);
    }
    for (const auto& inv : raise) {
        LOG_INFO(NpHandler, "raising invitation '{}' from {} fetched at sign-in",
                 inv.invitation_id, inv.from_npid);
        ImGui::InvitationPrompt::Push(user_id, inv.invitation_id, inv.session_id, inv.from_npid);
    }
}

} // namespace Libraries::Np
