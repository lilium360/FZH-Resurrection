// SPDX-FileCopyrightText: Copyright 2025 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <memory>
#include <mutex>
#include <thread>
#include <queue>
#include <source_location>

#include <boost/container/static_vector.hpp>
#include "common/unique_function.h"
#include "video_core/amdgpu/regs_color.h"
#include "video_core/amdgpu/regs_primitive.h"
#include "video_core/renderer_vulkan/vk_master_semaphore.h"
#include "video_core/renderer_vulkan/vk_resource_pool.h"

namespace tracy {
class VkCtxScope;
}

namespace Vulkan {

class Instance;

struct RenderAttachment {
    vk::ImageView image_view;
    vk::ImageLayout image_layout;
    std::array<u32, 4> clear_value;
    union {
        u32 is_clear;
        struct {
            bool has_depth;
            bool depth_clear;
            bool has_stencil;
            bool stencil_clear;
        };
    };
};
static_assert(std::has_unique_object_representations_v<RenderAttachment>);

struct RenderState {
    std::array<RenderAttachment, 8> color_attachments;
    RenderAttachment depth_stencil_attachment;
    u16 width;
    u16 height;
    u16 num_layers;
    u16 num_color_attachments;

    bool operator==(const RenderState& other) const noexcept {
        return std::memcmp(this, &other, sizeof(RenderState)) == 0;
    }
};
static_assert(std::has_unique_object_representations_v<RenderState>);

/// What the guest asked of the GPU, counted between two presented frames. SHADPS4_FRAME_STATS=1
/// logs the averages every few seconds: they tell a frame made of a handful of passes from one
/// that restarts rendering to the same targets hundreds of times, which is what costs on a GPU
/// that renders in tiles. SHADPS4_FRAME_STATS=2 also lists the passes of one frame each time,
/// with what they were drawn to and which code ended them.
/// Whether a draw may keep what an earlier one worked out where nothing that goes into it has
/// changed since: the copy of a small buffer, the permutation of a shader, the image and view
/// of a texture. On, unless SHADPS4_DBG_NO_DRAW_REUSE is set: that is there to tell whether a
/// fault in the picture comes from here, and to measure what it saves.
extern const bool g_draw_reuse;
inline bool DrawReuseEnabled() {
    return g_draw_reuse;
}

struct FrameStats {
    static bool Enabled();
    static void RenderPass(const RenderState& state);
    static void PassEnded(const std::source_location& where);
    static void Draw();
    static void Compute();
    static void Submit();
    /// Called once per presented frame.
    static void EndFrame();

    /// The way of a frame from the title to the display, for the averages in the log.
    enum class Stage : u32 {
        Submitting, ///< From the title's first command list until it handed the frame over.
        CatchUp,    ///< From there until the GPU thread had translated all of it.
        Translate, ///< Processor time the GPU thread spent turning the frame into host commands.
        Queued,    ///< From the frame being handed on until a display refresh took it up.
        GpuWait,   ///< From there until the GPU had finished drawing it.
        Latency,   ///< From the title's first command list of the frame until its flip.
        Count,
    };
    static void Time(Stage stage, std::chrono::nanoseconds time);

    /// How often the shortcuts of the draw path are taken, per frame.
    enum class Counter : u32 {
        ImageLookups,       ///< Texture cache lookups by address.
        ImageLookupsShort,  ///< ... answered from what was found before.
        ImageRegistrations, ///< Images added to or removed from the cache.
        PipelineBinds,      ///< Graphics pipeline binds asked for.
        PipelineBindsShort, ///< ... that found the pipeline bound already.
        Vertices,           ///< Vertices (or indices) drawn, instances counted.
        Count,
    };
    static void Add(Counter counter, u64 amount = 1);
    /// The title handed a command list over. The first one after a frame was taken marks the
    /// start of the next.
    static void GuestSubmit();
    /// When the frame now being flipped started, or the epoch if nothing was noted.
    static std::chrono::steady_clock::time_point TakeGuestSubmit();
    /// Processor time the calling thread has used so far.
    static std::chrono::nanoseconds ThreadTime();

