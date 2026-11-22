// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <chrono>
#include <deque>
#include <mutex>

#include "common/logging/log.h"
#include "imgui/invite_xr_input.h"

namespace ImGui::InviteXrInput {

namespace {

using Clock = std::chrono::steady_clock;

constexpr auto DoublePress = std::chrono::milliseconds{400};
constexpr auto ActiveFor = std::chrono::seconds{1};

std::mutex g_mutex;
bool g_answerable = false;
bool g_recallable = false;
bool g_focused = false;
u32 g_held = 0;
// Held since a press the game was not given, until let go, so that it never sees half of one.
u32 g_withheld = 0;
Clock::time_point g_menu_to_game{};
Clock::time_point g_last_update{};
std::deque<Event> g_events;

void Queue(Event event) {
    if (g_events.size() < 8) {
        g_events.push_back(event);
    }
}

} // namespace

u32 Update(u32 held) {
    std::scoped_lock lock{g_mutex};
    const auto now = Clock::now();
    g_last_update = now;
    const u32 pressed = held & ~g_held;
    g_held = held;
    g_withheld &= held;

    if ((pressed & Menu) != 0) {
        if (g_answerable) {
            g_focused = !g_focused;
            g_withheld |= Menu;
            LOG_INFO(ImGui, "Invitation, headset controllers: Menu {}",
                     g_focused ? "focuses the banner (A accepts, B declines)"
                               : "lets the banner go, unanswered");
        } else if (g_recallable && now - g_menu_to_game < DoublePress) {
            g_focused = true;
            g_withheld |= Menu;
            g_menu_to_game = {};
            Queue(Event::Recall);
            LOG_INFO(ImGui, "Invitation, headset controllers: Menu twice brings back the newest "
                            "unanswered invitation, focused");
        } else {
            g_menu_to_game = now;
        }
    }
    if (g_focused && (pressed & (A | B)) != 0) {
        const bool accept = (pressed & A) != 0;
        g_focused = false;
        g_withheld |= pressed & (A | B);
        Queue(accept ? Event::Accept : Event::Decline);
        LOG_INFO(ImGui, "Invitation, headset controllers: {}", accept ? "A accepts" : "B declines");
    }
    return g_withheld;
}

void Publish(bool answerable, bool recallable) {
    std::scoped_lock lock{g_mutex};
    g_answerable = answerable;
    g_recallable = recallable;
}

Event Take() {
    std::scoped_lock lock{g_mutex};
    if (g_events.empty()) {
        return Event::None;
    }
    const Event event = g_events.front();
    g_events.pop_front();
    return event;
}

bool Focused() {
    std::scoped_lock lock{g_mutex};
    return g_focused;
}

void Unfocus() {
    std::scoped_lock lock{g_mutex};
    if (g_focused) {
        g_focused = false;
        LOG_INFO(ImGui, "Invitation, headset controllers: the banner is let go");
    }
}

bool Active() {
    std::scoped_lock lock{g_mutex};
    return g_last_update != Clock::time_point{} && Clock::now() - g_last_update < ActiveFor;
}

} // namespace ImGui::InviteXrInput
