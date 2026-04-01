use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::compute::{ComputeInfra, ComputeParams, ProcessingPass};
use super::context::VkContext;
use super::dmabuf::DmabufImage;
use super::pipeline::VkPipeline;
use crate::color::DynamicRange;
use crate::error::{IrisError, IrisResult};
use crate::viewport::camera::Camera;
use crate::vk_check;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    scale: [f32; 2],
    rotation: f32,
    zoom: f32,
    pan: [f32; 2],
    tone_map_enabled: f32,
    hdr_output_enabled: f32,
}

struct CachedTexture {
    image: vk::Image,
    image_view: vk::ImageView,
    memory: vk::DeviceMemory,
    descriptor_set: vk::DescriptorSet,
    dims: (u32, u32),
    memory_bytes: u64,
    dynamic_range: DynamicRange,
}

impl CachedTexture {
    unsafe fn destroy(&self, device: &ash::Device, pool: vk::DescriptorPool) {
        device
            .free_descriptor_sets(pool, std::slice::from_ref(&self.descriptor_set))
            .ok();
        device.destroy_image_view(self.image_view, None);
        device.destroy_image(self.image, None);
        device.free_memory(self.memory, None);
    }
}

fn compute_mip_levels(w: u32, h: u32) -> u32 {
    ((w.max(h) as f32).log2().floor() as u32 + 1).max(1)
}

unsafe fn mip_barrier(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    mip_level: u32,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags,
    dst_stage: vk::PipelineStageFlags,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(mip_level)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        )
        .src_access_mask(src_access)
        .dst_access_mask(dst_access);

    device.cmd_pipeline_barrier(
        cmd,
        src_stage,
        dst_stage,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        std::slice::from_ref(&barrier),
    );
}

struct ProcessingImage {
    image: vk::Image,
    image_view: vk::ImageView,
    memory: vk::DeviceMemory,
    width: u32,
    height: u32,
}

impl ProcessingImage {
    unsafe fn new(
        context: &VkContext,
        width: u32,
        height: u32,
        vk_format: vk::Format,
    ) -> IrisResult<Self> {
        let image = vk_check!(
            context.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk_format)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::STORAGE
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_SRC
                            | vk::ImageUsageFlags::TRANSFER_DST,
                    )
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            ),
            "vkCreateImage(processing)"
        )?;

        let req = context.device.get_image_memory_requirements(image);
        let mem_idx = context
            .find_memory_type_index(&req, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .ok_or(IrisError::NoMemoryType("processing image"))?;

        let memory = vk_check!(
            context.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(mem_idx),
                None,
            ),
            "vkAllocateMemory(processing)"
        )?;

        vk_check!(
            context.device.bind_image_memory(image, memory, 0),
            "vkBindImageMemory(processing)"
        )?;

        let image_view = vk_check!(
            context.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk_format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(1),
                    ),
                None,
            ),
            "vkCreateImageView(processing)"
        )?;

        {
            let cmd = context.begin_one_shot_commands()?;
            super::dmabuf::image_layout_transition(
                &context.device,
                cmd,
                image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_WRITE,
            );
            context.end_one_shot_commands(cmd)?;
        }

        Ok(Self {
            image,
            image_view,
            memory,
            width,
            height,
        })
    }

    unsafe fn destroy(&self, device: &ash::Device) {
        device.destroy_image_view(self.image_view, None);
        device.destroy_image(self.image, None);
        device.free_memory(self.memory, None);
    }
}

pub struct VkRenderer {
    context: Arc<VkContext>,
    pipeline: VkPipeline,

    descriptor_pool: vk::DescriptorPool,
    sampler: vk::Sampler,

    uniform_buffer: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    uniform_mapped: *mut u8,

    cache: HashMap<PathBuf, CachedTexture>,
    cache_order: Vec<PathBuf>,
    cache_memory_used: u64,
    cache_memory_budget: u64,

    active_path: Option<PathBuf>,

    render_targets: [DmabufImage; 2],
    framebuffers: [vk::Framebuffer; 2],
    command_buffers: [vk::CommandBuffer; 2],
    fences: [vk::Fence; 2],
    frame_index: usize,

    framebuffer_width: u32,
    framebuffer_height: u32,

    pub dirty: bool,
    pub image_dims: (f32, f32),
    pub tone_map_enabled: bool,
    last_sync_fd: Option<std::os::fd::RawFd>,

    pub vk_format: vk::Format,
    pub format_fourcc: u32,

    compute: Option<ComputeInfra>,
    processing_a: Option<ProcessingImage>,
    processing_b: Option<ProcessingImage>,
    compute_descriptor_pool: vk::DescriptorPool,
    pub active_passes: Vec<ProcessingPass>,
}

impl VkRenderer {
    pub fn new(
        context: Arc<VkContext>,
        width: u32,
        height: u32,
        vk_format: vk::Format,
        format_fourcc: u32,
    ) -> IrisResult<Self> {
        let width = width.max(1);
        let height = height.max(1);

        unsafe {
            let pipeline = VkPipeline::new(context.clone(), vk_format)?;

            let pool_sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: 256,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: 256,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: 256,
                },
            ];