    /// How busy the GPU is with what the title draws, whether statistics are logged or not.
    /// Somebody looks many times a second whether the GPU still has work of the title's
    /// before it (GpuLook); the share of looks that found it busy since the last asking is the
    /// answer (TakeGpuLoad, below zero when nobody looked).
    static void GpuLook(bool busy);
    static float TakeGpuLoad();
};

/// True when SHADPS4_TRACE_SYNC is set: submissions and fence operations are then written to
/// stderr with a millisecond clock, to be read next to the GPU driver shim's own trace.
bool IsSyncTraceEnabled();
void TraceSync(const char* format, ...) __attribute__((format(printf, 1, 2)));

struct SubmitInfo {
    std::array<vk::Semaphore, 3> wait_semas;
    std::array<u64, 3> wait_ticks;
    std::array<vk::Semaphore, 3> signal_semas;
    std::array<u64, 3> signal_ticks;
    vk::Fence fence;
    u32 num_wait_semas;
    u32 num_signal_semas;

    void AddWait(vk::Semaphore semaphore, u64 tick = 1) {
        wait_semas[num_wait_semas] = semaphore;
        wait_ticks[num_wait_semas++] = tick;
    }

    void AddSignal(vk::Semaphore semaphore, u64 tick = 1) {
        signal_semas[num_signal_semas] = semaphore;
        signal_ticks[num_signal_semas++] = tick;
    }

    void AddSignal(vk::Fence fence) {
        this->fence = fence;
    }
};

using Viewports = boost::container::static_vector<vk::Viewport, AmdGpu::NUM_VIEWPORTS>;
using Scissors = boost::container::static_vector<vk::Rect2D, AmdGpu::NUM_VIEWPORTS>;
using ColorWriteMasks = std::array<vk::ColorComponentFlags, AmdGpu::NUM_COLOR_BUFFERS>;
struct StencilOps {
    vk::StencilOp fail_op{};
    vk::StencilOp pass_op{};
    vk::StencilOp depth_fail_op{};
    vk::CompareOp compare_op{};

    bool operator==(const StencilOps& other) const {
        return fail_op == other.fail_op && pass_op == other.pass_op &&
               depth_fail_op == other.depth_fail_op && compare_op == other.compare_op;
    }
};
struct DynamicState {
    struct {
        bool viewports : 1;
        bool scissors : 1;

        bool depth_test_enabled : 1;
        bool depth_write_enabled : 1;
        bool depth_compare_op : 1;

        bool depth_bounds_test_enabled : 1;
        bool depth_bounds : 1;

        bool depth_bias_enabled : 1;
        bool depth_bias : 1;

        bool stencil_test_enabled : 1;
        bool stencil_front_ops : 1;
        bool stencil_front_reference : 1;
        bool stencil_front_write_mask : 1;
        bool stencil_front_compare_mask : 1;
        bool stencil_back_ops : 1;
        bool stencil_back_reference : 1;
        bool stencil_back_write_mask : 1;
        bool stencil_back_compare_mask : 1;

        bool primitive_restart_enable : 1;
        bool rasterizer_discard_enable : 1;
        bool cull_mode : 1;
        bool front_face : 1;

        bool blend_constants : 1;
        bool color_write_masks : 1;
        bool line_width : 1;
        bool feedback_loop_enabled : 1;
    } dirty_state{};
    /// How many times Invalidate was called: what else is kept of the state of a command
    /// buffer (the vertex input, by the buffer cache) is void when this has changed.
    u64 invalidations{};

    Viewports viewports{};
    Scissors scissors{};

    bool depth_test_enabled{};
    bool depth_write_enabled{};
    vk::CompareOp depth_compare_op{};

    bool depth_bounds_test_enabled{};
    float depth_bounds_min{};
    float depth_bounds_max{};

