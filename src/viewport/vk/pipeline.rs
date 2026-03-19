use super::context::VkContext;
use super::shader;
use ash::vk;
use std::sync::Arc;

use crate::error::{IrisError, IrisResult};
use crate::vk_check;

pub struct VkPipeline {
    pub context: Arc<VkContext>,
    pub render_pass: vk::RenderPass,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
}

impl VkPipeline {
    /// Create the full graphics pipeline.
    ///
    /// `color_format` must match the `vk::Format` used for the `DmabufImage`
    /// render targets — this is the negotiated format from `Viewport::new()`.
    pub fn new(context: Arc<VkContext>, color_format: vk::Format) -> IrisResult<Self> {
        unsafe {
            // 1. Descriptor Set Layout
            let bindings = [
                // Binding 0: Uniform Buffer
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
                // Binding 1: Image Texture
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                // Binding 2: Sampler
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];

            let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
            let descriptor_set_layout = vk_check!(
                context
                    .device
                    .create_descriptor_set_layout(&layout_info, None),
                "vkCreateDescriptorSetLayout"
            )?;

            // 2. Pipeline Layout
            let layouts = [descriptor_set_layout];
            let pipeline_layout_info =
                vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
            let pipeline_layout = vk_check!(
                context
                    .device
                    .create_pipeline_layout(&pipeline_layout_info, None),
                "vkCreatePipelineLayout"
            )?;

            // 3. Render Pass — uses the negotiated format, not hardcoded
            let color_attachment = vk::AttachmentDescription::default()
                .format(color_format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);

            let color_attachment_ref = vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

            let subpass = vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(std::slice::from_ref(&color_attachment_ref));

            let render_pass_info = vk::RenderPassCreateInfo::default()
                .attachments(std::slice::from_ref(&color_attachment))
                .subpasses(std::slice::from_ref(&subpass));

            let render_pass = vk_check!(
                context.device.create_render_pass(&render_pass_info, None),
                "vkCreateRenderPass"
            )?;

            // 4. Compile Shaders
            let shader_src = include_str!("../shaders/image.wgsl");
            let shader_module = shader::compile_wgsl(&context.device, shader_src);

            let entry_point_vs = std::ffi::CString::new("vs_main").unwrap();
            let entry_point_fs = std::ffi::CString::new("fs_main").unwrap();

            let shader_stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(shader_module)
                    .name(&entry_point_vs),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(shader_module)
                    .name(&entry_point_fs),
            ];

            // 5. Fixed-Function State
            let vertex_input_info = vk::PipelineVertexInputStateCreateInfo::default();

            let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic_state =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

            let viewport_state = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);

            let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);

            let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);

            let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD);

            let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(std::slice::from_ref(&color_blend_attachment));

            // 6. Bake the Pipeline
            let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&shader_stages)
                .vertex_input_state(&vertex_input_info)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterizer)
                .multisample_state(&multisampling)
                .color_blend_state(&color_blending)
                .dynamic_state(&dynamic_state)
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0);

            let pipeline = context
                .device
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    std::slice::from_ref(&pipeline_info),
                    None,
                )
                .map_err(|(_pipelines, code)| IrisError::Vk {
                    call: "vkCreateGraphicsPipelines",
                    code,
                })?[0];

            // Clean up shader module (driver retains its own compiled form)
            context.device.destroy_shader_module(shader_module, None);

            Ok(Self {
                context,
                render_pass,
                descriptor_set_layout,
                pipeline_layout,
                pipeline,
            })
        }
    }
}

impl Drop for VkPipeline {
    fn drop(&mut self) {
        unsafe {
            self.context.device.destroy_pipeline(self.pipeline, None);
            self.context
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.context
                .device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            self.context
                .device
                .destroy_render_pass(self.render_pass, None);
        }
    }
}
