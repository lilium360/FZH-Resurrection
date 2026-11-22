// SPDX-FileCopyrightText: Copyright 2025 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <source_location>
#include <condition_variable>
#include <mutex>
#include <thread>
#include <unordered_set>
#include <boost/container/small_vector.hpp>
#include <queue>
#include <tsl/robin_map.h>

#include "common/lru_cache.h"
#include "common/slot_vector.h"
#include "shader_recompiler/resource.h"
#include "video_core/multi_level_page_table.h"
#include "video_core/texture_cache/blit_helper.h"
#include "video_core/texture_cache/image.h"
#include "video_core/texture_cache/image_view.h"
#include "video_core/texture_cache/sampler.h"
#include "video_core/texture_cache/tile_manager.h"

namespace AmdGpu {
struct Liverpool;
}

namespace VideoCore {

class BufferCache;
class PageManager;

class TextureCache {
    // Default values for garbage collection
    static constexpr s64 DEFAULT_PRESSURE_GC_MEMORY = 1_GB + 512_MB;
    static constexpr s64 DEFAULT_CRITICAL_GC_MEMORY = 3_GB;
    static constexpr s64 TARGET_GC_THRESHOLD = 8_GB;

    using ImageIds = boost::container::small_vector<ImageId, 16>;

    struct Traits {
        using Entry = ImageIds;
        static constexpr size_t AddressSpaceBits = 40;
        static constexpr size_t FirstLevelBits = 10;
        static constexpr size_t PageBits = 20;
    };
    using PageTable = MultiLevelPageTable<Traits>;

public:
    enum class BindingType : u32 {
        Texture,
        Storage,
        RenderTarget,
        DepthTarget,
        VideoOut,
    };

    struct ImageDesc {
        ImageInfo info;
        ImageViewInfo view_info;
        BindingType type{BindingType::Texture};

        ImageDesc() = default;
        ImageDesc(const AmdGpu::Image& image, const Shader::ImageResource& desc)
            : info{image, desc}, view_info{image, desc},
              type{desc.is_written ? BindingType::Storage : BindingType::Texture} {}
        ImageDesc(const AmdGpu::ColorBuffer& buffer, AmdGpu::CbDbExtent hint)
            : info{buffer, hint}, view_info{buffer}, type{BindingType::RenderTarget} {}
        ImageDesc(const AmdGpu::DepthBuffer& buffer, AmdGpu::DepthView view,
                  AmdGpu::DepthControl ctl, VAddr htile_address, AmdGpu::CbDbExtent hint,
                  bool write_buffer = false)
            : info{buffer, view.NumSlices(), htile_address, hint, write_buffer},
              view_info{buffer, view, ctl}, type{BindingType::DepthTarget} {}
        ImageDesc(const Libraries::VideoOut::BufferAttributeGroup& group, VAddr cpu_address)
            : info{group, cpu_address}, type{BindingType::VideoOut} {}
    };

public:
    TextureCache(const Vulkan::Instance& instance, Vulkan::Scheduler& scheduler,
                 AmdGpu::Liverpool* liverpool, BufferCache& buffer_cache, PageManager& tracker);
    ~TextureCache();

    TileManager& GetTileManager() noexcept {
        return tile_manager;
    }

    /// Invalidates any image in the logical page range.
    void InvalidateMemory(VAddr addr, size_t size);

    /// Marks an image as dirty if it exists at the provided address.
    void InvalidateMemoryFromGPU(VAddr address, size_t max_size);

    /// Evicts any images that overlap the unmapped range.
    void UnmapMemory(VAddr cpu_addr, size_t size);

    /// Schedules a copy of pending images for download back to CPU memory.
    void ProcessDownloadImages();
    void CompleteBrightnessReadbacks();

    /// Retrieves the image handle of the image with the provided attributes.
    struct BoundTexture;
    [[nodiscard]] ImageId FindImage(ImageDesc& desc, bool exact_fmt = false,
                                    BoundTexture* remember = nullptr);