    bool depth_bias_enabled{};
    float depth_bias_constant{};
    float depth_bias_clamp{};
    float depth_bias_slope{};

    bool stencil_test_enabled{};
    StencilOps stencil_front_ops{};
    u32 stencil_front_reference{};
    u32 stencil_front_write_mask{};
    u32 stencil_front_compare_mask{};
    StencilOps stencil_back_ops{};
    u32 stencil_back_reference{};
    u32 stencil_back_write_mask{};
    u32 stencil_back_compare_mask{};

    bool primitive_restart_enable{};
    bool rasterizer_discard_enable{};
    vk::CullModeFlags cull_mode{};
    vk::FrontFace front_face{};

    std::array<float, 4> blend_constants{};
    ColorWriteMasks color_write_masks{};
    float line_width{};
    bool feedback_loop_enabled{};

    /// Commits the dynamic state to the provided command buffer, or to anything that takes the
    /// same calls.
    template <typename Commands>
    void Commit(const Instance& instance, const Commands& cmdbuf);

    bool IsDirty() const {
        static constexpr decltype(dirty_state) clean{};
        return std::memcmp(&dirty_state, &clean, sizeof(dirty_state)) != 0;
    }

    /// Invalidates all dynamic state to be flushed into the next command buffer.
    void Invalidate() {
        std::memset(&dirty_state, 0xFF, sizeof(dirty_state));
        ++invalidations;
    }

    void SetViewports(const Viewports& viewports_) {
        if (!std::ranges::equal(viewports, viewports_)) {
            viewports = viewports_;
            dirty_state.viewports = true;
        }
    }

    void SetScissors(const Scissors& scissors_) {
        if (!std::ranges::equal(scissors, scissors_)) {
            scissors = scissors_;
            dirty_state.scissors = true;
        }
    }

    void SetDepthTestEnabled(const bool enabled) {
        if (depth_test_enabled != enabled) {
            depth_test_enabled = enabled;
            dirty_state.depth_test_enabled = true;
        }
    }

    void SetDepthWriteEnabled(const bool enabled) {
        if (depth_write_enabled != enabled) {
            depth_write_enabled = enabled;
            dirty_state.depth_write_enabled = true;
        }
    }

    void SetDepthCompareOp(const vk::CompareOp compare_op) {
        if (depth_compare_op != compare_op) {
            depth_compare_op = compare_op;
            dirty_state.depth_compare_op = true;
        }
    }

    void SetDepthBoundsTestEnabled(const bool enabled) {
        if (depth_bounds_test_enabled != enabled) {
            depth_bounds_test_enabled = enabled;
            dirty_state.depth_bounds_test_enabled = true;
        }
    }

    void SetDepthBounds(const float min, const float max) {
        if (depth_bounds_min != min || depth_bounds_max != max) {
            depth_bounds_min = min;
            depth_bounds_max = max;
            dirty_state.depth_bounds = true;
        }
    }

    void SetDepthBiasEnabled(const bool enabled) {
        if (depth_bias_enabled != enabled) {
            depth_bias_enabled = enabled;
            dirty_state.depth_bias_enabled = true;
        }
    }

    void SetDepthBias(const float constant, const float clamp, const float slope) {
        if (depth_bias_constant != constant || depth_bias_clamp != clamp ||
            depth_bias_slope != slope) {
            depth_bias_constant = constant;
            depth_bias_clamp = clamp;
            depth_bias_slope = slope;
            dirty_state.depth_bias = true;
        }
    }

    void SetStencilTestEnabled(const bool enabled) {
        if (stencil_test_enabled != enabled) {
            stencil_test_enabled = enabled;
            dirty_state.stencil_test_enabled = true;
        }
    }