            let descriptor_pool = vk_check!(
                context.device.create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(256)
                        .pool_sizes(&pool_sizes)
                        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET),
                    None,
                ),
                "vkCreateDescriptorPool"
            )?;

            let sampler = vk_check!(
                context.device.create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .min_lod(0.0)
                        .max_lod(16.0)
                        .unnormalized_coordinates(false),
                    None,
                ),
                "vkCreateSampler"
            )?;

            let uniform_size = std::mem::size_of::<Uniforms>() as u64;

            let uniform_buffer = vk_check!(
                context.device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(uniform_size)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                ),
                "vkCreateBuffer(uniform)"
            )?;

            let uniform_req = context
                .device
                .get_buffer_memory_requirements(uniform_buffer);

            let uniform_mem_idx = context
                .find_memory_type_index(
                    &uniform_req,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
                .ok_or(IrisError::NoMemoryType("uniform buffer"))?;

            let uniform_memory = vk_check!(
                context.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(uniform_req.size)
                        .memory_type_index(uniform_mem_idx),
                    None,
                ),
                "vkAllocateMemory(uniform)"
            )?;

            vk_check!(
                context
                    .device
                    .bind_buffer_memory(uniform_buffer, uniform_memory, 0),
                "vkBindBufferMemory(uniform)"
            )?;

            let uniform_mapped = vk_check!(
                context.device.map_memory(
                    uniform_memory,
                    0,
                    uniform_size,
                    vk::MemoryMapFlags::empty()
                ),
                "vkMapMemory(uniform)"
            )? as *mut u8;

            let blank_descriptor = create_blank_texture_descriptor(
                &context,
                descriptor_pool,
                pipeline.descriptor_set_layout,
                uniform_buffer,
                sampler,
                vk_format,
            )?;

            let rt0 = DmabufImage::new(context.clone(), width, height, vk_format, format_fourcc)?;
            let rt1 = DmabufImage::new(context.clone(), width, height, vk_format, format_fourcc)?;

            let fb0 = create_framebuffer(
                &context.device,
                pipeline.render_pass,
                rt0.render_image_view,
                width,
                height,
            )?;
            let fb1 = create_framebuffer(
                &context.device,
                pipeline.render_pass,
                rt1.render_image_view,
                width,
                height,
            )?;

            let cb0 = context.alloc_command_buffer()?;
            let cb1 = context.alloc_command_buffer()?;

            let make_fence = || {
                vk_check!(
                    context.device.create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    ),
                    "vkCreateFence(render)"
                )
            };
            let fence0 = make_fence()?;
            let fence1 = make_fence()?;

            let cache_memory_budget = context.vram_budget_bytes();
            println!(
                "[Iris] GPU cache budget: {} MiB",
                cache_memory_budget / (1024 * 1024)
            );

            let compute = match ComputeInfra::new(context.clone()) {
                Ok(c) => {
                    println!("[Iris] Compute shader infrastructure initialized");
                    Some(c)
                }
                Err(e) => {
                    eprintln!("[Iris] Compute shaders unavailable: {e}");
                    None
                }
            };

            let compute_pool_sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: 16,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: 16,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: 16,
                },
            ];

            let compute_descriptor_pool = vk_check!(
                context.device.create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(16)
                        .pool_sizes(&compute_pool_sizes)
                        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET),
                    None,
                ),
                "vkCreateDescriptorPool(compute)"
            )?;

            let mut renderer = Self {
                context,
                pipeline,
                descriptor_pool,
                sampler,
                uniform_buffer,
                uniform_memory,
                uniform_mapped,
                cache: HashMap::new(),
                cache_order: Vec::new(),
                cache_memory_used: 0,
                cache_memory_budget,
                active_path: None,
                render_targets: [rt0, rt1],
                framebuffers: [fb0, fb1],
                command_buffers: [cb0, cb1],
                fences: [fence0, fence1],
                frame_index: 0,
                framebuffer_width: width,
                framebuffer_height: height,
                dirty: true,
                image_dims: (1.0, 1.0),
                tone_map_enabled: false,
                last_sync_fd: None,
                vk_format,
                format_fourcc,
                compute,
                processing_a: None,
                processing_b: None,
                compute_descriptor_pool,
                active_passes: Vec::new(),
            };

            renderer
                .cache
                .insert(PathBuf::from("__blank__"), blank_descriptor);
            renderer.cache_order.push(PathBuf::from("__blank__"));
            renderer.active_path = Some(PathBuf::from("__blank__"));

            Ok(renderer)
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);

        if width == self.framebuffer_width && height == self.framebuffer_height {
            return;
        }

        unsafe {
            // Wait for any in-flight work to complete.
            // Do NOT reset fences — leave them signaled so the next
            // wait_fence() in render() returns immediately.
            // render()'s wait_fence already resets the fence it uses.
            let _ = self
                .context
                .device
                .wait_for_fences(&self.fences, true, u64::MAX);

            for i in 0..2 {
                self.context
                    .device
                    .destroy_framebuffer(self.framebuffers[i], None);

                match DmabufImage::new(
                    self.context.clone(),
                    width,
                    height,
                    self.vk_format,
                    self.format_fourcc,
                ) {
                    Ok(rt) => self.render_targets[i] = rt,
                    Err(e) => {
                        eprintln!("[Iris] resize DmabufImage failed: {e}");
                        return;
                    }
                }

                match create_framebuffer(
                    &self.context.device,
                    self.pipeline.render_pass,
                    self.render_targets[i].render_image_view,
                    width,
                    height,
                ) {
                    Ok(fb) => self.framebuffers[i] = fb,
                    Err(e) => {
                        eprintln!("[Iris] resize framebuffer failed: {e}");
                        return;
                    }
                }
            }

            if let Some(ref pi) = self.processing_a {
                pi.destroy(&self.context.device);
            }
            if let Some(ref pi) = self.processing_b {
                pi.destroy(&self.context.device);
            }
            if !self.active_passes.is_empty() {
                match (
                    ProcessingImage::new(&self.context, width, height, self.vk_format),
                    ProcessingImage::new(&self.context, width, height, self.vk_format),
                ) {
                    (Ok(a), Ok(b)) => {
                        self.processing_a = Some(a);
                        self.processing_b = Some(b);
                    }
                    _ => {
                        eprintln!("[Iris] resize processing images failed");
                        self.processing_a = None;
                        self.processing_b = None;
                    }
                }
            }

            self.framebuffer_width = width;
            self.framebuffer_height = height;
            // Reset frame index so the next render uses slot 0.
            // Both fences are signaled after wait_for_fences above,
            // so slot 0's fence will be immediately reset by wait_fence()
            // before submission, and slot 1 stays signaled until its turn.
            self.frame_index = 0;
            self.dirty = true;
        }
    }

    pub fn upload_and_activate(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) -> (u32, u32) {
        self.upload_texture(path, rgba, w, h);
        self.tone_map_enabled = false;
        self.activate(path);
        (w, h)
    }

    pub fn cache_only(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) {
        self.upload_texture(path, rgba, w, h);
    }

    pub fn upload_and_activate_16bit(
        &mut self,
        path: &Path,
        rgba16: &[u16],
        w: u32,
        h: u32,
    ) -> (u32, u32) {
        self.upload_texture_16bit(path, rgba16, w, h);
        self.tone_map_enabled = true;
        self.activate(path);
        (w, h)
    }

    pub fn cache_only_16bit(&mut self, path: &Path, rgba16: &[u16], w: u32, h: u32) {
        self.upload_texture_16bit(path, rgba16, w, h);
    }

    pub fn activate_cached(&mut self, path: &Path) -> Option<(f32, f32)> {
        if self.cache.contains_key(path) {
            self.activate(path);
            self.cache
                .get(path)
                .map(|c| (c.dims.0 as f32, c.dims.1 as f32))
        } else {
            None
        }
    }

    pub fn is_cached(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
    }

    pub fn toggle_pass(&mut self, pass: ProcessingPass) {
        if let Some(pos) = self.active_passes.iter().position(|p| *p == pass) {
            self.active_passes.remove(pos);
        } else {
            self.active_passes.push(pass);
        }

        unsafe {
            if !self.active_passes.is_empty() && self.processing_a.is_none() {
                let w = self.framebuffer_width;
                let h = self.framebuffer_height;
                match (
                    ProcessingImage::new(&self.context, w, h, self.vk_format),
                    ProcessingImage::new(&self.context, w, h, self.vk_format),
                ) {
                    (Ok(a), Ok(b)) => {
                        self.processing_a = Some(a);
                        self.processing_b = Some(b);
                    }
                    _ => eprintln!("[Iris] processing image alloc failed"),
                }
            } else if self.active_passes.is_empty() {
                if let Some(ref pi) = self.processing_a {
                    pi.destroy(&self.context.device);
                }
                if let Some(ref pi) = self.processing_b {
                    pi.destroy(&self.context.device);
                }
                self.processing_a = None;
                self.processing_b = None;
            }
        }

        self.dirty = true;
    }

    pub fn has_pass(&self, pass: ProcessingPass) -> bool {
        self.active_passes.contains(&pass)
    }

    pub fn render(&mut self, camera: &Camera) {
        if !self.dirty {
            return;
        }

        let active_path = match &self.active_path {
            Some(p) => p.clone(),
            None => return,
        };

        // Don't render the blank placeholder during resize — it causes
        // a fence stall because the compositor may still hold the previous
        // 1×1 DMA-BUF from the initial blank render.
        if active_path == PathBuf::from("__blank__") && self.framebuffer_width > 1 {
            return;
        }

        let descriptor_set = match self.cache.get(&active_path) {
            Some(c) => c.descriptor_set,
            None => return,
        };

        let cur = self.frame_index % 2;

        let result: IrisResult<()> = (|| unsafe {
            self.wait_fence(cur)?;
            self.write_uniforms(camera);
            self.record_and_submit(descriptor_set, cur)?;

            if !self.active_passes.is_empty() {
                self.run_compute_passes(cur)?;
            } else {
                match self.render_targets[cur].blit_render_to_export_async() {
                    Ok(fd) => self.last_sync_fd = Some(fd),
                    Err(e) => eprintln!("[Iris] blit_render_to_export_async: {e}"),
                }
            }
            Ok(())
        })();

        if let Err(e) = result {
            eprintln!("[Iris] render error: {e}");
        }

        self.frame_index = self.frame_index.wrapping_add(1);
        self.dirty = false;
    }

    pub fn take_sync_fd(&mut self) -> Option<std::os::fd::RawFd> {
        self.last_sync_fd.take()
    }

    pub fn export_fd_for_gtk(&self) -> Option<std::os::fd::RawFd> {
        self.render_targets[self.presented_slot()]
            .export_fd_for_gtk()
            .ok()
    }

    pub fn render_target_stride(&self) -> u32 {
        self.render_targets[self.presented_slot()].stride
    }

    pub fn render_target_fourcc(&self) -> u32 {
        self.render_targets[self.presented_slot()].format_fourcc
    }

    pub fn render_target_width(&self) -> u32 {
        self.framebuffer_width
    }

    pub fn render_target_height(&self) -> u32 {
        self.framebuffer_height
    }

    pub fn read_pixels(&self) -> Option<Vec<u8>> {
        self.render_targets[self.presented_slot()]
            .read_pixels()
            .ok()
    }

    fn presented_slot(&self) -> usize {
        self.frame_index.wrapping_sub(1) % 2
    }

    fn activate(&mut self, path: &Path) {
        if let Some(c) = self.cache.get(path) {
            self.image_dims = (c.dims.0 as f32, c.dims.1 as f32);
            self.tone_map_enabled = matches!(c.dynamic_range, DynamicRange::Hdr);
            self.active_path = Some(path.to_owned());
            self.dirty = true;

            self.cache_order.retain(|p| p != path);
            self.cache_order.insert(0, path.to_owned());
        }
    }

    unsafe fn wait_fence(&self, slot: usize) -> IrisResult<()> {
        vk_check!(
            self.context.device.wait_for_fences(
                std::slice::from_ref(&self.fences[slot]),
                true,
                u64::MAX
            ),
            "vkWaitForFences(render)"
        )?;
        vk_check!(
            self.context
                .device
                .reset_fences(std::slice::from_ref(&self.fences[slot])),
            "vkResetFences(render)"
        )?;
        Ok(())
    }

    unsafe fn write_uniforms(&self, camera: &Camera) {
        let scale = camera.fit_scale(self.image_dims.0, self.image_dims.1);
        let uniforms = Uniforms {
            scale,
            rotation: camera.rotation,
            zoom: camera.zoom,
            pan: [camera.position.x, camera.position.y],
            tone_map_enabled: if self.tone_map_enabled { 1.0 } else { 0.0 },
            hdr_output_enabled: 0.0,
        };
        std::ptr::copy_nonoverlapping(
            &uniforms as *const Uniforms as *const u8,
            self.uniform_mapped,
            std::mem::size_of::<Uniforms>(),
        );
    }

    unsafe fn record_and_submit(
        &self,
        descriptor_set: vk::DescriptorSet,
        slot: usize,
    ) -> IrisResult<()> {
        let cmd = self.command_buffers[slot];

        self.context
            .device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|c| IrisError::Vk {
                call: "vkBeginCommandBuffer",
                code: c,
            })?;

        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.051, 0.051, 0.051, 1.0],
            },
        }];

        let render_pass_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.pipeline.render_pass)
            .framebuffer(self.framebuffers[slot])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: self.framebuffer_width,
                    height: self.framebuffer_height,
                },
            })
            .clear_values(&clear_values);

        self.context.device.cmd_begin_render_pass(
            cmd,
            &render_pass_begin,
            vk::SubpassContents::INLINE,
        );

        self.context.device.cmd_bind_pipeline(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.pipeline.pipeline,
        );

        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.framebuffer_width as f32,
            height: self.framebuffer_height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: self.framebuffer_width,
                height: self.framebuffer_height,
            },
        };

        self.context
            .device
            .cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
        self.context
            .device
            .cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));

        self.context.device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            self.pipeline.pipeline_layout,
            0,
            std::slice::from_ref(&descriptor_set),
            &[],
        );

        self.context.device.cmd_draw(cmd, 6, 1, 0, 0);
        self.context.device.cmd_end_render_pass(cmd);

        self.context
            .device
            .end_command_buffer(cmd)
            .map_err(|c| IrisError::Vk {
                call: "vkEndCommandBuffer",
                code: c,
            })?;

        let submit_info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
        self.context
            .device
            .queue_submit(
                self.context.queue,
                std::slice::from_ref(&submit_info),
                self.fences[slot],
            )
            .map_err(|c| IrisError::Vk {
                call: "vkQueueSubmit",
                code: c,
            })?;

        Ok(())
    }

    unsafe fn run_compute_passes(&mut self, slot: usize) -> IrisResult<()> {
        let compute = match &self.compute {
            Some(c) => c,
            None => {
                match self.render_targets[slot].blit_render_to_export_async() {
                    Ok(fd) => self.last_sync_fd = Some(fd),
                    Err(e) => eprintln!("[Iris] blit fallback: {e}"),
                }
                return Ok(());
            }
        };

        let (pi_a, pi_b) = match (&self.processing_a, &self.processing_b) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                match self.render_targets[slot].blit_render_to_export_async() {
                    Ok(fd) => self.last_sync_fd = Some(fd),
                    Err(e) => eprintln!("[Iris] blit fallback: {e}"),
                }
                return Ok(());
            }
        };

        vk_check!(
            self.context.device.wait_for_fences(
                std::slice::from_ref(&self.fences[slot]),
                true,
                u64::MAX,
            ),
            "vkWaitForFences(pre-compute)"
        )?;

        let render_image = self.render_targets[slot].render_image;
        let render_view = self.render_targets[slot].render_image_view;

        let cmd = self.context.begin_one_shot_commands()?;

        super::dmabuf::image_layout_transition(
            &self.context.device,
            cmd,
            render_image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::TRANSFER_READ,
            vk::AccessFlags::SHADER_READ,
        );

        super::dmabuf::image_layout_transition(
            &self.context.device,
            cmd,
            pi_a.image,
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::GENERAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::SHADER_WRITE,
        );
        super::dmabuf::image_layout_transition(
            &self.context.device,
            cmd,
            pi_b.image,
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::GENERAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::SHADER_WRITE,
        );

        let passes = self.active_passes.clone();
        let targets = [pi_a, pi_b];
        let mut current_input_view = render_view;
        let mut current_input_image = render_image;
        let mut current_input_is_render = true;
        let mut last_output_idx: usize = 0;

        for (i, pass) in passes.iter().enumerate() {
            let output_idx = i % 2;
            let output_target = targets[output_idx];

            let params = match pass {
                ProcessingPass::Enhance => ComputeParams {
                    width: self.framebuffer_width,
                    height: self.framebuffer_height,
                    param_a: 0.05,
                    param_b: 0.95,
                },
                ProcessingPass::Sharpen => ComputeParams {
                    width: self.framebuffer_width,
                    height: self.framebuffer_height,
                    param_a: 1.5,
                    param_b: 0.0,
                },
                ProcessingPass::Denoise => ComputeParams {
                    width: self.framebuffer_width,
                    height: self.framebuffer_height,
                    param_a: 3.0,
                    param_b: 0.1,
                },
            };

            compute.write_params(&params);

            if !current_input_is_render {
                super::dmabuf::image_layout_transition(
                    &self.context.device,
                    cmd,
                    current_input_image,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                );
            }

            let desc_set = self
                .context
                .device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.compute_descriptor_pool)
                        .set_layouts(std::slice::from_ref(&compute.descriptor_set_layout)),
                )
                .map_err(|c| IrisError::Upload {
                    stage: "compute descriptor set",
                    code: c,
                })?[0];

            let input_info = vk::DescriptorImageInfo::default()
                .image_view(current_input_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

            let output_info = vk::DescriptorImageInfo::default()
                .image_view(output_target.image_view)
                .image_layout(vk::ImageLayout::GENERAL);

            let params_info = vk::DescriptorBufferInfo::default()
                .buffer(compute.params_buffer)
                .offset(0)
                .range(std::mem::size_of::<ComputeParams>() as u64);

            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(desc_set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(std::slice::from_ref(&input_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(desc_set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&output_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(desc_set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(std::slice::from_ref(&params_info)),
            ];
            self.context.device.update_descriptor_sets(&writes, &[]);

            let compute_pipeline = compute.pipeline_for(*pass);
            self.context.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                compute_pipeline,
            );
            self.context.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                compute.pipeline_layout,
                0,
                std::slice::from_ref(&desc_set),
                &[],
            );

            let groups_x = (self.framebuffer_width + 15) / 16;
            let groups_y = (self.framebuffer_height + 15) / 16;
            self.context.device.cmd_dispatch(cmd, groups_x, groups_y, 1);

            current_input_view = output_target.image_view;
            current_input_image = output_target.image;
            current_input_is_render = false;
            last_output_idx = output_idx;
        }

        let final_output = targets[last_output_idx];

        super::dmabuf::image_layout_transition(
            &self.context.device,
            cmd,
            final_output.image,
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::SHADER_WRITE,
            vk::AccessFlags::TRANSFER_READ,
        );

        super::dmabuf::image_layout_transition(
            &self.context.device,
            cmd,
            render_image,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags::SHADER_READ,
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        );

        self.context.end_one_shot_commands(cmd)?;

        vk_check!(
            self.context.device.reset_descriptor_pool(
                self.compute_descriptor_pool,
                vk::DescriptorPoolResetFlags::empty(),
            ),
            "vkResetDescriptorPool(compute)"
        )?;

        match self.render_targets[slot].blit_external_to_export_async(final_output.image) {
            Ok(fd) => self.last_sync_fd = Some(fd),
            Err(e) => eprintln!("[Iris] blit_external_to_export_async: {e}"),
        }

        {
            let cmd = self.context.begin_one_shot_commands()?;
            super::dmabuf::image_layout_transition(
                &self.context.device,
                cmd,
                pi_a.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::SHADER_WRITE,
            );
            super::dmabuf::image_layout_transition(
                &self.context.device,
                cmd,
                pi_b.image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_WRITE,
            );
            self.context.end_one_shot_commands(cmd)?;
        }

        Ok(())
    }

    fn upload_texture(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) {
        if let Some(old) = self.cache.remove(path) {
            unsafe { old.destroy(&self.context.device, self.descriptor_pool) };
            self.cache_memory_used = self.cache_memory_used.saturating_sub(old.memory_bytes);
            self.cache_order.retain(|p| p != path);
        }

        let max_dim = self.context.device_limits.max_image_dimension2_d;
        let (w, h, owned): (u32, u32, Cow<[u8]>) = if w > max_dim || h > max_dim {
            let scale = max_dim as f32 / w.max(h) as f32;
            let new_w = ((w as f32 * scale) as u32).max(1);
            let new_h = ((h as f32 * scale) as u32).max(1);
            eprintln!(
                "[Iris] Image {}×{} exceeds GPU limit (max {}); downscaling to {}×{}",
                w, h, max_dim, new_w, new_h
            );
            let Some(src) = image::RgbaImage::from_raw(w, h, rgba.to_vec()) else {
                eprintln!("[Iris] Downscale failed: RGBA buffer length mismatch");
                return;
            };
            let dst =
                image::imageops::resize(&src, new_w, new_h, image::imageops::FilterType::Lanczos3);
            (new_w, new_h, Cow::Owned(dst.into_raw()))
        } else {
            (w, h, Cow::Borrowed(rgba))
        };
        let rgba: &[u8] = &owned;

        let mem = (w as u64) * (h as u64) * 4;

        while self.cache_memory_used + mem > self.cache_memory_budget {
            let oldest = match self.cache_order.last().cloned() {
                Some(p) if p != PathBuf::from("__blank__") => p,
                _ => break,
            };
            if let Some(evicted) = self.cache.remove(&oldest) {
                unsafe { evicted.destroy(&self.context.device, self.descriptor_pool) };
                self.cache_memory_used =
                    self.cache_memory_used.saturating_sub(evicted.memory_bytes);
                self.cache_order.pop();
            }
        }

        unsafe {
            match upload_rgba_texture(
                &self.context,
                self.descriptor_pool,
                self.pipeline.descriptor_set_layout,
                self.uniform_buffer,
                self.sampler,
                rgba,
                w,
                h,
                self.vk_format,
            ) {
                Ok(cached) => {
                    self.cache_memory_used += mem;
                    self.cache_order.insert(0, path.to_owned());
                    self.cache.insert(path.to_owned(), cached);
                }
                Err(e) => eprintln!("[Iris] upload_texture failed: {e}"),
            }
        }
    }

    fn upload_texture_16bit(&mut self, path: &Path, rgba16: &[u16], w: u32, h: u32) {
        if let Some(old) = self.cache.remove(path) {
            unsafe { old.destroy(&self.context.device, self.descriptor_pool) };
            self.cache_memory_used = self.cache_memory_used.saturating_sub(old.memory_bytes);
            self.cache_order.retain(|p| p != path);
        }

        let max_dim = self.context.device_limits.max_image_dimension2_d;
        let (w, h, owned): (u32, u32, Cow<[u16]>) = if w > max_dim || h > max_dim {
            let scale = max_dim as f32 / w.max(h) as f32;
            let new_w = ((w as f32 * scale) as u32).max(1);
            let new_h = ((h as f32 * scale) as u32).max(1);
            eprintln!(
                "[Iris] RAW {}×{} exceeds GPU limit (max {}); downscaling to {}×{}",
                w, h, max_dim, new_w, new_h
            );

            let Some(src) =
                image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(w, h, rgba16.to_vec())
            else {
                eprintln!("[Iris] RAW downscale failed: buffer length mismatch");
                return;
            };

            let dst =
                image::imageops::resize(&src, new_w, new_h, image::imageops::FilterType::Lanczos3);
            (new_w, new_h, Cow::Owned(dst.into_raw()))
        } else {
            (w, h, Cow::Borrowed(rgba16))
        };
        let rgba16: &[u16] = &owned;

        let mem = (w as u64) * (h as u64) * 8;

        while self.cache_memory_used + mem > self.cache_memory_budget {
            let oldest = match self.cache_order.last().cloned() {
                Some(p) if p != PathBuf::from("__blank__") => p,
                _ => break,
            };
            if let Some(evicted) = self.cache.remove(&oldest) {
                unsafe { evicted.destroy(&self.context.device, self.descriptor_pool) };
                self.cache_memory_used =
                    self.cache_memory_used.saturating_sub(evicted.memory_bytes);
                self.cache_order.pop();
            }
        }

        unsafe {
            match upload_rgba16_texture(
                &self.context,
                self.descriptor_pool,
                self.pipeline.descriptor_set_layout,
                self.uniform_buffer,
                self.sampler,
                rgba16,
                w,
                h,
            ) {
                Ok(cached) => {
                    self.cache_memory_used += mem;
                    self.cache_order.insert(0, path.to_owned());
                    self.cache.insert(path.to_owned(), cached);
                }
                Err(e) => eprintln!("[Iris] upload_texture_16bit failed: {e}"),
            }
        }
    }
}