    /// FindImage for a texture that was bound before: the image it answered then, if it would
    /// answer the same now, or none (and then FindImage is to be asked).
    [[nodiscard]] ImageId FindImageAgain(BoundTexture& texture);

    /// Retrieves image whose address matches provided
    [[nodiscard]] ImageId FindImageFromRange(VAddr address, size_t size, bool ensure_valid = true);

    /// Retrieves an image view with the properties of the specified image id.
    [[nodiscard]] ImageView& FindTexture(ImageId image_id, const ImageDesc& desc);

    /// Retrieves the render target with specified properties
    [[nodiscard]] ImageView& FindRenderTarget(ImageId image_id, const ImageDesc& desc);

    /// Retrieves the depth target with specified properties
    [[nodiscard]] ImageView& FindDepthTarget(ImageId image_id, const ImageDesc& desc);

    /// Updates image contents if it was modified by CPU.
    void UpdateImage(ImageId image_id) {
        std::scoped_lock lock{mutex};
        Image& image = slot_images[image_id];
        TrackImage(image_id);
        TouchImage(image);
        RefreshImage(image);
    }

    /// Resolves overlap between existing cache image and pending merged image
    [[nodiscard]] std::tuple<ImageId, int, int> ResolveOverlap(const ImageInfo& info,
                                                               BindingType binding,
                                                               ImageId cache_img_id,
                                                               ImageId merged_image_id);

    /// Resolves depth overlap and either re-creates the image or returns existing one
    [[nodiscard]] ImageId ResolveDepthOverlap(const ImageInfo& requested_info, BindingType binding,
                                              ImageId cache_img_id);

    /// Recreates a backing whose dimension cannot support the requested view.
    [[nodiscard]] ImageId ResolveDimensionOverlap(const ImageInfo& requested_info,
                                                  ImageId cache_img_id);

    /// Creates a new image with provided image info and copies subresources from image_id
    [[nodiscard]] ImageId ExpandImage(const ImageInfo& info, ImageId image_id);

    /// Reuploads image contents.
    void RefreshImage(Image& image);

    /// Retrieves the sampler that matches the provided S# descriptor.
    [[nodiscard]] vk::Sampler GetSampler(const AmdGpu::Sampler& sampler,
                                         AmdGpu::BorderColorBuffer border_color_base);

    /// Retrieves the image with the specified id.
    [[nodiscard]] Image& GetImage(ImageId id) {
        auto& image = slot_images[id];
        TouchImage(image);
        return image;
    }

    /// Retrieves the image view with the specified id.
    [[nodiscard]] ImageView& GetImageView(ImageId id) {
        return slot_image_views[id];
    }

    /// Visits every image in the cache. Debugging aid.
    template <typename Func>
    void ForEachImage(Func&& func) {
        std::scoped_lock lock{mutex};
        for (Image& image : slot_images) {
            func(image);
        }
    }

    /// Get the associated depth stencil image if it is still valid.
    ImageId GetAssociatedDepth(Image& image) {
        if (!image.depth_id) {
            return {};
        }
        if (slot_images.is_allocated(image.depth_id)) {
            auto& depth_image = slot_images[image.depth_id];
            if (depth_image.image_uid == image.depth_uid &&
                depth_image.flags & ImageFlagBits::Registered) {
                return image.depth_id;
            }
        }
        // The linked depth image is no longer valid, disassociate it.
        image.DisassociateDepth();
        return {};
    }

    /// Returns true if the specified address is a metadata surface.
    bool IsMeta(VAddr address) const {
        return surface_metas.contains(address);
    }

    /// Returns true if the specified address is the HTILE surface of a depth target.
    bool IsHtile(VAddr address) const {
        const auto& it = surface_metas.find(address);
        return it != surface_metas.end() && it.value().type == MetaDataInfo::Type::HTile;
    }