    void SetStencilOps(const StencilOps& front_ops, const StencilOps& back_ops) {
        if (stencil_front_ops != front_ops) {
            stencil_front_ops = front_ops;
            dirty_state.stencil_front_ops = true;
        }
        if (stencil_back_ops != back_ops) {
            stencil_back_ops = back_ops;
            dirty_state.stencil_back_ops = true;
        }
    }

    void SetStencilReferences(const u32 front_reference, const u32 back_reference) {
        if (stencil_front_reference != front_reference) {
            stencil_front_reference = front_reference;
            dirty_state.stencil_front_reference = true;
        }
        if (stencil_back_reference != back_reference) {
            stencil_back_reference = back_reference;
            dirty_state.stencil_back_reference = true;
        }
    }

    void SetStencilWriteMasks(const u32 front_write_mask, const u32 back_write_mask) {
        if (stencil_front_write_mask != front_write_mask) {
            stencil_front_write_mask = front_write_mask;
            dirty_state.stencil_front_write_mask = true;
        }
        if (stencil_back_write_mask != back_write_mask) {
            stencil_back_write_mask = back_write_mask;
            dirty_state.stencil_back_write_mask = true;
        }
    }

    void SetStencilCompareMasks(const u32 front_compare_mask, const u32 back_compare_mask) {
        if (stencil_front_compare_mask != front_compare_mask) {
            stencil_front_compare_mask = front_compare_mask;
            dirty_state.stencil_front_compare_mask = true;
        }
        if (stencil_back_compare_mask != back_compare_mask) {
            stencil_back_compare_mask = back_compare_mask;
            dirty_state.stencil_back_compare_mask = true;
        }
    }

    void SetPrimitiveRestartEnabled(const bool enabled) {
        if (primitive_restart_enable != enabled) {
            primitive_restart_enable = enabled;
            dirty_state.primitive_restart_enable = true;
        }
    }

    void SetCullMode(const vk::CullModeFlags cull_mode_) {
        if (cull_mode != cull_mode_) {
            cull_mode = cull_mode_;
            dirty_state.cull_mode = true;
        }
    }

    void SetFrontFace(const vk::FrontFace front_face_) {
        if (front_face != front_face_) {
            front_face = front_face_;
            dirty_state.front_face = true;
        }
    }

    void SetBlendConstants(const std::array<float, 4> blend_constants_) {
        if (blend_constants != blend_constants_) {
            blend_constants = blend_constants_;
            dirty_state.blend_constants = true;
        }
    }

    void SetRasterizerDiscardEnabled(const bool enabled) {
        if (rasterizer_discard_enable != enabled) {
            rasterizer_discard_enable = enabled;
            dirty_state.rasterizer_discard_enable = true;
        }
    }

    void SetColorWriteMasks(const ColorWriteMasks& color_write_masks_) {
        if (!std::ranges::equal(color_write_masks, color_write_masks_)) {
            color_write_masks = color_write_masks_;
            dirty_state.color_write_masks = true;
        }
    }

    void SetLineWidth(const float width) {
        if (line_width != width) {
            line_width = width;
            dirty_state.line_width = true;
        }
    }

    void SetAttachmentFeedbackLoopEnabled(const bool enabled) {
        if (feedback_loop_enabled != enabled) {
            feedback_loop_enabled = enabled;
            dirty_state.feedback_loop_enabled = true;
        }
    }
};

class Scheduler {
public:
    explicit Scheduler(const Instance& instance);
    ~Scheduler();

    /// Sends the current execution context to the GPU
    /// and increments the scheduler timeline semaphore.
    void Flush(SubmitInfo& info);

    /// Sends the current execution context to the GPU
    /// and increments the scheduler timeline semaphore.
    void Flush();

    /// Sends the current execution context to the GPU and waits for it to complete.
    void Finish();

    /// Waits for the given tick to trigger on the GPU.
    void Wait(u64 tick);

    /// Attempts to execute operations whose tick the GPU has caught up with.
    void PopPendingOperations();

    /// Starts a new rendering scope with provided state.
    void BeginRendering(const RenderState& new_state);

