// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <memory>
#include <unordered_map>
#include <vector>

#include <boost/container/small_vector.hpp>
#include <boost/container/static_vector.hpp>
#include "common/lru_cache.h"
#include "common/slot_vector.h"
#include "common/types.h"
#include "video_core/buffer_cache/buffer.h"
#include "video_core/buffer_cache/fault_manager.h"
#include "video_core/buffer_cache/range_set.h"
#include "video_core/multi_level_page_table.h"

namespace AmdGpu {
struct Liverpool;
}

namespace Core {
class MemoryManager;
}

namespace Vulkan {
class GraphicsPipeline;
}

namespace VideoCore {

using BufferId = Common::SlotId;

static constexpr BufferId NULL_BUFFER_ID{0};

class TextureCache;
class MemoryTracker;
class PageManager;

class BufferCache {
public:
    static constexpr u32 CACHING_PAGEBITS = 14;
    static constexpr u64 CACHING_PAGESIZE = u64{1} << CACHING_PAGEBITS;
    static constexpr u64 DEVICE_PAGESIZE = 16_KB;
    static constexpr u64 CACHING_NUMPAGES = u64{1} << (40 - CACHING_PAGEBITS);
    static constexpr u64 BDA_PAGETABLE_SIZE = CACHING_NUMPAGES * sizeof(vk::DeviceAddress);

    // Default values for garbage collection
    static constexpr s64 DEFAULT_TRIGGER_GC_MEMORY = 1_GB;
    static constexpr s64 DEFAULT_CRITICAL_GC_MEMORY = 2_GB;
    static constexpr s64 TARGET_GC_THRESHOLD = 8_GB;

    struct PageData {
        BufferId buffer_id{};
    };

    struct Traits {
        using Entry = PageData;
        static constexpr size_t AddressSpaceBits = 40;
        static constexpr size_t FirstLevelBits = 16;
        static constexpr size_t PageBits = CACHING_PAGEBITS;
    };
    using PageTable = MultiLevelPageTable<Traits>;

    struct OverlapResult {
        boost::container::small_vector<BufferId, 16> ids;
        VAddr begin;
        VAddr end;
        bool has_stream_leap = false;
    };

public:
    explicit BufferCache(const Vulkan::Instance& instance, Vulkan::Scheduler& scheduler,
                         AmdGpu::Liverpool* liverpool, TextureCache& texture_cache,
                         PageManager& tracker);
    ~BufferCache();

    /// Returns a pointer to GDS device local buffer.
    [[nodiscard]] const Buffer* GetGdsBuffer() const noexcept {
        return &gds_buffer;
    }

    /// Retrieves the device local DBA page table buffer.
    [[nodiscard]] Buffer* GetBdaPageTableBuffer() noexcept {
        return &bda_pagetable_buffer;
    }

    /// Retrieves the fault buffer.
    [[nodiscard]] Buffer* GetFaultBuffer() noexcept {
        return fault_manager.GetFaultBuffer();
    }

    /// Retrieves the buffer with the specified id.
    [[nodiscard]] Buffer& GetBuffer(BufferId id) {
        return slot_buffers[id];
    }

    /// Retrieves a utility buffer optimized for specified memory usage.
    StreamBuffer& GetUtilityBuffer(MemoryUsage usage) noexcept {
        if (usage == MemoryUsage::Stream) {
            return stream_buffer;
        } else if (usage == MemoryUsage::Download) {
            return download_buffer;
        } else if (usage == MemoryUsage::DeviceLocal) {
            return device_buffer;
        } else {
            return staging_buffer;
        }
    }

    /// Invalidates any buffer in the logical page range.
    void InvalidateMemory(VAddr device_addr, u64 size);

    /// Flushes any GPU modified buffer in the logical page range back to CPU memory.
    void ReadMemory(VAddr device_addr, u64 size, bool is_write = false);

    /// Binds host vertex buffers for the current draw.
    void BindVertexBuffers(const Vulkan::GraphicsPipeline& pipeline);

    /// Bind host index buffer for the current draw.
    void BindIndexBuffer(u32 index_offset);

    /// Writes a value to GPU buffer. (uses command buffer to temporarily store the data)
    void FillBuffer(VAddr address, u32 num_bytes, u32 value, bool is_gds);

    /// Performs buffer to buffer data copy on the GPU.
    void CopyBuffer(VAddr dst, VAddr src, u32 num_bytes, bool dst_gds, bool src_gds);

    /// Obtains a buffer for the specified region.
    [[nodiscard]] std::pair<Buffer*, u32> ObtainBuffer(VAddr gpu_addr, u32 size, bool is_written,
                                                       bool is_texel_buffer = false,
                                                       BufferId buffer_id = {});

    /// Whether ObtainBuffer, asked for that range to read from, will answer with the stream
    /// buffer: it has a copy of the range there (see StreamCopy) that it either takes again or
    /// makes anew, for as long as nothing is submitted and no buffer is bound to be written.
    /// The buffer of the cache that holds the range is not needed then.
    [[nodiscard]] bool HasStreamCopy(VAddr gpu_addr, u64 size) const;

