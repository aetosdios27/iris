use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;

use super::context::VkContext;
use super::shader;
use crate::error::{IrisError, IrisResult};
use crate::vk_check;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ComputeParams {
    pub width: u32,
    pub height: u32,
    pub param_a: f32,
    pub param_b: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingPass {
    Enhance,
    Sharpen,
    Denoise,
}

pub struct ComputeInfra {
    context: Arc<VkContext>,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub enhance_pipeline: vk::Pipeline,
    pub sharpen_pipeline: vk::Pipeline,
    pub denoise_pipeline: vk::Pipeline,
    pub params_buffer: vk::Buffer,
    pub params_memory: vk::DeviceMemory,
    pub params_mapped: *mut u8,
}

impl ComputeInfra {
    pub fn new(context: Arc<VkContext>) -> IrisResult<Self> {
        unsafe {
            // Descriptor set layout: input sampled, output storage, params uniform
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            ];

            let descriptor_set_layout = vk_check!(
                context.device.create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                ),
                "vkCreateDescriptorSetLayout(compute)"
            )?;

            let pipeline_layout = vk_check!(
                context.device.create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(std::slice::from_ref(&descriptor_set_layout)),
                    None,
                ),
                "vkCreatePipelineLayout(compute)"
            )?;

            let enhance_pipeline = Self::create_compute_pipeline(
                &context,
                pipeline_layout,
                include_str!("../shaders/enhance.wgsl"),
            )?;

            let sharpen_pipeline = Self::create_compute_pipeline(
                &context,
                pipeline_layout,
                include_str!("../shaders/sharpen.wgsl"),
            )?;

            let denoise_pipeline = Self::create_compute_pipeline(
                &context,
                pipeline_layout,
                include_str!("../shaders/denoise.wgsl"),
            )?;

            // Params uniform buffer (16 bytes, persistently mapped)
            let params_size = std::mem::size_of::<ComputeParams>() as u64;

            let params_buffer = vk_check!(
                context.device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(params_size)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                ),
                "vkCreateBuffer(compute_params)"
            )?;

            let req = context.device.get_buffer_memory_requirements(params_buffer);
            let mem_idx = context
                .find_memory_type_index(
                    &req,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
                .ok_or(IrisError::NoMemoryType("compute params buffer"))?;

            let params_memory = vk_check!(
                context.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(mem_idx),
                    None,
                ),
                "vkAllocateMemory(compute_params)"
            )?;

            vk_check!(
                context
                    .device
                    .bind_buffer_memory(params_buffer, params_memory, 0),
                "vkBindBufferMemory(compute_params)"
            )?;

            let params_mapped = vk_check!(
                context.device.map_memory(
                    params_memory,
                    0,
                    params_size,
                    vk::MemoryMapFlags::empty(),
                ),
                "vkMapMemory(compute_params)"
            )? as *mut u8;

            Ok(Self {
                context,
                descriptor_set_layout,
                pipeline_layout,
                enhance_pipeline,
                sharpen_pipeline,
                denoise_pipeline,
                params_buffer,
                params_memory,
                params_mapped,
            })
        }
    }

    pub fn pipeline_for(&self, pass: ProcessingPass) -> vk::Pipeline {
        match pass {
            ProcessingPass::Enhance => self.enhance_pipeline,
            ProcessingPass::Sharpen => self.sharpen_pipeline,
            ProcessingPass::Denoise => self.denoise_pipeline,
        }
    }

    pub unsafe fn write_params(&self, params: &ComputeParams) {
        std::ptr::copy_nonoverlapping(
            params as *const ComputeParams as *const u8,
            self.params_mapped,
            std::mem::size_of::<ComputeParams>(),
        );
    }

    fn create_compute_pipeline(
        context: &VkContext,
        layout: vk::PipelineLayout,
        wgsl_source: &str,
    ) -> IrisResult<vk::Pipeline> {
        unsafe {
            let shader_module = shader::compile_wgsl(&context.device, wgsl_source);
            let entry_point = std::ffi::CString::new("cs_main").unwrap();

            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(shader_module)
                .name(&entry_point);

            let create_info = vk::ComputePipelineCreateInfo::default()
                .stage(stage)
                .layout(layout);

            let pipeline = context
                .device
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    std::slice::from_ref(&create_info),
                    None,
                )
                .map_err(|(_pipelines, code)| IrisError::Vk {
                    call: "vkCreateComputePipelines",
                    code,
                })?[0];

            context.device.destroy_shader_module(shader_module, None);
            Ok(pipeline)
        }
    }
}

impl Drop for ComputeInfra {
    fn drop(&mut self) {
        unsafe {
            self.context.device.unmap_memory(self.params_memory);
            self.context.device.destroy_buffer(self.params_buffer, None);
            self.context.device.free_memory(self.params_memory, None);
            self.context
                .device
                .destroy_pipeline(self.enhance_pipeline, None);
            self.context
                .device
                .destroy_pipeline(self.sharpen_pipeline, None);
            self.context
                .device
                .destroy_pipeline(self.denoise_pipeline, None);
            self.context
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.context
                .device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}