impl Drop for VkRenderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .context
                .device
                .wait_for_fences(&self.fences, true, u64::MAX);

            for (_, tex) in self.cache.drain() {
                tex.destroy(&self.context.device, self.descriptor_pool);
            }

            if let Some(ref pi) = self.processing_a {
                pi.destroy(&self.context.device);
            }
            if let Some(ref pi) = self.processing_b {
                pi.destroy(&self.context.device);
            }

            for i in 0..2 {
                self.context.device.destroy_fence(self.fences[i], None);
                self.context.device.free_command_buffers(
                    self.context.command_pool,
                    std::slice::from_ref(&self.command_buffers[i]),
                );
                self.context
                    .device
                    .destroy_framebuffer(self.framebuffers[i], None);
            }

            self.context.device.unmap_memory(self.uniform_memory);
            self.context
                .device
                .destroy_buffer(self.uniform_buffer, None);
            self.context.device.free_memory(self.uniform_memory, None);
            self.context.device.destroy_sampler(self.sampler, None);
            self.context
                .device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.context
                .device
                .destroy_descriptor_pool(self.compute_descriptor_pool, None);
        }
    }
}

unsafe fn create_framebuffer(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    image_view: vk::ImageView,
    width: u32,
    height: u32,
) -> IrisResult<vk::Framebuffer> {
    vk_check!(
        device.create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(std::slice::from_ref(&image_view))
                .width(width)
                .height(height)
                .layers(1),
            None,
        ),
        "vkCreateFramebuffer"
    )
}