    /// Whether ObtainBuffer, asked for that range to read from, answers with the stream
    /// buffer as long as GpuMarks stays what it is: the range is small enough and the GPU
    /// has not modified it.
    [[nodiscard]] bool ReadsStreamCopy(VAddr gpu_addr, u64 size);

    /// ObtainBuffer for a range to read from that ReadsStreamCopy was true for when GpuMarks
    /// was `gpu_marks`.
    [[nodiscard]] std::pair<Buffer*, u32> ObtainStreamCopy(VAddr gpu_addr, u32 size,
                                                           u64 gpu_marks);

    /// See MemoryTracker::GpuMarks.
    [[nodiscard]] u64 GpuMarks() const;

    /// Whether a range may be among those still to be read by MakeDeferredCopy.
    [[nodiscard]] bool OverlapsDeferredRead(VAddr gpu_addr, u64 size) const;
    /// Everything handed to the scheduler so far has been recorded.
    void DeferredReadsDone();
    /// The title's memory is written to for a command, or the title is told how far the GPU
    /// has got and may write to it from here on: the copies made of it by MakeDeferredCopy are
    /// not taken up again.
    void ForgetDeferredCopies() {
        ++deferred_epoch;
    }

    /// Attempts to obtain a buffer without modifying the cache contents.
    [[nodiscard]] std::pair<Buffer*, u32> ObtainBufferForImage(VAddr gpu_addr, u32 size);

    /// Strict-stream multi-slot host-visible upload (mode C/D). Frees only after Wait.
    [[nodiscard]] std::pair<Buffer*, u32> AcquireImageStagingSlot(u32 size);
    void RefreshImageStagingCompletions();

    /// FHD detile source lifetime (buffer_cache 3-way rotate).
    /// Note after detile records a read; Ensure before Synchronize/rewrite of same buffer.
    void NoteDetileSourceUse(vk::Buffer handle, u32 offset, u32 size, const char* path);
    void EnsureDetileSourceWritable(Buffer& buffer, u32 size);

    /// Return true when a region is registered on the cache
    [[nodiscard]] bool IsRegionRegistered(VAddr addr, size_t size);

    /// Return true when a CPU region is modified from the CPU
    [[nodiscard]] bool IsRegionCpuModified(VAddr addr, size_t size);

    /// Return true when a CPU region is modified from the GPU
    [[nodiscard]] bool IsRegionGpuModified(VAddr addr, size_t size);

    /// Return buffer id for the specified region
    BufferId FindBuffer(VAddr device_addr, u32 size);

    /// Processes the fault buffer.
    void ProcessFaultBuffer();

    /// Synchronizes all buffers in the specified range.
    void SynchronizeBuffersInRange(VAddr device_addr, u64 size);

    /// Synchronizes all buffers neede for DMA.
    void SynchronizeDmaBuffers();

    /// Runs the garbage collector.
    void RunGarbageCollector();

private:
    template <typename Func>
    void ForEachBufferInRange(VAddr device_addr, u64 size, Func&& func) {
        buffer_ranges.ForEachInRange(device_addr, size,
                                     [&](u64 page_start, u64 page_end, BufferId id) {
                                         Buffer& buffer = slot_buffers[id];
                                         func(id, buffer);
                                     });
    }

    inline bool IsBufferInvalid(BufferId buffer_id) const {
        return !buffer_id || slot_buffers[buffer_id].is_deleted;
    }

    template <bool async>
    void DownloadBufferMemory(Buffer& buffer, VAddr device_addr, u64 size, bool is_write);

    [[nodiscard]] OverlapResult ResolveOverlaps(VAddr device_addr, u32 wanted_size);

    void JoinOverlap(BufferId new_buffer_id, BufferId overlap_id, bool accumulate_stream_score);

    BufferId CreateBuffer(VAddr device_addr, u32 wanted_size);

    void Register(BufferId buffer_id);

    void Unregister(BufferId buffer_id);

    template <bool insert>
    void ChangeRegister(BufferId buffer_id);

    bool SynchronizeBuffer(Buffer& buffer, VAddr device_addr, u32 size, bool is_written,
                           bool is_texel_buffer);

    /// The copy of a range that is in the stream buffer and still holds what the range does,
    /// if there is one (see StreamCopy).
    std::optional<u64> TakeStreamCopy(VAddr device_addr, u32 size);

    /// Copies a range that the GPU has not modified into the stream buffer, and remembers the
    /// copy for TakeStreamCopy.
    u64 MakeStreamCopy(VAddr device_addr, u32 size);

    /// Gives a range that the GPU has not modified a place in the stream buffer and leaves
    /// copying it there to the thread that records the commands, which gets to it before the
    /// commands that read the copy are submitted. The bytes of the title's memory are the
    /// largest part of what a draw makes the GPU thread touch, and most were last written
    /// by another processor. The copy is of the range as it is a moment later than now: no
    /// different to a title, which may not write to what it has handed to the GPU before
    /// the GPU is through with it (see Rasterizer::SyncDeferredReads for when it is told so).
    u64 MakeDeferredCopy(VAddr device_addr, u32 size);

