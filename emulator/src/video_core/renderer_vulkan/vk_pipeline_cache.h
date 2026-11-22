// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#pragma once

#include <atomic>
#include <condition_variable>
#include <deque>
#include <functional>
#include <memory>
#include <mutex>
#include <thread>
#include <variant>
#include <tsl/robin_map.h>
#include "shader_recompiler/profile.h"
#include "shader_recompiler/recompiler.h"
#include "shader_recompiler/specialization.h"
#include "video_core/renderer_vulkan/vk_compute_pipeline.h"
#include "video_core/renderer_vulkan/vk_graphics_pipeline.h"
#include "video_core/renderer_vulkan/vk_resource_pool.h"

template <>
struct std::hash<vk::ShaderModule> {
    std::size_t operator()(const vk::ShaderModule& module) const noexcept {
        return std::hash<size_t>{}(reinterpret_cast<size_t>((VkShaderModule)module));
    }
};

namespace AmdGpu {
class Liverpool;
}

namespace Serialization {
struct Archive;
}

namespace Shader {
struct Info;
}

namespace Vulkan {

class Instance;
class Scheduler;
class ShaderCache;

struct Program {
    struct Module {
        vk::ShaderModule module;
        Shader::StageSpecialization spec;
    };
    static constexpr size_t MaxPermutations = 8;
    using ModuleList = boost::container::small_vector<Module, MaxPermutations>;

    Shader::Info info;
    ModuleList modules{};

    /// The last draw with the program for which a permutation of it was found: what its
    /// specialization was made from, and which permutation that was. Most draws of a program
    /// differ from the one before only in where their buffers and textures lie, which a
    /// specialization does not go by: they are told from this, without making the
    /// specialization and comparing it with every permutation again (an eighth of what a
    /// draw cost the GPU thread).
    struct LastUse {
        bool valid{};
        Shader::RuntimeInfo runtime_info{};
        Shader::Backend::Bindings start{};
        boost::container::small_vector<u64, 96> source;
        size_t perm_idx{};
    };
    LastUse last_use;

    Program() = default;
    Program(Shader::Stage stage, Shader::LogicalStage l_stage, Shader::ShaderParams params)
        : info{stage, l_stage, params} {}

    void AddPermut(vk::ShaderModule module, Shader::StageSpecialization&& spec) {
        modules.emplace_back(module, std::move(spec));
    }

    void InsertPermut(vk::ShaderModule module, Shader::StageSpecialization&& spec,
                      size_t perm_idx) {
        modules.resize(std::max(modules.size(), perm_idx + 1)); // <-- beware of realloc
        modules[perm_idx] = {module, std::move(spec)};
    }
};

class PipelineCache {
public:
    explicit PipelineCache(const Instance& instance, Scheduler& scheduler,
                           AmdGpu::Liverpool* liverpool);
    ~PipelineCache();

    void WarmUp();
    void Sync();

    bool LoadComputePipeline(Serialization::Archive& ar);
    bool LoadGraphicsPipeline(Serialization::Archive& ar);
    bool LoadPipelineStage(Serialization::Archive& ar, size_t stage);

    const GraphicsPipeline* GetGraphicsPipeline();

    /// The colour targets (a bit each) that the graphics pipelines asked for from now on leave
    /// out, as if the draw had nothing bound there. For a draw that has one surface bound as
    /// more than one of its targets, which is drawn once for each (Rasterizer::Draw).
    void LeaveTargetsOut(u8 targets) {
        targets_left_out = targets;
    }

    const ComputePipeline* GetComputePipeline();

    /// The fetch shader data is the one kept with the program's permutation, or null: good
    /// until that program gets another permutation.
    using Result = std::tuple<const Shader::Info*, vk::ShaderModule,
                              const Shader::Gcn::FetchShaderData*, u64>;
    Result GetProgram(Shader::Stage stage, Shader::LogicalStage l_stage,
                      const Shader::ShaderParams& params, Shader::Backend::Bindings& binding);

    std::optional<vk::ShaderModule> ReplaceShader(vk::ShaderModule module,
                                                  std::span<const u32> spv_code);
    bool ReplaceDiagnosticGroundShader(std::span<const u32> spv_code);