    /// Returns true if a slice of the specified metadata surface has been cleared.
    bool IsMetaCleared(VAddr address, u32 slice) const {
        const auto& it = surface_metas.find(address);
        if (it != surface_metas.end()) {
            return it.value().clear_mask & (1u << slice);
        }
        return false;
    }

    /// Clears all slices of the specified metadata surface.
    bool ClearMeta(VAddr address) {
        auto it = surface_metas.find(address);
        if (it != surface_metas.end()) {
            it.value().clear_mask = u32(-1);
            ++meta_clears;
            return true;
        }
        return false;
    }

    /// Updates the state of a slice of the specified metadata surface.
    bool TouchMeta(VAddr address, u32 slice, bool is_clear) {
        auto it = surface_metas.find(address);
        if (it != surface_metas.end()) {
            if (is_clear) {
                it.value().clear_mask |= 1u << slice;
                ++meta_clears;
            } else {
                it.value().clear_mask &= ~(1u << slice);
            }
            return true;
        }
        return false;
    }

    /// How many images were added or removed so far, and how many times a metadata surface
    /// was marked as cleared: what a draw found for its targets holds while neither moves.
    u64 Registrations() const {
        return registrations;
    }
    u64 MetaClears() const {
        return meta_clears;
    }

    /// Whether UpdateImage would do nothing to the image but note its use: it has nothing to
    /// upload, and all of its memory is watched for writes.
    bool UpToDate(const Image& image) const {
        return (False(image.flags & ImageFlagBits::Dirty) || image.info.num_samples > 1) &&
               (False(image.flags & ImageFlagBits::Registered) ||
                (image.track_addr == image.info.guest_address &&
                 image.track_addr_end == image.info.guest_address + image.info.guest_size));
    }

    /// Whether binding the image as a target again would do nothing to it: it is up to date
    /// and not bound as anything else by the draw.
    bool StandsAsTarget(ImageId image_id) {
        const Image& image = slot_images[image_id];
        return UpToDate(image) && !image.binding.is_bound && !image.binding.needs_rebind;
    }

    /// Runs the garbage collector.
    void RunGarbageCollector();

    template <typename Func>
    void ForEachImageInRegion(VAddr cpu_addr, size_t size, Func&& func) {
        using FuncReturn = typename std::invoke_result<Func, ImageId, Image&>::type;
        static constexpr bool BOOL_BREAK = std::is_same_v<FuncReturn, bool>;
        ImageIds images;
        ForEachPage(cpu_addr, size, [this, &images, cpu_addr, size, func](u64 page) {
            const auto it = page_table.find(page);
            if (it == nullptr) {
                if constexpr (BOOL_BREAK) {
                    return false;
                } else {
                    return;
                }
            }
            for (const ImageId image_id : *it) {
                Image& image = slot_images[image_id];
                if (image.flags & ImageFlagBits::Picked) {
                    continue;
                }
                if (!image.Overlaps(cpu_addr, size)) {
                    continue;
                }
                image.flags |= ImageFlagBits::Picked;
                images.push_back(image_id);
                if constexpr (BOOL_BREAK) {
                    if (func(image_id, image)) {
                        return true;
                    }
                } else {
                    func(image_id, image);
                }
            }
            if constexpr (BOOL_BREAK) {
                return false;
            }
        });
        for (const ImageId image_id : images) {
            slot_images[image_id].flags &= ~ImageFlagBits::Picked;
        }
    }

private:
    /// Iterate over all page indices in a range
    template <typename Func>
    static void ForEachPage(PAddr addr, size_t size, Func&& func) {
        static constexpr bool RETURNS_BOOL = std::is_same_v<std::invoke_result<Func, u64>, bool>;
        const u64 page_end = (addr + size - 1) >> Traits::PageBits;
        for (u64 page = addr >> Traits::PageBits; page <= page_end; ++page) {
            if constexpr (RETURNS_BOOL) {
                if (func(page)) {
                    break;
                }
            } else {
                func(page);
            }
        }
    }

