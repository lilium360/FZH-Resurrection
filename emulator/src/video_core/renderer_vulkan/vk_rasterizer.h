// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <memory>
#include <vector>

#include <boost/container/small_vector.hpp>

#include "common/recursive_lock.h"
#include "common/shared_first_mutex.h"
#include "video_core/buffer_cache/buffer_cache.h"
#include "video_core/page_manager.h"
#include "video_core/renderer_vulkan/vk_occlusion_queries.h"
#include "video_core/renderer_vulkan/vk_pipeline_cache.h"
#include "video_core/renderer_vulkan/vk_scheduler.h"
#include "video_core/texture_cache/texture_cache.h"

namespace AmdGpu {
struct Liverpool;
}

namespace Core {
class MemoryManager;
}

namespace Vulkan {

class Scheduler;
class RenderState;
class GraphicsPipeline;

class Rasterizer {
public:
    explicit Rasterizer(const Instance& instance, Scheduler& scheduler,
                        AmdGpu::Liverpool* liverpool);
    ~Rasterizer();

    [[nodiscard]] Scheduler& GetScheduler() noexcept {
        return scheduler;
    }

    /// Handles DMA writes that land on a depth target's HTILE surface. Returns true when the
    /// write was consumed as a depth clear.
    bool TryHtileClear(VAddr address, std::span<const u32> htile_words);

    /// Logs the next `count` guest draws, dispatches and markers. Debugging aid.
    static void StartDrawTrace(s32 count);

    [[nodiscard]] VideoCore::BufferCache& GetBufferCache() noexcept {
        return buffer_cache;
    }

    [[nodiscard]] VideoCore::TextureCache& GetTextureCache() noexcept {
        return texture_cache;
    }

    void Draw(bool is_indexed, u32 index_offset = 0);
    /// A dump of the z-pass counters by the title; false if it is to be answered as before.
    bool OcclusionDump(u64* address, u32 num_pairs) {
        return occlusion_queries.Dump(address, num_pairs);
    }

    /// The memory of the title that draws read is read by the thread that records their
    /// commands, a moment after the draw is translated (BufferCache::MakeDeferredCopy). That
    /// is through with all of it after SyncDeferredReads: called before the title is told
    /// that the GPU has got to some point of its commands, and before anything is written to
    /// its memory on behalf of a command (BeforeGuestWrite, if only that range matters).
    void SyncDeferredReads();
    void BeforeGuestWrite(VAddr address, u64 size);
    /// A write to the title's memory for a command that tells it nothing else: made in its
    /// place among the commands handed over, after the reads of the draws before it.
    void WriteGuestInOrder(void* address, u64 data, u32 num_bytes);
    /// Lets the recording thread have what was handed over, without waiting for it.
    void KickDeferred();
    /// What four bytes read at `address` hold once the writes of WriteGuestInOrder are made:
    /// 0 if none of those touches them, 1 if `value`, 2 if only waiting for them tells.
    int PendingGuestWrite(VAddr address, u32& value);
    void DrawIndirect(bool is_indexed, VAddr arg_address, u32 offset, u32 size, u32 max_count,
                      VAddr count_address);

    void DispatchDirect();
    void DispatchIndirect(VAddr address, u32 offset, u32 size);

    void ScopeMarkerBegin(const std::string_view& str, bool from_guest = false);
    void ScopeMarkerEnd(bool from_guest = false);
    void ScopedMarkerInsert(const std::string_view& str, bool from_guest = false);
    void ScopedMarkerInsertColor(const std::string_view& str, const u32 color,
                                 bool from_guest = false);

    void FillBuffer(VAddr address, u32 num_bytes, u32 value, bool is_gds);
    void CopyBuffer(VAddr dst, VAddr src, u32 num_bytes, bool dst_gds, bool src_gds);
    u32 ReadDataFromGds(u32 gsd_offset);
    bool InvalidateMemory(VAddr addr, u64 size);
    bool ReadMemory(VAddr addr, u64 size);
    bool IsMapped(VAddr addr, u64 size);
    void MapMemory(VAddr addr, u64 size);
    void UnmapMemory(VAddr addr, u64 size);

    void CpSync();
    u64 Flush();
    void Finish();
    void OnSubmit();
    void CompleteGuestReadbacks();

    PipelineCache& GetPipelineCache() {
        return pipeline_cache;
    }

    template <typename Func>
    void ForEachMappedRangeInRange(VAddr addr, u64 size, Func&& func) {
        const auto range = decltype(mapped_ranges)::interval_type::right_open(addr, addr + size);
        Common::RecursiveSharedLock lock{mapped_ranges_mutex};
        for (const auto& mapped_range : (mapped_ranges & range)) {
            func(mapped_range);
        }
    }

private:
    void PrepareRenderState(const GraphicsPipeline* pipeline);
    RenderState BeginRendering(const GraphicsPipeline* pipeline);

