// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include "common/types.h"

namespace ImGui::InviteXrInput {

// Answering an invitation with the headset's controllers. The OpenXR host decides on its own
// thread, as each press comes in, whether the game sees it, from what the prompt layer last
// published; the prompt layer then takes the outcome on the present thread.

enum Button : u32 {
    Menu = 1u << 0,
    A = 1u << 1,
    B = 1u << 2,
};

enum class Event { None, Accept, Decline, Recall };

// XrHost thread, on each read of the controllers while they play: which of the buttons are
// held. Returns those the game must not be given.
u32 Update(u32 held);

// Present thread, every frame the prompt layer draws. `answerable`: a banner offers an
// invitation that can still be answered. `recallable`: none is on show, but one is waiting.
void Publish(bool answerable, bool recallable);
Event Take();
bool Focused();
void Unfocus();
// The headset's controllers were read as the game's pad within the last second.
bool Active();

} // namespace ImGui::InviteXrInput