unsafe fn upload_rgba_texture(
    context: &VkContext,
    descriptor_pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    sampler: vk::Sampler,
    rgba: &[u8],
    w: u32,
    h: u32,
    vk_format: vk::Format,
) -> IrisResult<CachedTexture> {
    let mip_levels = compute_mip_levels(w, h);
    let data_size = (w as u64) * (h as u64) * 4;

    let staging_buffer = context
        .device
        .create_buffer(
            &vk::BufferCreateInfo::default()
                .size(data_size)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer create",
            code: c,
        })?;

    let staging_req = context
        .device
        .get_buffer_memory_requirements(staging_buffer);
    let staging_mem_idx = context
        .find_memory_type_index(
            &staging_req,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or(IrisError::NoMemoryType("staging buffer"))?;

    let staging_memory = context
        .device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(staging_req.size)
                .memory_type_index(staging_mem_idx),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer alloc",
            code: c,
        })?;

    context
        .device
        .bind_buffer_memory(staging_buffer, staging_memory, 0)
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer bind",
            code: c,
        })?;

    let ptr = context
        .device
        .map_memory(staging_memory, 0, data_size, vk::MemoryMapFlags::empty())
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer map",
            code: c,
        })? as *mut u8;
    std::ptr::copy_nonoverlapping(rgba.as_ptr(), ptr, rgba.len());
    context.device.unmap_memory(staging_memory);

    let image = context
        .device
        .create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk_format)
                .extent(vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                })
                .mip_levels(mip_levels)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::SAMPLED,
                )
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "texture image create",
            code: c,
        })?;

    let tex_req = context.device.get_image_memory_requirements(image);
    let tex_mem_idx = context
        .find_memory_type_index(&tex_req, vk::MemoryPropertyFlags::DEVICE_LOCAL)
        .ok_or(IrisError::NoMemoryType("texture image"))?;

    let memory = context
        .device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(tex_req.size)
                .memory_type_index(tex_mem_idx),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "texture alloc",
            code: c,
        })?;

    context
        .device
        .bind_image_memory(image, memory, 0)
        .map_err(|c| IrisError::Upload {
            stage: "texture bind",
            code: c,
        })?;

    {
        let cmd = context.begin_one_shot_commands()?;
        mip_barrier(
            &context.device,
            cmd,
            image,
            0,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::TRANSFER_WRITE,
        );

        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            });

        context.device.cmd_copy_buffer_to_image(
            cmd,
            staging_buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            std::slice::from_ref(&region),
        );

        let mut mip_w = w as i32;
        let mut mip_h = h as i32;
        for i in 1..mip_levels {
            mip_barrier(
                &context.device,
                cmd,
                image,
                i - 1,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::TRANSFER_READ,
            );
            mip_barrier(
                &context.device,
                cmd,
                image,
                i,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            );

            let next_w = (mip_w / 2).max(1);
            let next_h = (mip_h / 2).max(1);
            let blit = vk::ImageBlit::default()
                .src_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(i - 1)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: mip_w,
                        y: mip_h,
                        z: 1,
                    },
                ])
                .dst_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(i)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: next_w,
                        y: next_h,
                        z: 1,
                    },
                ]);

            context.device.cmd_blit_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                std::slice::from_ref(&blit),
                vk::Filter::LINEAR,
            );

            mip_barrier(
                &context.device,
                cmd,
                image,
                i - 1,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::SHADER_READ,
            );
            mip_w = next_w;
            mip_h = next_h;
        }

        mip_barrier(
            &context.device,
            cmd,
            image,
            mip_levels - 1,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::SHADER_READ,
        );
        context.end_one_shot_commands(cmd)?;
    }

    context.device.destroy_buffer(staging_buffer, None);
    context.device.free_memory(staging_memory, None);

    let image_view = context
        .device
        .create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk_format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(mip_levels)
                        .base_array_layer(0)
                        .layer_count(1),
                ),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "texture image view",
            code: c,
        })?;

    let descriptor_set = allocate_descriptor_set(
        &context.device,
        descriptor_pool,
        layout,
        uniform_buffer,
        image_view,
        sampler,
    )?;

    Ok(CachedTexture {
        image,
        image_view,
        memory,
        descriptor_set,
        dims: (w, h),
        memory_bytes: (w as u64) * (h as u64) * 4,
        dynamic_range: DynamicRange::Sdr,
    })
}