    /// Ends current rendering scope.
    void EndRendering(std::source_location where = std::source_location::current());

    /// The same for a caller that goes on handing commands over (Record) instead of asking
    /// for the command buffer: the end of the pass is not waited for.
    void EndRenderingRecorded(std::source_location where = std::source_location::current());

    /// Returns the current render state.
    const RenderState& GetRenderState() const {
        return render_state;
    }

    /// The number of the render pass begun last, and whether that pass is still the open one:
    /// nothing that ends a pass (an upload, a barrier, a copy, a submit) has happened since.
    u64 PassSerial() const {
        return pass_serial;
    }
    bool IsPassOpen(u64 serial) const {
        return is_rendering && pass_serial == serial;
    }

    /// Binds a graphics pipeline for the draws of the title, unless it is the one the last draw
    /// of this render pass was made with: a run of objects drawn with the same pipeline is
    /// common, and binding it again makes the driver send the GPU its whole state again.
    void BindGraphicsPipeline(vk::Pipeline pipeline) {
        FrameStats::Add(FrameStats::Counter::PipelineBinds);
        if (bound_pipeline != pipeline) {
            Record([pipeline](vk::CommandBuffer cmdbuf, const u8*) {
                cmdbuf.bindPipeline(vk::PipelineBindPoint::eGraphics, pipeline);
            });
            bound_pipeline = pipeline;
        } else {
            FrameStats::Add(FrameStats::Counter::PipelineBindsShort);
        }
    }

    /// Begins a query inside the render pass that is open; it ends with EndQuery or with the
    /// pass, whichever comes first.
    void BeginQuery(vk::QueryPool pool, u32 index) {
        Record([pool, index](vk::CommandBuffer cmdbuf, const u8*) {
            cmdbuf.beginQuery(pool, index, {});
        });
        active_query_pool = pool;
        active_query_index = index;
        has_active_query = true;
    }

    /// Ends the query begun with BeginQuery, if it is still running.
    void EndQuery() {
        if (has_active_query) {
            Record([pool = active_query_pool, index = active_query_index](
                       vk::CommandBuffer cmdbuf, const u8*) { cmdbuf.endQuery(pool, index); });
            has_active_query = false;
        }
    }

    bool HasActiveQuery() const {
        return has_active_query;
    }

    /// Counts a draw towards the next flush (see IsFlushDue).
    void NoteDraw() {
        ++draws_since_flush;
    }

    /// True when enough has been recorded, and a render pass has ended since, for the GPU to be
    /// given it now instead of at the end of the frame. A frame takes the GPU thread about as
    /// long to translate as it takes the GPU to draw: handing everything over at the end makes
    /// the two take turns, handing it over in parts lets them work at the same time.
    bool IsFlushDue() const {
        return flush_due;
    }

    /// Returns the current pipeline dynamic state tracking.
    DynamicState& GetDynamicState() {
        return dynamic_state;
    }

    void CommitDynamicState();

    /// Returns the current command buffer, with everything recorded so far in it.
    vk::CommandBuffer CommandBuffer() const {
        Drain();
        return current_cmdbuf;
    }

    /// Hands a command over to be recorded into the current command buffer, after everything
    /// recorded before it and before anything recorded after it. The draws of the title are
    /// recorded like this by a thread of its own: what the driver does for a draw is a fifth of
    /// what the draw costs the GPU thread, and none of it has to be waited for. Whoever asks
    /// for the command buffer itself (CommandBuffer) gets it once that thread is through.
    /// `command` is called as command(vk::CommandBuffer, const u8* payload) and must hold what
    /// it needs by value; `payload_size` bytes come with it, to be filled in through the
    /// pointer returned before Publish is called.
    template <typename F>
    u8* Reserve(u32 payload_size, F&& command) {
        using Command = std::decay_t<F>;
        static_assert(alignof(Command) <= RecordAlign);
        static constexpr u32 closure_size =
            (sizeof(Command) + RecordAlign - 1) & ~(RecordAlign - 1);
        const u32 size = sizeof(RecordHeader) + closure_size +
                         ((payload_size + RecordAlign - 1) & ~(RecordAlign - 1));
        RecordHeader* const header = ReserveRecord(size);
        u8* const closure = reinterpret_cast<u8*>(header + 1);
        header->run = [](vk::CommandBuffer cmdbuf, u8* data) {
            auto* const stored = reinterpret_cast<Command*>(data);
            (*stored)(cmdbuf, const_cast<const u8*>(data) + closure_size);
            std::destroy_at(stored);
        };
        std::construct_at(reinterpret_cast<Command*>(closure), std::forward<F>(command));
        return closure + closure_size;
    }