    /// Gets or creates a null image for a particular format.
    ImageId GetNullImage(vk::Format format);

    /// Copies image memory back to CPU.
    void DownloadImageMemory(ImageId image_id);

    /// Thread function for copying downloaded images out to CPU memory.
    void DownloadedImagesThread(const std::stop_token& token);

    /// Create an image from the given parameters
    [[nodiscard]] ImageId InsertImage(const ImageInfo& info, VAddr cpu_addr);

    /// Register image in the page table
    void RegisterImage(ImageId image);

    /// Unregister image from the page table
    void UnregisterImage(ImageId image);

    /// Track CPU reads and writes for image
    void TrackImage(ImageId image_id);
    void TrackImageHead(ImageId image_id);
    void TrackImageTail(ImageId image_id);

    /// Stop tracking CPU reads and writes for image
    void UntrackImage(ImageId image_id);
    void UntrackImageHead(ImageId image_id);
    void UntrackImageTail(ImageId image_id);

    void MarkAsMaybeDirty(ImageId image_id, Image& image);

    /// Removes the image and any views/surface metas that reference it.
    void DeleteImage(ImageId image_id, bool erase_metas = true);

    /// Touch the image in the LRU cache.
    void TouchImage(const Image& image);

    void FreeImage(ImageId image_id,
                   std::source_location where = std::source_location::current()) {
        NoteFreedImage(image_id, where.line());
        UntrackImage(image_id);
        UnregisterImage(image_id);
        DeleteImage(image_id);
    }

private:
    /// SHADPS4_FRAME_STATS: says which line of the cache frees the large images.
    void NoteFreedImage(ImageId image_id, u32 line);

    /// Puts aside the image that ResolveDepthOverlap has just made over as `new_image_id`,
    /// instead of freeing it: the next time the title wants the memory the other way round,
    /// this is the image it gets.
    void StashDepthTwin(ImageId old_image_id, ImageId new_image_id) {
        StashTwin(depth_twins, old_image_id, new_image_id);
    }

    /// The same for any list of images put aside.
    void StashTwin(std::vector<ImageId>& twins, ImageId old_image_id, ImageId new_image_id);

    /// Images of memory a title uses now as a depth buffer, now as a colour image, put aside
    /// while it is the other one's turn. Neither registered nor tracked.
    std::vector<ImageId> depth_twins;

    /// When the title wants as a volume what the cache has as an array of as many layers, or
    /// the other way round: the image to use, with what the cached one holds copied into it.
    /// The cached one is put aside for the next turn. Null when the two are not such a pair.
    ImageId SwitchVolumeTwin(const ImageInfo& requested_info, ImageId cache_image_id);

    /// Images of memory a title draws into as an array and reads as a volume, put aside while
    /// it is the other one's turn. Neither registered nor tracked.
    std::vector<ImageId> volume_twins;

    const Vulkan::Instance& instance;
    Vulkan::Scheduler& scheduler;
    AmdGpu::Liverpool* liverpool;
    BufferCache& buffer_cache;
    PageManager& tracker;
    BlitHelper blit_helper;
    TileManager tile_manager;
    Common::SlotVector<Image> slot_images;

    /// What FindImage answered the last time it was asked for an image that was there exactly
    /// as described. A draw asks for the same textures as the draw before it more often than
    /// not, and looking through every page an image covers for candidates each time is what
    /// made the question expensive. An answer holds until an image is added or removed in
    /// the memory it is about (see FoundStillHolds).
    struct FoundImage {
        VAddr guest_address{};
        u32 guest_size{};
        Extent3D size{};
        SubresourceExtent resources{};
        vk::Format pixel_format{};
        AmdGpu::ImageType type{};
        bool exact_fmt{};
        ImageId image_id{};
        u64 registrations{};
        /// In the first of a pair: whether it, and not the second, was the one asked for last.
        bool last_asked{};
    };
    std::array<FoundImage, 2048> found_images{};
    /// The image that stands for the stencil part of the depth buffer the last draw had
    /// (FindDepthTarget), and the memory it was looked up in. Every draw with a depth buffer
    /// asks for it, and each time the images of all the pages of that memory were gone
    /// through to find it.
    FoundImage found_stencil{};