unsafe fn upload_rgba16_texture(
    context: &VkContext,
    descriptor_pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    sampler: vk::Sampler,
    rgba16: &[u16],
    w: u32,
    h: u32,
) -> IrisResult<CachedTexture> {
    let mip_levels = compute_mip_levels(w, h);
    let data_size = (w as u64) * (h as u64) * 8;

    let staging_buffer = context
        .device
        .create_buffer(
            &vk::BufferCreateInfo::default()
                .size(data_size)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer create (16bit)",
            code: c,
        })?;

    let staging_req = context
        .device
        .get_buffer_memory_requirements(staging_buffer);
    let staging_mem_idx = context
        .find_memory_type_index(
            &staging_req,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or(IrisError::NoMemoryType("staging buffer (16bit)"))?;

    let staging_memory = context
        .device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(staging_req.size)
                .memory_type_index(staging_mem_idx),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer alloc (16bit)",
            code: c,
        })?;

    context
        .device
        .bind_buffer_memory(staging_buffer, staging_memory, 0)
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer bind (16bit)",
            code: c,
        })?;

    let ptr = context
        .device
        .map_memory(staging_memory, 0, data_size, vk::MemoryMapFlags::empty())
        .map_err(|c| IrisError::Upload {
            stage: "staging buffer map (16bit)",
            code: c,
        })? as *mut u8;
    let raw_bytes: &[u8] = bytemuck::cast_slice(rgba16);
    std::ptr::copy_nonoverlapping(raw_bytes.as_ptr(), ptr, raw_bytes.len());
    context.device.unmap_memory(staging_memory);

    let image = context
        .device
        .create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R16G16B16A16_UNORM)
                .extent(vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                })
                .mip_levels(mip_levels)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::SAMPLED,
                )
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "texture image create (16bit)",
            code: c,
        })?;

    let tex_req = context.device.get_image_memory_requirements(image);
    let tex_mem_idx = context
        .find_memory_type_index(&tex_req, vk::MemoryPropertyFlags::DEVICE_LOCAL)
        .ok_or(IrisError::NoMemoryType("texture image (16bit)"))?;

    let memory = context
        .device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(tex_req.size)
                .memory_type_index(tex_mem_idx),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "texture alloc (16bit)",
            code: c,
        })?;

    context
        .device
        .bind_image_memory(image, memory, 0)
        .map_err(|c| IrisError::Upload {
            stage: "texture bind (16bit)",
            code: c,
        })?;

    {
        let cmd = context.begin_one_shot_commands()?;
        mip_barrier(
            &context.device,
            cmd,
            image,
            0,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::TRANSFER_WRITE,
        );

        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            });

        context.device.cmd_copy_buffer_to_image(
            cmd,
            staging_buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            std::slice::from_ref(&region),
        );

        let mut mip_w = w as i32;
        let mut mip_h = h as i32;
        for i in 1..mip_levels {
            mip_barrier(
                &context.device,
                cmd,
                image,
                i - 1,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::TRANSFER_READ,
            );
            mip_barrier(
                &context.device,
                cmd,
                image,
                i,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            );

            let next_w = (mip_w / 2).max(1);
            let next_h = (mip_h / 2).max(1);
            let blit = vk::ImageBlit::default()
                .src_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(i - 1)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: mip_w,
                        y: mip_h,
                        z: 1,
                    },
                ])
                .dst_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(i)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: next_w,
                        y: next_h,
                        z: 1,
                    },
                ]);

            context.device.cmd_blit_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                std::slice::from_ref(&blit),
                vk::Filter::LINEAR,
            );

            mip_barrier(
                &context.device,
                cmd,
                image,
                i - 1,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::SHADER_READ,
            );
            mip_w = next_w;
            mip_h = next_h;
        }

        mip_barrier(
            &context.device,
            cmd,
            image,
            mip_levels - 1,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::SHADER_READ,
        );
        context.end_one_shot_commands(cmd)?;
    }

    context.device.destroy_buffer(staging_buffer, None);
    context.device.free_memory(staging_memory, None);

    let image_view = context
        .device
        .create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R16G16B16A16_UNORM)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(mip_levels)
                        .base_array_layer(0)
                        .layer_count(1),
                ),
            None,
        )
        .map_err(|c| IrisError::Upload {
            stage: "texture image view (16bit)",
            code: c,
        })?;

    let descriptor_set = allocate_descriptor_set(
        &context.device,
        descriptor_pool,
        layout,
        uniform_buffer,
        image_view,
        sampler,
    )?;

    Ok(CachedTexture {
        image,
        image_view,
        memory,
        descriptor_set,
        dims: (w, h),
        memory_bytes: (w as u64) * (h as u64) * 8,
        dynamic_range: DynamicRange::Hdr,
    })
}