    /// The most that may come with a command.
    static constexpr u32 MaxRecordPayload = 32 * 1024;

    /// Makes what Reserve prepared part of the command stream.
    void Publish() {
        RecordHeader* const header = record_reserved;
        if (!record_threaded) {
            header->run(current_cmdbuf, reinterpret_cast<u8*>(header + 1));
            return;
        }
        record_produced += header->size;
    }

    /// Lets the recording thread start on what was handed over so far. Telling it costs about
    /// what recording a small command does: it is told once in a few draws (KickAfterDraw),
    /// and whenever it is waited for.
    void Kick() {
        if (record_produced != record_kicked) {
            KickRecords();
        }
    }
    void KickAfterDraw() {
        if (++record_draws_unkicked >= DrawsPerKick) {
            Kick();
        }
    }

    /// Whether commands handed over are recorded by another thread, later.
    bool IsRecordThreaded() const {
        return record_threaded;
    }

    template <typename F>
    void Record(F&& command) {
        Reserve(0, std::forward<F>(command));
        Publish();
    }

    /// A write to the title's memory was handed over (Record): reads of that range by the
    /// thread that hands over wait for it (BeforeGuestRead).
    void NoteGuestWrite(u64 address, u32 size, u64 data) {
        if (!record_threaded) {
            return;
        }
        if (guest_writes.size() < guest_writes.capacity()) {
            guest_writes.push_back({address, size, data});
        } else {
            guest_writes_overflow = true;
        }
        guest_writes_pending = true;
    }
    void BeforeGuestRead(u64 address, u64 size) {
        if (guest_writes_pending) {
            CheckGuestRead(address, size);
        }
    }

    /// What a read of `bytes` (up to four) at `address` finds once the writes handed over
    /// are made: nothing of theirs (None), `value` (Known), or something that takes waiting
    /// for them to tell (Unknown).
    enum class GuestWrite { None, Known, Unknown };
    GuestWrite PendingGuestWrite(u64 address, u32 bytes, u32& value);

    /// Waits until everything recorded so far is in the command buffer.
    void Drain() const {
        if (record_produced != record_consumed.load(std::memory_order_acquire)) {
            WaitDrained();
        }
    }

    /// Returns the current command buffer tick.
    [[nodiscard]] u64 CurrentTick() const noexcept {
        return master_semaphore.CurrentTick();
    }

    /// Returns true when a tick has been triggered by the GPU.
    [[nodiscard]] bool IsFree(u64 tick) noexcept {
        if (master_semaphore.IsFree(tick)) {
            return true;
        }
        master_semaphore.Refresh();
        return master_semaphore.IsFree(tick);
    }

    /// Returns the master timeline semaphore.
    [[nodiscard]] MasterSemaphore* GetMasterSemaphore() noexcept {
        return &master_semaphore;
    }

    /// Defers an operation until the gpu has reached the current cpu tick.
    /// Will be run when submitting or calling PopPendingOperations.
    void DeferOperation(Common::UniqueFunction<void>&& func) {
        // More than one thread defers operations (the headset coming back does, for one).
        std::scoped_lock lk{pending_ops_mutex};
        pending_ops.emplace(std::move(func), CurrentTick());
    }

