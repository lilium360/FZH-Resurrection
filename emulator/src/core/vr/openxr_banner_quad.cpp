// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <string_view>

#include "common/logging/log.h"
#include "core/vr/openxr_banner_quad.h"
#include "imgui/invite_banner.h"

#define XR_USE_GRAPHICS_API_VULKAN
#include <openxr/openxr_platform.h>

namespace Core::Vr {

namespace {

using ImGui::InviteBanner::QuadHeight;
using ImGui::InviteBanner::QuadWidth;

constexpr vk::DeviceSize StagingSize = vk::DeviceSize{QuadWidth} * QuadHeight * 4;

float Env(const char* name, float fallback, float lo, float hi) {
    const char* value = std::getenv(name);
    if (value == nullptr || value[0] == '\0') {
        return fallback;
    }
    const float parsed = std::strtof(value, nullptr);
    return std::isfinite(parsed) ? std::clamp(parsed, lo, hi) : fallback;
}

struct Placement {
    bool enabled;
    float width_deg;
    float up_deg;
    float distance;
};

const Placement& Place() {
    static const Placement placement = [] {
        const char* off = std::getenv("SHADPS4_XR_BANNER");
        Placement p{
            .enabled = off == nullptr || std::string_view{off} != "0",
            .width_deg = Env("SHADPS4_XR_BANNER_WIDTH_DEG", 33.0f, 5.0f, 120.0f),
            .up_deg = Env("SHADPS4_XR_BANNER_UP_DEG", 10.0f, -60.0f, 60.0f),
            .distance = Env("SHADPS4_XR_BANNER_DISTANCE", 2.0f, 0.3f, 20.0f),
        };
        LOG_INFO(Core_Vr,
                 "Invitation banner in the headset: {}, {:.0f} degrees wide, {:.0f} degrees up, "
                 "{:.2f} m away",
                 p.enabled ? "on" : "off (SHADPS4_XR_BANNER=0)", p.width_deg, p.up_deg,
                 p.distance);
        return p;
    }();
    return placement;
}

} // namespace

const XrCompositionLayerBaseHeader* BannerQuad::Layer(XrSession session, XrSpace view_space,
                                                      const OpenXrHost::Graphics& graphics,
                                                      std::mutex& queue_mutex) {
    if (failed || view_space == XR_NULL_HANDLE) {
        return nullptr;
    }
    if (swapchain == XR_NULL_HANDLE) {
        if (!ImGui::InviteBanner::IsActive() || !Place().enabled) {
            return nullptr;
        }
        if (!Create(session, graphics, queue_mutex)) {
            Destroy(graphics, queue_mutex);
            failed = true;
            LOG_ERROR(Core_Vr, "The invitation banner cannot be shown in the headset: it is "
                               "left out for this run");
            return nullptr;
        }
    }
    if (submitted) {
        // The staging buffer is still being read: show the last picture, never wait here.
        if (graphics.device.getFenceStatus(fence) != vk::Result::eSuccess) {
            return uploaded && showing
                       ? reinterpret_cast<const XrCompositionLayerBaseHeader*>(&layer)
                       : nullptr;
        }
        submitted = false;
    }

    const auto started = std::chrono::steady_clock::now();
    u64 next_key = key;
    float lift = 0.0f;
    if (!ImGui::InviteBanner::ComposeQuad(mapped, bgra, fresh, next_key, lift)) {
        if (showing) {
            showing = false;
            LOG_INFO(Core_Vr,
                     "Invitation banner: gone from the headset, {} pictures uploaded, the "
                     "slowest frame spent {:.2f} ms on it",
                     uploads, worst_ms);
        }
        return nullptr;
    }
    if (!showing) {
        showing = true;
        uploads = 0;
        worst_ms = 0.0f;
    }
    if (next_key != key || fresh) {
        if (!Upload(graphics, queue_mutex)) {
            fresh = true;
            return nullptr;
        }
        key = next_key;
        fresh = false;
        uploaded = true;
        ++uploads;
    }
    if (!uploaded) {
        return nullptr;
    }
    worst_ms = std::max(worst_ms, std::chrono::duration<float, std::milli>(
                                      std::chrono::steady_clock::now() - started)
                                      .count());

    const Placement& p = Place();
    const float up = p.up_deg * 0.017453293f;
    const float width = 2.0f * p.distance * std::tan(p.width_deg * 0.017453293f / 2.0f);
    const float height = width * QuadHeight / QuadWidth;
    const float raise = lift * height;
    layer.layerFlags = XR_COMPOSITION_LAYER_BLEND_TEXTURE_SOURCE_ALPHA_BIT;
    layer.space = view_space;
    layer.eyeVisibility = XR_EYE_VISIBILITY_BOTH;
    layer.subImage.swapchain = swapchain;
    layer.subImage.imageRect = {{0, 0},
                                {static_cast<int32_t>(QuadWidth), static_cast<int32_t>(QuadHeight)}};
    layer.subImage.imageArrayIndex = 0;
    // Tilted up by `up` about the view's x axis, so that it faces the eyes; slid along its own
    // up direction.
    layer.pose.orientation = {std::sin(up / 2.0f), 0.0f, 0.0f, std::cos(up / 2.0f)};
    layer.pose.position = {0.0f, p.distance * std::sin(up) + raise * std::cos(up),
                           -p.distance * std::cos(up) + raise * std::sin(up)};
    layer.size = {width, height};
    return reinterpret_cast<const XrCompositionLayerBaseHeader*>(&layer);
}

bool BannerQuad::Create(XrSession session, const OpenXrHost::Graphics& graphics,
                        std::mutex& queue_mutex) {
    uint32_t count = 0;
    xrEnumerateSwapchainFormats(session, 0, &count, nullptr);
    std::vector<int64_t> formats(count);
    xrEnumerateSwapchainFormats(session, count, &count, formats.data());
    int64_t format = 0;
    for (const vk::Format wanted : {vk::Format::eR8G8B8A8Srgb, vk::Format::eB8G8R8A8Srgb}) {
        if (std::ranges::find(formats, static_cast<int64_t>(wanted)) != formats.end()) {
            format = static_cast<int64_t>(wanted);
            bgra = wanted == vk::Format::eB8G8R8A8Srgb;
            break;
        }
    }
    if (format == 0) {
        LOG_ERROR(Core_Vr, "Invitation banner: no sRGB picture format offered");
        return false;
    }

    XrSwapchainCreateInfo info{XR_TYPE_SWAPCHAIN_CREATE_INFO};
    info.usageFlags = XR_SWAPCHAIN_USAGE_COLOR_ATTACHMENT_BIT | XR_SWAPCHAIN_USAGE_TRANSFER_DST_BIT;
    info.format = format;
    info.sampleCount = 1;
    info.width = QuadWidth;
    info.height = QuadHeight;
    info.faceCount = 1;
    info.arraySize = 1;
    info.mipCount = 1;
    XrResult result;
    std::vector<XrSwapchainImageVulkanKHR> listed;
    {
        std::scoped_lock lock{queue_mutex};
        result = xrCreateSwapchain(session, &info, &swapchain);
        if (XR_SUCCEEDED(result)) {
            uint32_t image_count = 0;
            xrEnumerateSwapchainImages(swapchain, 0, &image_count, nullptr);
            listed.assign(image_count, {XR_TYPE_SWAPCHAIN_IMAGE_VULKAN_KHR});
            result = xrEnumerateSwapchainImages(
                swapchain, image_count, &image_count,
                reinterpret_cast<XrSwapchainImageBaseHeader*>(listed.data()));
        }
    }
    if (XR_FAILED(result) || listed.empty()) {
        LOG_ERROR(Core_Vr, "Invitation banner: no swapchain of {}x{} (XrResult {})", QuadWidth,
                  QuadHeight, static_cast<int>(result));
        return false;
    }
    images.clear();
    for (const auto& image : listed) {
        images.push_back(image.image);
    }

    const vk::Device device = graphics.device;
    const auto [buffer_result, buffer] = device.createBuffer(vk::BufferCreateInfo{
        .size = StagingSize,
        .usage = vk::BufferUsageFlagBits::eTransferSrc,
        .sharingMode = vk::SharingMode::eExclusive,
    });
    if (buffer_result != vk::Result::eSuccess) {
        return false;
    }
    staging = buffer;
    const vk::MemoryRequirements needs = device.getBufferMemoryRequirements(staging);
    const vk::PhysicalDeviceMemoryProperties props = graphics.physical_device.getMemoryProperties();
    constexpr auto Wanted =
        vk::MemoryPropertyFlagBits::eHostVisible | vk::MemoryPropertyFlagBits::eHostCoherent;
    u32 type = ~0u;
    for (u32 i = 0; i < props.memoryTypeCount; ++i) {
        if ((needs.memoryTypeBits & (1u << i)) != 0 &&
            (props.memoryTypes[i].propertyFlags & Wanted) == Wanted) {
            type = i;
            break;
        }
    }
    if (type == ~0u) {
        return false;
    }
    const auto [memory_result, allocated] = device.allocateMemory(vk::MemoryAllocateInfo{
        .allocationSize = needs.size,
        .memoryTypeIndex = type,
    });
    if (memory_result != vk::Result::eSuccess) {
        return false;
    }
    memory = allocated;
    if (device.bindBufferMemory(staging, memory, 0) != vk::Result::eSuccess) {
        return false;
    }
    const auto [map_result, pointer] = device.mapMemory(memory, 0, StagingSize);
    if (map_result != vk::Result::eSuccess) {
        return false;
    }
    mapped = static_cast<u8*>(pointer);

    const auto [pool_result, created_pool] = device.createCommandPool(vk::CommandPoolCreateInfo{
        .flags = vk::CommandPoolCreateFlagBits::eResetCommandBuffer,
        .queueFamilyIndex = graphics.queue_family,
    });
    if (pool_result != vk::Result::eSuccess) {
        return false;
    }
    pool = created_pool;
    const vk::CommandBufferAllocateInfo allocate_info{
        .commandPool = pool,
        .level = vk::CommandBufferLevel::ePrimary,
        .commandBufferCount = 1,
    };
    if (device.allocateCommandBuffers(&allocate_info, &cmdbuf) != vk::Result::eSuccess) {
        return false;
    }
    const auto [fence_result, created_fence] = device.createFence(vk::FenceCreateInfo{});
    if (fence_result != vk::Result::eSuccess) {
        return false;
    }
    fence = created_fence;
    fresh = true;
    uploaded = false;
    key = ~u64{0};
    LOG_INFO(Core_Vr, "Invitation banner: a quad layer of {}x{} ({}), {} images", QuadWidth,
             QuadHeight, bgra ? "BGRA" : "RGBA", images.size());
    return true;
}

bool BannerQuad::Upload(const OpenXrHost::Graphics& graphics, std::mutex& queue_mutex) {
    uint32_t index = 0;
    XrResult result;
    {
        std::scoped_lock lock{queue_mutex};
        XrSwapchainImageAcquireInfo acquire{XR_TYPE_SWAPCHAIN_IMAGE_ACQUIRE_INFO};
        result = xrAcquireSwapchainImage(swapchain, &acquire, &index);
    }
    if (XR_FAILED(result) || index >= images.size()) {
        return false;
    }
    XrSwapchainImageWaitInfo wait{XR_TYPE_SWAPCHAIN_IMAGE_WAIT_INFO};
    wait.timeout = XR_INFINITE_DURATION;
    const bool waited = XR_SUCCEEDED(xrWaitSwapchainImage(swapchain, &wait));

    const vk::Image target{images[index]};
    constexpr vk::ImageSubresourceRange Color{
        .aspectMask = vk::ImageAspectFlagBits::eColor,
        .levelCount = 1,
        .layerCount = 1,
    };
    (void)cmdbuf.reset();
    (void)cmdbuf.begin(vk::CommandBufferBeginInfo{
        .flags = vk::CommandBufferUsageFlagBits::eOneTimeSubmit,
    });
    const vk::ImageMemoryBarrier2 to_copy{
        .srcStageMask = vk::PipelineStageFlagBits2::eAllCommands,
        .srcAccessMask = vk::AccessFlagBits2::eNone,
        .dstStageMask = vk::PipelineStageFlagBits2::eTransfer,
        .dstAccessMask = vk::AccessFlagBits2::eTransferWrite,
        .oldLayout = vk::ImageLayout::eUndefined,
        .newLayout = vk::ImageLayout::eTransferDstOptimal,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .image = target,
        .subresourceRange = Color,
    };
    cmdbuf.pipelineBarrier2(vk::DependencyInfo{
        .imageMemoryBarrierCount = 1,
        .pImageMemoryBarriers = &to_copy,
    });
    cmdbuf.copyBufferToImage(staging, target, vk::ImageLayout::eTransferDstOptimal,
                             vk::BufferImageCopy{
                                 .imageSubresource{
                                     .aspectMask = vk::ImageAspectFlagBits::eColor,
                                     .layerCount = 1,
                                 },
                                 .imageExtent = {QuadWidth, QuadHeight, 1},
                             });
    // The runtime wants its images back as colour attachments.
    const vk::ImageMemoryBarrier2 to_show{
        .srcStageMask = vk::PipelineStageFlagBits2::eTransfer,
        .srcAccessMask = vk::AccessFlagBits2::eTransferWrite,
        .dstStageMask = vk::PipelineStageFlagBits2::eAllCommands,
        .dstAccessMask = vk::AccessFlagBits2::eColorAttachmentRead |
                         vk::AccessFlagBits2::eShaderRead,
        .oldLayout = vk::ImageLayout::eTransferDstOptimal,
        .newLayout = vk::ImageLayout::eColorAttachmentOptimal,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .image = target,
        .subresourceRange = Color,
    };
    cmdbuf.pipelineBarrier2(vk::DependencyInfo{
        .imageMemoryBarrierCount = 1,
        .pImageMemoryBarriers = &to_show,
    });
    (void)cmdbuf.end();

    bool ok = false;
    {
        std::scoped_lock lock{queue_mutex};
        if (waited) {
            (void)graphics.device.resetFences(fence);
            const vk::SubmitInfo submit{
                .commandBufferCount = 1,
                .pCommandBuffers = &cmdbuf,
            };
            ok = graphics.queue.submit(submit, fence) == vk::Result::eSuccess;
            submitted = ok;
        }
        XrSwapchainImageReleaseInfo release{XR_TYPE_SWAPCHAIN_IMAGE_RELEASE_INFO};
        result = xrReleaseSwapchainImage(swapchain, &release);
    }
    return ok && XR_SUCCEEDED(result);
}

void BannerQuad::Destroy(const OpenXrHost::Graphics& graphics, std::mutex& queue_mutex) {
    const vk::Device device = graphics.device;
    if (submitted) {
        (void)device.waitForFences(fence, true, 1'000'000'000);
        submitted = false;
    }
    if (swapchain != XR_NULL_HANDLE) {
        std::scoped_lock lock{queue_mutex};
        xrDestroySwapchain(swapchain);
        swapchain = XR_NULL_HANDLE;
    }
    images.clear();
    if (fence) {
        device.destroyFence(fence);
        fence = nullptr;
    }
    if (pool) {
        device.destroyCommandPool(pool);
        pool = nullptr;
        cmdbuf = nullptr;
    }
    if (staging) {
        device.destroyBuffer(staging);
        staging = nullptr;
    }
    if (memory) {
        device.freeMemory(memory);
        memory = nullptr;
        mapped = nullptr;
    }
    uploaded = false;
    fresh = true;
    key = ~u64{0};
}

} // namespace Core::Vr