    /// Everything of the title's state that the targets of a draw are worked out from.
    struct TargetSource {
        std::array<AmdGpu::ColorBuffer, AmdGpu::NUM_COLOR_BUFFERS> color_buffers;
        std::array<AmdGpu::CbDbExtent, AmdGpu::NUM_COLOR_BUFFERS> color_extents;
        std::array<u8, AmdGpu::NUM_COLOR_BUFFERS> color_samples;
        AmdGpu::DepthBuffer depth_buffer;
        AmdGpu::DepthView depth_view;
        AmdGpu::DepthControl depth_control;
        AmdGpu::DepthRenderControl depth_render_control;
        AmdGpu::CbDbExtent depth_extent;
        VAddr htile_address;
        float depth_clear;
        u32 stencil_clear;
        u32 color_target_mask;
        u32 color_control_mode;
        u32 mrt_mask;
    };
    /// The targets of the last draw, for the next one if it renders to the same: a pass of the
    /// title is hundreds of draws to one set of targets, and finding the images, their views
    /// and their state again for each was a tenth of what a draw cost the GPU thread. They
    /// stand while the title's target state is the same (TargetSource), the render pass that
    /// draw was made in is still open, no image was added or removed, no metadata surface was
    /// marked cleared, and none of the images has an upload pending or is bound as a texture.
    struct LastTargets {
        bool valid{};
        TargetSource source;
        RenderState state;
        u64 pass_serial{};
        u64 registrations{};
        u64 meta_clears{};
        boost::container::static_vector<VideoCore::ImageId, AmdGpu::NUM_COLOR_BUFFERS + 1> images;
    };
    void ReadTargetSource(const GraphicsPipeline* pipeline, TargetSource& source) const;
    /// Whether the draw has the targets of the last one; marks them as its targets if so.
    bool TakeLastTargets(const TargetSource& source);
    /// Whether they still stand once the draw has bound its resources.
    bool LastTargetsStand();
    void Resolve();
    void DepthStencilCopy(bool is_depth, bool is_stencil);
    void EliminateFastClear();

    void UpdateDynamicState(const GraphicsPipeline* pipeline, bool is_indexed) const;
    void UpdateViewportScissorState() const;
    void UpdateDepthStencilState() const;
    void UpdatePrimitiveState(bool is_indexed) const;
    void UpdateRasterizationState() const;
    void UpdateColorBlendingState(const GraphicsPipeline* pipeline) const;

    bool FilterDraw();

    /// The passes a draw is made in: for each, the colour targets (a bit each) it leaves out.
    /// One pass that leaves nothing out, unless two of the draw's targets are one surface.
    boost::container::small_vector<u8, 4> SharedTargetPasses(u8 mrt_mask) const;

    void BindBuffers(const Shader::Info& stage, Shader::Backend::Bindings& binding,
                     Shader::PushData& push_data);
    void BindTextures(const Shader::Info& stage, Shader::Backend::Bindings& binding);
    bool BindResources(const Pipeline* pipeline);
    std::unique_ptr<VideoCore::Buffer> IsolateReadConstGuestBuffer(VAddr addr, u64 size);
    void RetireIsolatedReadConstSnapshots();

    /// Set while a draw is made in more than one pass (SharedTargetPasses).
    bool in_target_passes{};

    void ResetBindings() {
        for (auto& image_id : bound_images) {
            texture_cache.GetImage(image_id).binding = {};
        }
        bound_images.clear();
    }

    bool IsComputeMetaClear(const Pipeline* pipeline);
    bool IsComputeImageCopy(const Pipeline* pipeline);
    bool IsComputeImageClear(const Pipeline* pipeline);

private:
    friend class VideoCore::BufferCache;

    const Instance& instance;
    Scheduler& scheduler;
    VideoCore::PageManager page_manager;
    VideoCore::BufferCache buffer_cache;
    VideoCore::TextureCache texture_cache;
    AmdGpu::Liverpool* liverpool;
    Core::MemoryManager* memory;
    boost::icl::interval_set<VAddr> mapped_ranges;
    Common::SharedFirstMutex mapped_ranges_mutex;
    PipelineCache pipeline_cache;
    OcclusionQueries occlusion_queries;

    using RenderTargetInfo = std::pair<VideoCore::ImageId, VideoCore::TextureCache::ImageDesc>;
    std::array<RenderTargetInfo, AmdGpu::NUM_COLOR_BUFFERS> cb_descs;
    std::pair<VideoCore::ImageId, VideoCore::TextureCache::ImageDesc> db_desc;
    boost::container::small_vector<vk::DescriptorImageInfo,
                                   Shader::NUM_IMAGES + Shader::NUM_SAMPLERS>
        image_infos;
    boost::container::static_vector<vk::DescriptorBufferInfo, Shader::NUM_BUFFERS> buffer_infos;
    boost::container::small_vector<VideoCore::ImageId, Shader::NUM_IMAGES> bound_images;

    u32 set_write_index{};
    Pipeline::DescriptorWrites set_writes;
    Pipeline::BufferBarriers buffer_barriers;
    Shader::PushData push_data;

    using BufferBindingInfo = std::tuple<VideoCore::BufferId, AmdGpu::Buffer, u64>;
    boost::container::static_vector<BufferBindingInfo, Shader::NUM_BUFFERS> buffer_bindings;
    std::vector<std::unique_ptr<VideoCore::Buffer>> isolated_readconst_buffers;
    u32 isolated_readconst_hits{};
    using ImageBindingInfo = std::pair<VideoCore::ImageId, VideoCore::TextureCache::ImageDesc>;
    boost::container::small_vector<ImageBindingInfo, Shader::NUM_IMAGES> image_bindings;
    /// See BindTextures.
    std::unique_ptr<std::array<VideoCore::TextureCache::BoundTexture, 16384>> bound_textures;
    bool fault_process_pending{};
    bool attachment_feedback_loop{};
    LastTargets last_targets;
    /// Set by BeginRendering when a target was cleared because its metadata said so.
    bool targets_meta_cleared{};
};

} // namespace Vulkan
