// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <string>

#include "common/types.h"

namespace ImGui::InviteBanner {

// The small panel that announces an incoming invitation: drawn on the desktop window through
// ImGui, and for the headset into a pixel buffer that the OpenXR host shows as a quad layer of
// its own. It only shows; InvitationPrompt decides what is shown and handles the answers.

enum class Answer { None, Accept, Decline };

// Shows the invitation `key` from `online_id`. `announce`: a new invitation, with the chime (not
// one the player brought back).
void Show(const std::string& key, const std::string& online_id, bool announce);
// Replaces the hints with the outcome of the answer for a moment, then leaves.
void ShowResult(Answer answer);
void Leave();
void SetFocused(bool focused);
// The hints name the headset controllers' buttons rather than the gamepad's.
void SetXrHints(bool xr);
// The invitation whose answer hints are on show; empty while there is none.
std::string OfferedKey();
bool IsActive();

// For the ImGui thread: draws on the desktop window, and reports a click on an answer hint.
Answer DrawDesktop();

// For the headset's frame thread.
constexpr u32 QuadWidth = 960;
constexpr u32 QuadHeight = 147;
// False while nothing is shown. Otherwise `lift` is how far the panel is above its place, in
// panel heights, and when the state differs from `key` the banner is written to `pixels`
// (QuadWidth x QuadHeight, sRGB, alpha premultiplied in linear light, RGBA or BGRA) and `key`
// is updated. `fresh`: `pixels` holds nothing written before.
bool ComposeQuad(u8* pixels, bool bgra, bool fresh, u64& key, float& lift);

} // namespace ImGui::InviteBanner