    static std::string GetShaderName(Shader::Stage stage, u64 hash,
                                     std::optional<size_t> perm = {});

    auto& GetProfile() const {
        return profile;
    }

private:
    bool RefreshGraphicsKey();
    bool RefreshGraphicsStages();
    bool RefreshComputeKey();

    void DumpShader(std::span<const u32> code, u64 hash, Shader::Stage stage, size_t perm_idx,
                    std::string_view ext);
    std::optional<std::vector<u32>> GetShaderPatch(u64 hash, Shader::Stage stage, size_t perm_idx,
                                                   std::string_view ext);
    vk::ShaderModule CompileModule(Shader::Info& info, Shader::RuntimeInfo& runtime_info,
                                   const std::span<const u32>& code, size_t perm_idx,
                                   Shader::Backend::Bindings& binding);
    const Shader::RuntimeInfo& BuildRuntimeInfo(Shader::Stage stage, Shader::LogicalStage l_stage);

    [[nodiscard]] bool IsPipelineCacheDirty() const {
        return num_new_pipelines > 0;
    }

private:
    const Instance& instance;
    Scheduler& scheduler;
    AmdGpu::Liverpool* liverpool;
    DescriptorHeap desc_heap;
    vk::UniquePipelineCache pipeline_cache;
    vk::UniquePipelineLayout pipeline_layout;
    Shader::Profile profile{};
    Shader::Pools pools;
    tsl::robin_map<size_t, std::unique_ptr<Program>> program_cache;
    tsl::robin_map<ComputePipelineKey, std::unique_ptr<ComputePipeline>> compute_pipelines;
    tsl::robin_map<GraphicsPipelineKey, std::unique_ptr<GraphicsPipeline>> graphics_pipelines;
    std::array<Shader::RuntimeInfo, MaxShaderStages> runtime_infos{};
    std::array<const Shader::Info*, MaxShaderStages> infos{};
    std::array<vk::ShaderModule, MaxShaderStages> modules{};
    /// Of the vertex stage of the pipeline being looked up; see Result.
    const Shader::Gcn::FetchShaderData* fetch_shader{};
    // Preloading must retain vertex fetch data across later shader stages.
    std::optional<Shader::Gcn::FetchShaderData> preload_fetch_shader{};
    GraphicsPipelineKey graphics_key{};
    /// The pipeline the last draw was given, and its key: for most draws it is the one of the
    /// draw before, and the two keys are compared sooner than one is looked up.
    GraphicsPipelineKey last_graphics_key{};
    const GraphicsPipeline* last_graphics_pipeline{};
    Shader::SpecializationSource specialization_source;
    /// Time spent translating shaders and building a pipeline for the draw being looked up.
    std::chrono::steady_clock::duration compile_wait{};
    u8 targets_left_out{};
    ComputePipelineKey compute_key{};
    u32 num_new_pipelines{}; // new pipelines added to the cache since the game start

    // Only if Config::collectShadersForDebug()
    tsl::robin_map<vk::ShaderModule,
                   std::vector<std::variant<GraphicsPipelineKey, ComputePipelineKey>>>
        module_related_pipelines;

    // Graphics pipelines built away from the title's draws, so that a draw never waits for the
    // driver to compile: until its pipeline is ready the draw is left out. On by default where
    // the vertex input is dynamic state; SHADPS4_ASYNC_PIPELINES=0 builds them in the draw again.
    struct PendingGraphics {
        std::unique_ptr<GraphicsPipeline> pipeline;
        GraphicsPipeline::SerializationSupport sdata{};
        std::atomic<bool> done{false};
        bool failed{false};
    };
    void BuildThread(std::stop_token stop);

    bool async_pipelines{};
    tsl::robin_map<GraphicsPipelineKey, std::shared_ptr<PendingGraphics>> pending_graphics;
    std::mutex build_mutex;
    std::condition_variable_any build_cv;
    std::deque<std::function<void()>> build_queue;
    // Last, so that the threads are gone before anything they use is.
    std::vector<std::jthread> build_threads;
};

} // namespace Vulkan
