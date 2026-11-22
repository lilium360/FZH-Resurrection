// SPDX-FileCopyrightText: Copyright 2026 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <chrono>
#include <mutex>
#include <utility>
#include <vector>

#include "common/types.h"
#include "video_core/renderer_vulkan/vk_common.h"

namespace Vulkan {

class Instance;
class Scheduler;

/// Answers the title's occlusion queries with what the host GPU counts.
///
/// A title asks whether something is hidden by drawing its bounding box between two dumps of the
/// GPU's z-pass counters and reading, a frame later, how far they moved. The counters used to be
/// made up and always moved, so everything counted as visible and a title that culls by these
/// answers (Unreal does) drew every object in front of the camera, walls or not: Firewall Zero
/// Hour drew 7800 draws a frame standing in a closed room, 650 when every query said "hidden".
///
/// Here the draws between the two dumps run inside a host occlusion query. The first dump is
/// answered at once (counters at zero); the second is written when the host GPU has the count,
/// from the scheduler's priority thread, so the thread translating the title's commands never
/// waits for it. The title waits for the second dump's valid bit as it would on the console.
class OcclusionQueries {
public:
    explicit OcclusionQueries(const Instance& instance, Scheduler& scheduler);
    ~OcclusionQueries();

    /// A dump of the z-pass counters to `address`. False when the queries are off and the
    /// caller is to answer as before.
    bool Dump(u64* address, u32 num_pairs);

    /// A draw is about to be recorded; the scheduler is inside its render pass.
    void BeforeDraw();

    /// A draw was left out (no pipeline yet, nothing bound): a query it belonged to cannot say
    /// "hidden" on its account.
    void DrawDropped() {
        open_visible = true;
    }

    /// The title's submission is through; a flush follows.
    void OnSubmit() {
        Close();
    }

    /// Between two draws: a run of queries that is over gets its answers.
    void BetweenDraws() {
        if (close_due) {
            Close();
        }
    }

private:
    struct GuestQuery {
        u64* end_address;
        u32 num_pairs;
        u32 first_slot;
        u32 num_slots;
        bool visible;
    };
    struct Batch {
        vk::QueryPool pool;
        u32 used{};
        std::vector<GuestQuery> queries;
        std::chrono::steady_clock::time_point first_query;
        std::chrono::steady_clock::time_point closed;
    };

    vk::QueryPool TakePool();
    void Close();
    void Resolve(const Batch& batch);

    const Instance& instance;
    Scheduler& scheduler;
    bool enabled{};
    Batch batch;
    bool open{};
    bool open_visible{};
    u64* open_begin{};
    u32 open_pairs{};
    u32 open_first_slot{};
    u32 draws_outside{};
    bool close_due{};
    bool wait_for_answers{};
    std::mutex pools_mutex;
    std::vector<vk::QueryPool> free_pools;
    std::vector<vk::QueryPool> all_pools;
};

} // namespace Vulkan
