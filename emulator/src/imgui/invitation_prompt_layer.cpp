// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

#include <imgui.h>

#include "common/logging/log.h"
#include "common/types.h"
#include "core/libraries/np/np_handler/np_handler.h"
#include "imgui/imgui_layer.h"
#include "imgui/invitation_prompt_layer.h"
#include "imgui/invite_banner.h"
#include "imgui/invite_xr_input.h"
#include "imgui/renderer/imgui_core.h"
#include "imgui/shadnet_notifications_layer.h"

namespace ImGui::InvitationPrompt {

namespace {

using ImGui::InviteBanner::Answer;

enum class State {
    Waiting,  // shown, awaiting user choice
    InFlight, // Accept/Decline running on a worker thread; buttons disabled
};

struct Prompt {
    s32 user_id;
    std::string invitation_id;
    std::string session_id;
    std::string from_npid;
    State state = State::Waiting;
    bool fake = false;
    bool announced = false;
};

constexpr std::size_t kMaxPrompts = 4;    // oldest dropped beyond this
constexpr float kFocusHoldSeconds = 0.6f; // hold Share/Back this long to focus the prompt

std::mutex g_mutex;
std::deque<Prompt> g_prompts;

bool g_focused = false;
bool g_capture_held = false;
float g_back_held_secs = 0.0f;
std::atomic<bool> g_registered{false};

// SHADPS4_DBG_INVITE_AUTO=<seconds>: for unattended tests, that long after a prompt appears its
// Accept is pressed (same variable as the invitation dialog's auto Send/Accept); with
// "<seconds>,decline" Decline is, here.
float DbgInviteAutoDelay() {
    static const float delay = [] {
        const char* value = std::getenv("SHADPS4_DBG_INVITE_AUTO");
        return value != nullptr ? std::strtof(value, nullptr) : -1.0f;
    }();
    return delay;
}
bool DbgInviteAutoHas(const char* word) {
    const char* value = std::getenv("SHADPS4_DBG_INVITE_AUTO");
    return value != nullptr && std::strstr(value, word) != nullptr;
}
bool DbgInviteAutoDeclines() {
    static const bool declines = DbgInviteAutoHas("decline");
    return declines;
}
// "<seconds>,focus": the banner is focused instead of answered, as holding Share would.
bool DbgInviteAutoFocuses() {
    static const bool focuses = DbgInviteAutoHas("focus");
    return focuses;
}
double g_shown_at = -1.0;
double g_auto_focused_at = -1.0;
// A focus that is never answered must not hold the game's buttons for good.
constexpr double kMaxFocusSeconds = 20.0;
double g_focus_since = -1.0;
std::string g_shown_key;

// SHADPS4_DBG_INVITE_FAKE=<seconds>[,<name>]: an invitation from nobody, that many seconds after
// the start and again every 20 s while none is on show; answering it only shows the outcome.
struct Fake {
    float first = -1.0f;
    std::string name = "Player1";
};
const Fake& DbgInviteFake() {
    static const Fake fake = [] {
        Fake f;
        if (const char* value = std::getenv("SHADPS4_DBG_INVITE_FAKE"); value != nullptr) {
            f.first = std::strtof(value, nullptr);
            if (const char* comma = std::strchr(value, ','); comma != nullptr && comma[1] != '\0') {
                f.name = comma + 1;
            }
        }
        return f;
    }();
    return fake;
}
constexpr const char* kFakeId = "fake-invitation";
constexpr float kFakeEvery = 20.0f;
const auto g_start = std::chrono::steady_clock::now();
float g_next_fake = -1.0f;

class InvitationPromptUI final : public ImGui::Layer {
public:
    void Draw() override;
    bool ShouldKeepDrawing() override {
        std::lock_guard lock(g_mutex);
        return !g_prompts.empty() || ImGui::InviteBanner::IsActive();
    }
};

InvitationPromptUI g_layer;

void SetState(const std::string& invitation_id, State state) {
    std::lock_guard lock(g_mutex);
    for (auto& p : g_prompts) {
        if (p.invitation_id == invitation_id) {
            p.state = state;
        }
    }
}

void Remove(const std::string& invitation_id) {
    std::lock_guard lock(g_mutex);
    g_prompts.erase(
        std::remove_if(g_prompts.begin(), g_prompts.end(),
                       [&](const Prompt& p) { return p.invitation_id == invitation_id; }),
        g_prompts.end());
}

void PushPrompt(Prompt prompt) {
    std::lock_guard lock(g_mutex);
    // A re-sent invite replaces the older prompt rather than stacking a duplicate.
    g_prompts.erase(
        std::remove_if(g_prompts.begin(), g_prompts.end(),
                       [&](const Prompt& p) { return p.invitation_id == prompt.invitation_id; }),
        g_prompts.end());
    g_prompts.push_back(std::move(prompt));
    while (g_prompts.size() > kMaxPrompts) {
        g_prompts.pop_front();
    }
}

void MaybeRaiseFake() {
    const Fake& fake = DbgInviteFake();
    if (fake.first < 0.0f) {
        return;
    }
    const float now =
        std::chrono::duration<float>(std::chrono::steady_clock::now() - g_start).count();
    if (g_next_fake < 0.0f) {
        g_next_fake = fake.first;
    }
    if (now < g_next_fake) {
        return;
    }
    g_next_fake = now + kFakeEvery;
    if (ImGui::InviteBanner::IsActive()) {
        return;
    }
    LOG_INFO(ImGui, "SHADPS4_DBG_INVITE_FAKE: a fake invitation from {} at {:.1f} s", fake.name,
             now);
    Prompt prompt{0, kFakeId, "", fake.name};
    prompt.fake = true;
    PushPrompt(std::move(prompt));
}

// Accept/Decline hit the shadNet server (blocking HTTP inside NpHandler), so they run on a
// detached worker thread rather than the render thread. The prompt is marked InFlight until the
// worker finishes, then removed,the outcome is surfaced as a shadNet toast.
void RunAccept(Prompt prompt) {
    SetState(prompt.invitation_id, State::InFlight);
    std::thread([prompt = std::move(prompt)] {
        const bool ok = Libraries::Np::NpHandler::GetInstance().AcceptSessionInvitation(
            prompt.user_id, prompt.invitation_id);
        Remove(prompt.invitation_id);
        if (ok) {
            ImGui::ShadNetNotify::Push(ImGui::ShadNetNotify::Kind::Info,
                                       "Accepted " + prompt.from_npid +
                                           "'s invitation, joining session");
        } else {
            ImGui::ShadNetNotify::Push(ImGui::ShadNetNotify::Kind::Info,
                                       "Could not accept " + prompt.from_npid +
                                           "'s invitation (expired or already used)");
        }
    }).detach();
}

void RunDecline(Prompt prompt) {
    SetState(prompt.invitation_id, State::InFlight);
    std::thread([prompt = std::move(prompt)] {
        Libraries::Np::NpHandler::GetInstance().DeclineSessionInvitation(prompt.user_id,
                                                                         prompt.invitation_id);
        Remove(prompt.invitation_id);
    }).detach();
}

void SetFocused(bool focused) {
    if (g_focused == focused) {
        return;
    }
    g_focused = focused;
    if (focused && !g_capture_held) {
        ImGui::Core::AcquireGamepadInputCapture();
        g_capture_held = true;
    } else if (!focused && g_capture_held) {
        ImGui::Core::ReleaseGamepadInputCapture();
        g_capture_held = false;
    }
}

void Respond(const Prompt& prompt, Answer answer) {
    LOG_INFO(ImGui, "Invitation from {}: {}{}", prompt.from_npid,
             answer == Answer::Accept ? "accepted" : "declined", prompt.fake ? " (fake)" : "");
    ImGui::InviteBanner::ShowResult(answer);
    SetFocused(false);
    ImGui::InviteXrInput::Unfocus();
    if (prompt.fake) {
        Remove(prompt.invitation_id);
    } else if (answer == Answer::Accept) {
        RunAccept(prompt);
    } else {
        RunDecline(prompt);
    }
}

} // namespace

void Push(s32 user_id, std::string invitation_id, std::string session_id, std::string from_npid) {
    PushPrompt({user_id, std::move(invitation_id), std::move(session_id), std::move(from_npid)});
}

void Dismiss(const std::string& invitation_id) {
    Remove(invitation_id);
    if (ImGui::InviteBanner::OfferedKey() == invitation_id) {
        ImGui::InviteBanner::Leave();
    }
}

void Register() {
    if (!g_registered.exchange(true)) {
        ImGui::Layer::AddLayer(&g_layer);
    }
}

void RegisterForFakeInvites() {
    if (DbgInviteFake().first >= 0.0f) {
        Register();
    }
}

void Unregister() {
    SetFocused(false);
    ImGui::InviteXrInput::Publish(false, false);
    ImGui::InviteXrInput::Unfocus();
    if (g_registered.exchange(false)) {
        ImGui::Layer::RemoveLayer(&g_layer);
    }
}

void InvitationPromptUI::Draw() {
    MaybeRaiseFake();

    // New invitations are announced one at a time, oldest first, each once the banner is free.
    const std::string on_show = ImGui::InviteBanner::OfferedKey();
    std::vector<Prompt> snapshot;
    std::optional<Prompt> fresh;
    {
        std::lock_guard lock(g_mutex);
        for (auto& p : g_prompts) {
            if (!p.announced) {
                if (on_show.empty() || on_show == p.invitation_id) {
                    p.announced = true;
                    fresh = p;
                }
                break;
            }
        }
        snapshot.assign(g_prompts.begin(), g_prompts.end());
    }
    if (fresh) {
        ImGui::InviteBanner::Show(fresh->invitation_id, fresh->from_npid, true);
    }
    const auto newest_waiting = [&]() -> const Prompt* {
        for (auto it = snapshot.rbegin(); it != snapshot.rend(); ++it) {
            if (it->state == State::Waiting) {
                return &*it;
            }
        }
        return nullptr;
    };

    namespace XrInput = ImGui::InviteXrInput;
    Answer xr_answer = Answer::None;
    for (XrInput::Event event; (event = XrInput::Take()) != XrInput::Event::None;) {
        if (event == XrInput::Event::Recall) {
            if (const Prompt* p = newest_waiting(); p != nullptr) {
                ImGui::InviteBanner::Show(p->invitation_id, p->from_npid, false);
            }
        } else {
            xr_answer = event == XrInput::Event::Accept ? Answer::Accept : Answer::Decline;
        }
    }

    const auto waiting = [&](const std::string& id) -> const Prompt* {
        for (const auto& p : snapshot) {
            if (p.invitation_id == id && p.state == State::Waiting) {
                return &p;
            }
        }
        return nullptr;
    };
    const std::string offered = ImGui::InviteBanner::OfferedKey();
    const Prompt* current = offered.empty() ? nullptr : waiting(offered);
    if (current == nullptr) {
        // Timed out or answered: the game has its buttons back, the invitation (if unanswered)
        // stays in the list for holding Share (or Menu twice on the headset) to bring back.
        SetFocused(false);
        XrInput::Unfocus();
    }
    XrInput::Publish(current != nullptr, current == nullptr && newest_waiting() != nullptr);
    if (offered != g_shown_key) {
        g_shown_key = offered;
        g_shown_at = ImGui::GetTime();
    }

    Answer answer = Answer::None;
    if (!g_focused) {
        if (ImGui::IsKeyDown(ImGuiKey_GamepadBack)) {
            g_back_held_secs += ImGui::GetIO().DeltaTime;
            if (g_back_held_secs >= kFocusHoldSeconds) {
                g_back_held_secs = 0.0f;
                if (current == nullptr) {
                    current = newest_waiting();
                    if (current != nullptr) {
                        ImGui::InviteBanner::Show(current->invitation_id, current->from_npid,
                                                  false);
                    }
                }
                if (current != nullptr) {
                    SetFocused(true);
                }
            }
        } else {
            g_back_held_secs = 0.0f;
        }
    } else if (ImGui::IsKeyPressed(ImGuiKey_GamepadFaceDown, false)) {
        answer = Answer::Accept;
    } else if (ImGui::IsKeyPressed(ImGuiKey_GamepadFaceRight, false)) {
        answer = Answer::Decline;
    } else if (ImGui::IsKeyPressed(ImGuiKey_Escape, false)) {
        SetFocused(false);
    }
    if (xr_answer != Answer::None) {
        answer = xr_answer;
    }
    if (!g_focused && !XrInput::Focused()) {
        g_focus_since = -1.0;
    } else if (g_focus_since < 0.0) {
        g_focus_since = ImGui::GetTime();
    } else if (ImGui::GetTime() - g_focus_since >= kMaxFocusSeconds) {
        LOG_INFO(ImGui, "Invitation: focused for {:.0f} s without an answer, let go",
                 kMaxFocusSeconds);
        SetFocused(false);
        XrInput::Unfocus();
        g_focus_since = -1.0;
    }
    ImGui::InviteBanner::SetFocused(g_focused || XrInput::Focused());
    ImGui::InviteBanner::SetXrHints(XrInput::Active());

    if (const Answer clicked = ImGui::InviteBanner::DrawDesktop(); clicked != Answer::None) {
        answer = clicked;
    }
    if (current != nullptr && answer == Answer::None && DbgInviteAutoDelay() >= 0.0f &&
        ImGui::GetTime() - g_shown_at >= DbgInviteAutoDelay()) {
        if (!DbgInviteAutoFocuses()) {
            answer = DbgInviteAutoDeclines() ? Answer::Decline : Answer::Accept;
        } else if (g_auto_focused_at != g_shown_at) {
            g_auto_focused_at = g_shown_at;
            SetFocused(true);
        }
    }
    if (current != nullptr && answer != Answer::None) {
        Respond(*current, answer);
    }
}

} // namespace ImGui::InvitationPrompt