    /// Defers an operation until the gpu has reached the current cpu tick.
    /// Runs as soon as possible in another thread.
    void DeferPriorityOperation(Common::UniqueFunction<void>&& func) {
        {
            std::unique_lock lk(priority_pending_ops_mutex);
            priority_pending_ops.emplace(std::move(func), CurrentTick());
        }
        priority_pending_ops_cv.notify_one();
    }

    static std::mutex submit_mutex;

private:
    static constexpr u32 RecordAlign = 16;
    static constexpr u32 RecordSize = 1u << 20;
    static constexpr u32 MaxRecord = 1u << 16;
    /// (The command buffer is the current one: it is only changed once the ring is empty.)
    struct alignas(RecordAlign) RecordHeader {
        void (*run)(vk::CommandBuffer cmdbuf, u8* data);
        u32 size;
    };
    static_assert(sizeof(RecordHeader) % RecordAlign == 0);

    RecordHeader* ReserveRecord(u32 size);
    void KickRecords();
    void WaitDrained() const;
    void RunRecords(u64 end);
    void WaitRecordSpace(u64 needed);
    void RecordThread();
    void CheckGuestRead(u64 address, u64 size);
    void RecordEndRendering();
    static void BeginRenderingOf(const RenderState& state, vk::CommandBuffer cmdbuf);

    void AllocateWorkerCommandBuffers();

    void SubmitExecution(SubmitInfo& info);

    void PriorityPendingOpsThread(std::stop_token stoken);

private:
    const Instance& instance;
    MasterSemaphore master_semaphore;
    CommandPool command_pool;
    DynamicState dynamic_state;
    vk::CommandBuffer current_cmdbuf;
    std::condition_variable_any event_cv;
    struct PendingOp {
        Common::UniqueFunction<void> callback;
        u64 gpu_tick;
    };
    std::queue<PendingOp> pending_ops;
    std::mutex pending_ops_mutex;
    std::queue<PendingOp> priority_pending_ops;
    std::mutex priority_pending_ops_mutex;
    std::condition_variable_any priority_pending_ops_cv;
    std::jthread priority_pending_ops_thread;
    RenderState render_state;
    bool is_rendering = false;
    u64 pass_serial{};
    u32 draws_since_flush{};
    bool flush_due{};
    vk::QueryPool active_query_pool;
    u32 active_query_index{};
    bool has_active_query{};
    /// What BindGraphicsPipeline bound last in the current render pass. Other code binds
    /// pipelines of its own, always in a pass of its own: nothing is assumed across passes.
    vk::Pipeline bound_pipeline{};
    tracy::VkCtxScope* profiler_scope{};

    /// The commands handed over (Reserve), in a ring: `record_produced` bytes have been
    /// written to it since the start, `record_published` of them may be run, and
    /// `record_consumed` have been.
    const bool record_threaded;
    std::unique_ptr<u8[]> record_buffer;
    RecordHeader* record_reserved{};
    u64 record_produced{};
    u64 record_kicked{};
    struct GuestWriteNote {
        u64 address;
        u32 size;
        u64 data;
    };
    boost::container::static_vector<GuestWriteNote, 64> guest_writes;
    bool guest_writes_overflow{};
    bool guest_writes_pending{};
    static constexpr u32 DrawsPerKick = 4;
    u32 record_draws_unkicked{};
    /// A value `record_consumed` has had: the ring is free at least up to there.
    u64 record_consumed_seen{};
    alignas(64) std::atomic<u64> record_published{};
    alignas(64) mutable std::atomic<u64> record_consumed{};
    alignas(64) std::atomic<bool> record_sleeping{};
    /// Held by whoever runs the commands of the ring: the recording thread, or the thread
    /// that waits for them when it finds the other one idle (WaitDrained).
    std::atomic<bool> record_running{};
    std::atomic<bool> record_stop{};
    std::thread record_thread;
};

} // namespace Vulkan