    vk::Buffer UploadCopies(Buffer& buffer, std::span<vk::BufferCopy> copies,
                            size_t total_size_bytes);

    bool SynchronizeBufferFromImage(Buffer& buffer, VAddr device_addr, u32 size);

    void WriteDataBuffer(Buffer& buffer, VAddr address, const void* value, u32 num_bytes);

    void TouchBuffer(const Buffer& buffer);

    void DeleteBuffer(BufferId buffer_id);

    const Vulkan::Instance& instance;
    Vulkan::Scheduler& scheduler;
    AmdGpu::Liverpool* liverpool;
    Core::MemoryManager* memory;
    TextureCache& texture_cache;
    FaultManager fault_manager;
    std::unique_ptr<MemoryTracker> memory_tracker;
    StreamBuffer staging_buffer;
    StreamBuffer stream_buffer;
    StreamBuffer download_buffer;
    StreamBuffer device_buffer;
    Buffer gds_buffer;
    /// Shaders only go through the page table when the direct memory access setting is on.
    bool uses_bda_pagetable;
    Buffer bda_pagetable_buffer;
    Common::SlotVector<Buffer> slot_buffers;

    /// Mode C/D: independent host-visible buffers for full-res image uploads.
    /// Never wrap a single StreamBuffer range until host completion is proven.
    static constexpr u32 kImageStagingInitialSlots = 8;
    static constexpr u32 kImageStagingMaxSlots = 16;
    struct ImageStagingSlot {
        std::unique_ptr<Buffer> buffer;
        u32 capacity{};
        u64 generation{};
        u64 tick{};
        bool busy{false};
    };
    std::vector<ImageStagingSlot> image_staging_slots;
    u64 next_image_staging_generation{1};
    u32 next_image_staging_rr{0};
    u64 image_staging_stats_acquired{};
    u64 image_staging_stats_waits{};

    /// Per VkBuffer handle: last detile-read generation (Mode A blind spot).
    struct DetileSourceLease {
        u64 generation{};
        u64 tick{};
        u32 size{};
        bool busy{false};
    };
    std::unordered_map<u64, DetileSourceLease> detile_source_leases;
    u64 next_detile_source_generation{1};
    u64 detile_source_stats_notes{};
    u64 detile_source_stats_waits{};
    u64 detile_source_stats_unsafe{};

    /// A small read-only range that ObtainBuffer copied into the stream buffer while the
    /// command buffer being recorded was. A frame binds the same constants, and the vertices
    /// and indices of the same small meshes, draw after draw; each time they were copied again,
    /// eight kilobytes a draw. While the range still holds the same bytes, the copy that is
    /// there is as good as a new one. The bytes are compared with the ones the copy was made
    /// from, kept here: nothing tells when a title writes to its constants, and the stream
    /// buffer itself is memory that is slow to read.
    struct StreamCopy {
        VAddr address{};
        u32 size{};
        u32 kept{};
        u64 offset{};
        u64 tick{};
        u64 cycle{};
        u64 gpu_marks{};
        u64 epoch{};
    };
    static constexpr size_t NumStreamCopies = 4096;
    static constexpr size_t StreamCopiesKeptSize = 16_MB;
    std::unique_ptr<std::array<StreamCopy, NumStreamCopies>> stream_copies;
    std::unique_ptr<u8[]> stream_copies_kept;
    size_t stream_copies_kept_used{};
    /// Whether small ranges are read by the recording thread (MakeDeferredCopy).
    bool deferred_copies{};
    /// The ranges handed over for that since DeferredReadsDone: the pages they touch, hashed,
    /// and the ranges themselves while they are few.
    static constexpr size_t MaxDeferredReads = 128;
    std::array<u64, 64> deferred_read_pages{};
    boost::container::static_vector<std::pair<VAddr, u32>, MaxDeferredReads> deferred_reads;
    bool deferred_reads_overflow{};
    bool deferred_reads_pending{};
    /// The vertex input set last (BindVertexBuffers), and DynamicState::invalidations then.
    boost::container::static_vector<vk::VertexInputBindingDescription2EXT, 32>
        vertex_input_bindings;
    boost::container::static_vector<vk::VertexInputAttributeDescription2EXT, 32>
        vertex_input_attributes;
    u64 vertex_input_invalidations{~0ULL};
    /// What MakeDeferredCopy takes up again is of this epoch.
    u64 deferred_epoch{1};
    u64 stream_copies_tick{};

    u64 total_used_memory = 0;
    u64 trigger_gc_memory = 0;
    u64 critical_gc_memory = 0;
    u64 gc_tick = 0;
    Common::LeastRecentlyUsedCache<BufferId, u64> lru_cache;
    RangeSet gpu_modified_ranges;
    SplitRangeMap<BufferId> buffer_ranges;
    PageTable page_table;
};

} // namespace VideoCore
