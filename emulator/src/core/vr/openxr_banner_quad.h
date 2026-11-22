// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <mutex>
#include <vector>

#include <openxr/openxr.h>

#include "core/vr/openxr_host.h"

namespace Core::Vr {

/// The invitation banner (ImGui::InviteBanner) in the headset: a quad layer of its own, fixed to
/// the view, over the title's frames, which are left as they are. Whatever goes wrong here costs
/// the banner, never a frame: it is then left out for the rest of the run.
class BannerQuad {
public:
    /// Between xrBeginFrame and xrEndFrame, on the frame thread: the layer to add to this frame,
    /// or nothing.
    const XrCompositionLayerBaseHeader* Layer(XrSession session, XrSpace view_space,
                                              const OpenXrHost::Graphics& graphics,
                                              std::mutex& queue_mutex);
    /// Before the session goes.
    void Destroy(const OpenXrHost::Graphics& graphics, std::mutex& queue_mutex);

private:
    bool Create(XrSession session, const OpenXrHost::Graphics& graphics, std::mutex& queue_mutex);
    bool Upload(const OpenXrHost::Graphics& graphics, std::mutex& queue_mutex);

    XrSwapchain swapchain{XR_NULL_HANDLE};
    std::vector<VkImage> images;
    bool bgra{};
    vk::Buffer staging;
    vk::DeviceMemory memory;
    u8* mapped{};
    vk::CommandPool pool;
    vk::CommandBuffer cmdbuf;
    vk::Fence fence;
    bool submitted{};
    bool failed{};
    bool fresh{true};
    bool uploaded{};
    bool showing{};
    u32 uploads{};
    float worst_ms{};
    u64 key{~u64{0}};
    XrCompositionLayerQuad layer{XR_TYPE_COMPOSITION_LAYER_QUAD};
};

} // namespace Core::Vr