    /// Where the last images were added or removed, and how many were in all.
    struct RegistrationRange {
        VAddr address{};
        u64 size{};
    };
    std::array<RegistrationRange, 64> registration_log{};
    u64 registrations{1};
    u64 meta_clears{};

    /// Notes that an image was added or removed in that memory.
    void NoteRegistration(VAddr address, u64 size) {
        registration_log[registrations % registration_log.size()] = {address, size};
        ++registrations;
    }

public:
    /// What binding a texture from a T# came to: the image FindImage answered for it, and the
    /// view FindTexture gave of that image. A frame binds the same few hundred textures draw
    /// after draw; describing, finding and viewing each of them every time was a sixth of what
    /// a draw cost the GPU thread. This is good for as long as FindImage would answer the same
    /// (FindImageAgain tells) and the image the view is of is still what the image is kept in.
    struct BoundTexture {
        enum class State : u8 {
            Empty,
            Found,  ///< FindImage has answered; the view is not known yet.
            Viewed, ///< Complete.
        };
        std::array<u64, 4> sharp{};
        u32 traits{};
        State state{};
        /// What FindImage answered: kept only when it was an image exactly as described.
        FoundImage found;
        /// The image bound in the end: the depth buffer, for the stencil part of one.
        ImageId image_id{};
        SubresourceRange range{};
        const void* backing{};
        vk::ImageView view{};
        /// The layout it was last bound in as a plain texture (`settled`), and how many
        /// times the state of a part of the image had changed by then: see
        /// Rasterizer::BindTextures.
        vk::ImageLayout layout{};
        u64 state_version{};
        bool settled{};
        /// When FindImageAgain last noted the use of the image: the command buffer that was
        /// being recorded, and the round of the search for unused images.
        u64 noted_tick{~0ull};
        u64 noted_gc_tick{~0ull};
    };

private:
    /// Whether an answer of FindImage is still the answer, to the same question; notes the use
    /// of the image if so.
    bool FoundAgain(FoundImage& found);

    /// Whether no image was added or removed in the memory of that answer since it was given
    /// or last checked. An image somewhere else changes nothing for it: without telling the
    /// two apart, a title that makes one image anew every frame had every answer thrown away
    /// every frame.
    bool FoundStillHolds(FoundImage& found) const {
        if (found.registrations == registrations) {
            return true;
        }
        if (registrations - found.registrations > registration_log.size()) {
            return false;
        }
        const VAddr end = found.guest_address + found.guest_size;
        for (u64 i = found.registrations; i != registrations; ++i) {
            const auto& range = registration_log[i % registration_log.size()];
            if (range.address < end && found.guest_address < range.address + range.size) {
                return false;
            }
        }
        found.registrations = registrations;
        return true;
    }
    Common::SlotVector<ImageView> slot_image_views;
    tsl::robin_map<u64, Sampler> samplers;
    tsl::robin_map<vk::Format, ImageId> null_images;
    std::unordered_set<ImageId> download_images;
    u64 total_used_memory = 0;
    u64 trigger_gc_memory = 0;
    u64 pressure_gc_memory = 0;
    u64 critical_gc_memory = 0;
    u64 gc_tick = 0;
    Common::LeastRecentlyUsedCache<ImageId, u64> lru_cache;
    bool readback_linear_images;
    PageTable page_table;
    std::mutex mutex;
    struct MetaDataInfo {
        enum class Type {
            CMask,
            FMask,
            HTile,
        };
        Type type;
        s32 clear_mask = -1;
    };
    tsl::robin_map<VAddr, MetaDataInfo> surface_metas;
};

} // namespace VideoCore
