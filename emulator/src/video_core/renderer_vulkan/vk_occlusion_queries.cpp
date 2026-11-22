// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <cstdlib>
#include <cstring>
#include <memory>

#include "common/assert.h"
#include "common/logging/log.h"
#include "video_core/renderer_vulkan/vk_instance.h"
#include "video_core/renderer_vulkan/vk_occlusion_queries.h"
#include "video_core/renderer_vulkan/vk_scheduler.h"

namespace Vulkan {

namespace {
constexpr u32 PoolSize = 4096;
constexpr u64 CounterValid = 0x8000000000000000ULL;
/// What a query answers when it cannot be counted: plainly visible.
constexpr u64 VisibleSamples = 0x2FFFFFFULL;
/// Draws outside any query after which a run of queries is taken to be over.
constexpr u32 DrawsToClose = 8;

void WriteGuest(u64* address, u64 value) {
    // Straight into the title's memory, as the first dump is. (Through the memory manager's
    // backing each write took some 5 us, and a frame has thousands: the answers of a run of
    // 400 queries came 20 ms late, and the title waits for them.)
    std::memcpy(address, &value, sizeof(value));
}
} // namespace

OcclusionQueries::OcclusionQueries(const Instance& instance_, Scheduler& scheduler_)
    : instance{instance_}, scheduler{scheduler_} {
    // SHADPS4_OCCLUSION_QUERIES=0: the made-up counters of before, everything visible.
    const char* value = std::getenv("SHADPS4_OCCLUSION_QUERIES");
    enabled = !(value != nullptr && value[0] == '0') && instance.IsHostQueryResetSupported();
    LOG_INFO(Render_Vulkan, "Occlusion queries: {}",
             enabled ? "counted by the host GPU" : "made up, everything visible");
    // SHADPS4_OCCLUSION_WAIT=1: this thread waits for the GPU and writes the answers itself,
    // instead of another thread writing them when the GPU has them. Measured on Firewall Zero
    // Hour the answers come no sooner (the GPU takes 3 to 16 ms to get there) and the wait is
    // taken from the translation of the title's commands.
    const char* wait = std::getenv("SHADPS4_OCCLUSION_WAIT");
    wait_for_answers = wait != nullptr && wait[0] == '1';
    if (enabled) {
        batch.pool = TakePool();
    }
}

OcclusionQueries::~OcclusionQueries() {
    const auto device = instance.GetDevice();
    for (const auto pool : all_pools) {
        device.destroyQueryPool(pool);
    }
}

vk::QueryPool OcclusionQueries::TakePool() {
    std::scoped_lock lock{pools_mutex};
    if (!free_pools.empty()) {
        const auto pool = free_pools.back();
        free_pools.pop_back();
        return pool;
    }
    const auto device = instance.GetDevice();
    const vk::QueryPoolCreateInfo pool_info = {
        .queryType = vk::QueryType::eOcclusion,
        .queryCount = PoolSize,
    };
    auto [result, pool] = device.createQueryPool(pool_info);
    ASSERT_MSG(result == vk::Result::eSuccess, "Failed to create an occlusion query pool: {}",
               vk::to_string(result));
    device.resetQueryPool(pool, 0, PoolSize);
    all_pools.push_back(pool);
    return pool;
}

bool OcclusionQueries::Dump(u64* address, u32 num_pairs) {
    if (!enabled) {
        return false;
    }
    if (open && address == open_begin + 1) {
        // The second dump of a pair: the query ends here.
        scheduler.EndQuery();
        batch.queries.push_back(GuestQuery{
            .end_address = address,
            .num_pairs = num_pairs,
            .first_slot = open_first_slot,
            .num_slots = batch.used - open_first_slot,
            .visible = open_visible,
        });
        if (batch.queries.size() == 1) {
            batch.first_query = std::chrono::steady_clock::now();
        }
        open = false;
        draws_outside = 0;
        return true;
    }
    if (open) {
        // A first dump with a query still open: that one is left as visible.
        static bool told = false;
        if (!std::exchange(told, true)) {
            LOG_WARNING(Render_Vulkan,
                        "An occlusion query was begun at {} with the one at {} still open; "
                        "the open one counts as visible",
                        fmt::ptr(address), fmt::ptr(open_begin));
        }
        scheduler.EndQuery();
        batch.queries.push_back(GuestQuery{
            .end_address = open_begin + 1,
            .num_pairs = open_pairs,
            .first_slot = open_first_slot,
            .num_slots = batch.used - open_first_slot,
            .visible = true,
        });
    }
    // The first dump: counters at zero, and no answer yet in the slots of the second.
    for (u32 i = 0; i < num_pairs; ++i) {
        address[2 * i] = CounterValid;
        address[2 * i + 1] = 0;
    }
    open = true;
    open_visible = false;
    open_begin = address;
    open_pairs = num_pairs;
    open_first_slot = batch.used;
    return true;
}

void OcclusionQueries::BeforeDraw() {
    if (!enabled) {
        return;
    }
    if (open) {
        // The pass may have ended since the last draw and taken the host query with it: the
        // title's query then goes on in another one, and the counts are added up.
        if (!scheduler.HasActiveQuery()) {
            if (batch.used < PoolSize) {
                scheduler.BeginQuery(batch.pool, batch.used++);
            } else {
                open_visible = true;
            }
        }
        return;
    }
    if (!batch.queries.empty() && ++draws_outside >= DrawsToClose) {
        // Not here: a draw is being recorded.
        close_due = true;
    }
}

void OcclusionQueries::Close() {
    close_due = false;
    if (!enabled || open || batch.queries.empty()) {
        return;
    }
    batch.closed = std::chrono::steady_clock::now();
    auto done = std::make_shared<Batch>(std::move(batch));
    batch = Batch{.pool = TakePool()};
    draws_outside = 0;
    // The title waits for these answers before it begins its next frame: the sooner the better.
    const u64 tick = scheduler.CurrentTick();
    if (wait_for_answers) {
        scheduler.Flush();
        scheduler.Wait(tick);
        Resolve(*done);
    } else {
        // On the scheduler's priority thread, once the GPU is through what was recorded so far.
        scheduler.DeferPriorityOperation([this, done] { Resolve(*done); });
        scheduler.Flush();
    }
}

void OcclusionQueries::Resolve(const Batch& done) {
    const auto device = instance.GetDevice();
    std::vector<u64> counts(done.used);
    bool counted = true;
    if (done.used != 0) {
        const auto result = device.getQueryPoolResults(
            done.pool, 0, done.used, counts.size() * sizeof(u64), counts.data(), sizeof(u64),
            vk::QueryResultFlagBits::e64 | vk::QueryResultFlagBits::eWait);
        counted = result == vk::Result::eSuccess;
        device.resetQueryPool(done.pool, 0, done.used);
    }
    for (const auto& query : done.queries) {
        u64 samples = 0;
        if (query.visible || !counted) {
            samples = VisibleSamples;
        } else {
            for (u32 slot = 0; slot < query.num_slots; ++slot) {
                samples += counts[query.first_slot + slot];
            }
        }
        // The title adds up, over the pairs, how far each counter moved: all of it in the first.
        for (u32 i = 0; i < query.num_pairs; ++i) {
            WriteGuest(query.end_address + 2 * i, CounterValid | (i == 0 ? samples : 0));
        }
    }
    // SHADPS4_FRAME_STATS: how long the answers take, every five seconds.
    static const bool stats = [] {
        const char* value = std::getenv("SHADPS4_FRAME_STATS");
        return value != nullptr && value[0] != 0 && value[0] != '0';
    }();
    if (stats) {
        using namespace std::chrono;
        static u64 batches = 0, queries = 0, hidden = 0;
        static double since_first = 0.0, since_close = 0.0;
        static steady_clock::time_point told = steady_clock::now();
        const auto now = steady_clock::now();
        ++batches;
        queries += done.queries.size();
        for (const auto& query : done.queries) {
            u64 samples = query.visible ? 1 : 0;
            for (u32 slot = 0; slot < query.num_slots && counted; ++slot) {
                samples += counts[query.first_slot + slot];
            }
            hidden += samples == 0;
        }
        since_first += duration<double, std::milli>(now - done.first_query).count();
        since_close += duration<double, std::milli>(now - done.closed).count();
        if (now - told >= seconds{5}) {
            LOG_INFO(Render_Vulkan,
                     "occlusion queries: {} runs of {:.0f} queries, {:.0f}% answered hidden; "
                     "answers {:.2f} ms after the run's first query, {:.2f} ms after its end",
                     batches, double(queries) / batches, 100.0 * hidden / std::max<u64>(queries, 1),
                     since_first / batches, since_close / batches);
            batches = queries = hidden = 0;
            since_first = since_close = 0.0;
            told = now;
        }
    }
    std::scoped_lock lock{pools_mutex};
    free_pools.push_back(done.pool);
}

} // namespace Vulkan
