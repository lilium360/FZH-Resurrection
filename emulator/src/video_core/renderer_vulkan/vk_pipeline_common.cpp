// SPDX-FileCopyrightText: Copyright 2024 shadPS4 Emulator Project
// SPDX-License-Identifier: GPL-2.0-or-later

#include <boost/container/small_vector.hpp>
#include <boost/container/static_vector.hpp>

#include "shader_recompiler/resource.h"
#include "video_core/renderer_vulkan/vk_instance.h"
#include "video_core/renderer_vulkan/vk_pipeline_cache.h"
#include "video_core/renderer_vulkan/vk_pipeline_common.h"
#include "video_core/renderer_vulkan/vk_scheduler.h"

namespace Vulkan {

Pipeline::Pipeline(const Instance& instance_, Scheduler& scheduler_, DescriptorHeap& desc_heap_,
                   const Shader::Profile& profile_, vk::PipelineCache pipeline_cache,
                   bool is_compute_ /*= false*/)
    : instance{instance_}, scheduler{scheduler_}, desc_heap{desc_heap_}, profile{profile_},
      is_compute{is_compute_} {}

Pipeline::~Pipeline() = default;

void Pipeline::BindResources(DescriptorWrites& set_writes, const BufferBarriers& buffer_barriers,
                             const Shader::PushData& push_data) const {
    const auto bind_point =
        IsCompute() ? vk::PipelineBindPoint::eCompute : vk::PipelineBindPoint::eGraphics;

    if (!buffer_barriers.empty()) {
        const auto dependencies = vk::DependencyInfo{
            .dependencyFlags = vk::DependencyFlagBits::eByRegion,
            .bufferMemoryBarrierCount = u32(buffer_barriers.size()),
            .pBufferMemoryBarriers = buffer_barriers.data(),
        };
        scheduler.EndRendering();
        scheduler.CommandBuffer().pipelineBarrier2(dependencies);
    }

    const vk::ShaderStageFlags stage_flags =
        IsCompute() ? vk::ShaderStageFlagBits::eCompute : AllGraphicsStageBits;

    {
        // Recorded with the writes and what they point at copied, one after the other: the
        // caller's are gone by the time the command buffer gets them. A set that is not
        // pushed is written by the recording thread as well: it is nobody else's until the
        // GPU is through with the commands it is bound for here.
        const vk::DescriptorSet desc_set = set_writes.empty() || uses_push_descriptors
                                               ? vk::DescriptorSet{}
                                               : desc_heap.Commit(*desc_layout);
        static constexpr u32 InfoSize = sizeof(vk::DescriptorBufferInfo);
        static_assert(sizeof(vk::DescriptorImageInfo) == InfoSize &&
                      alignof(vk::DescriptorImageInfo) == alignof(vk::DescriptorBufferInfo));
        const auto is_buffer = [](vk::DescriptorType type) {
            return type == vk::DescriptorType::eUniformBuffer ||
                   type == vk::DescriptorType::eStorageBuffer;
        };
        const u32 num_writes = static_cast<u32>(set_writes.size());
        u32 num_infos = 0;
        for (const auto& set_write : set_writes) {
            num_infos += set_write.descriptorCount;
        }
        // Of a write, only what differs from one to the next is handed over.
        struct RecordedWrite {
            u32 binding;
            u32 count;
            vk::DescriptorType type;
            u32 padding;
        };
        static_assert(sizeof(RecordedWrite) == 16);
        const u32 writes_size = num_writes * sizeof(RecordedWrite);
        u8* const payload = scheduler.Reserve(
            writes_size + num_infos * InfoSize,
            [layout = *pipeline_layout, bind_point, stage_flags, push_data, num_writes, is_buffer,
             desc_set, device = instance.GetDevice()](vk::CommandBuffer cmdbuf, const u8* data) {
                cmdbuf.pushConstants(layout, stage_flags, 0u, sizeof(push_data), &push_data);
                if (num_writes == 0) {
                    return;
                }
                const auto* const recorded = reinterpret_cast<const RecordedWrite*>(data);
                const u8* infos = data + num_writes * sizeof(RecordedWrite);
                boost::container::small_vector<vk::WriteDescriptorSet, 48> writes_storage(
                    num_writes);
                auto* const writes = writes_storage.data();
                for (u32 i = 0; i < num_writes; ++i) {
                    auto& write = writes[i];
                    write.dstSet = desc_set;
                    write.dstBinding = recorded[i].binding;
                    write.descriptorCount = recorded[i].count;
                    write.descriptorType = recorded[i].type;
                    if (is_buffer(write.descriptorType)) {
                        write.pBufferInfo =
                            reinterpret_cast<const vk::DescriptorBufferInfo*>(infos);
                    } else {
                        write.pImageInfo = reinterpret_cast<const vk::DescriptorImageInfo*>(infos);
                    }
                    infos += write.descriptorCount * InfoSize;
                }
                if (!desc_set) {
                    cmdbuf.pushDescriptorSetKHR(bind_point, layout, 0, num_writes, writes);
                    return;
                }
                device.updateDescriptorSets(num_writes, writes, 0, nullptr);
                cmdbuf.bindDescriptorSets(bind_point, layout, 0, 1, &desc_set, 0, nullptr);
            });
        if (num_writes != 0) {
            auto* recorded = reinterpret_cast<RecordedWrite*>(payload);
            u8* infos = payload + writes_size;
            for (const auto& set_write : set_writes) {
                ASSERT(set_write.dstArrayElement == 0);
                *recorded++ = {set_write.dstBinding, set_write.descriptorCount,
                               set_write.descriptorType, 0};
                const void* const source = is_buffer(set_write.descriptorType)
                                               ? static_cast<const void*>(set_write.pBufferInfo)
                                               : static_cast<const void*>(set_write.pImageInfo);
                std::memcpy(infos, source, set_write.descriptorCount * InfoSize);
                infos += set_write.descriptorCount * InfoSize;
            }
        }
        scheduler.Publish();
    }
}

std::string Pipeline::GetDebugString() const {
    std::string stage_desc;
    for (const auto& stage : stages) {
        if (stage) {
            const auto shader_name = PipelineCache::GetShaderName(stage->stage, stage->pgm_hash);
            if (stage_desc.empty()) {
                stage_desc = shader_name;
            } else {
                stage_desc = fmt::format("{},{}", stage_desc, shader_name);
            }
        }
    }
    return stage_desc;
}

} // namespace Vulkan