unsafe fn create_blank_texture_descriptor(
    context: &VkContext,
    descriptor_pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    sampler: vk::Sampler,
    vk_format: vk::Format,
) -> IrisResult<CachedTexture> {
    let blank_rgba = [0u8, 0, 0, 255];
    upload_rgba_texture(
        context,
        descriptor_pool,
        layout,
        uniform_buffer,
        sampler,
        &blank_rgba,
        1,
        1,
        vk_format,
    )
}

unsafe fn allocate_descriptor_set(
    device: &ash::Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    image_view: vk::ImageView,
    sampler: vk::Sampler,
) -> IrisResult<vk::DescriptorSet> {
    let descriptor_set = device
        .allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(std::slice::from_ref(&layout)),
        )
        .map_err(|c| IrisError::Upload {
            stage: "descriptor set alloc",
            code: c,
        })?[0];

    let uniform_buffer_info = vk::DescriptorBufferInfo::default()
        .buffer(uniform_buffer)
        .offset(0)
        .range(std::mem::size_of::<Uniforms>() as u64);
    let image_info = vk::DescriptorImageInfo::default()
        .image_view(image_view)
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
    let sampler_info = vk::DescriptorImageInfo::default().sampler(sampler);

    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(std::slice::from_ref(&uniform_buffer_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&image_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(std::slice::from_ref(&sampler_info)),
    ];
    device.update_descriptor_sets(&writes, &[]);

    Ok(descriptor_set)
}
