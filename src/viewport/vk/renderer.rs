use ash::vk;
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::context::VkContext;
use super::dmabuf::DmabufImage;
use super::pipeline::VkPipeline;
use crate::viewport::camera::Camera;

// ── Uniform buffer layout (must match image.wgsl) ────────────────────────────

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    scale: [f32; 2],
    rotation: f32,
    zoom: f32,
    pan: [f32; 2],
    _padding: [f32; 2],
}

// ── Per-image GPU texture ─────────────────────────────────────────────────────

struct CachedTexture {
    image: vk::Image,
    image_view: vk::ImageView,
    memory: vk::DeviceMemory,
    descriptor_set: vk::DescriptorSet,
    dims: (u32, u32),
    memory_bytes: u64,
}

impl CachedTexture {
    unsafe fn destroy(&self, device: &ash::Device) {
        device.destroy_image_view(self.image_view, None);
        device.destroy_image(self.image, None);
        device.free_memory(self.memory, None);
        // descriptor_set is freed with the pool — nothing to do here
    }
}

// ── VkRenderer ───────────────────────────────────────────────────────────────

pub struct VkRenderer {
    context: Arc<VkContext>,
    pipeline: VkPipeline,

    // Descriptor infrastructure
    descriptor_pool: vk::DescriptorPool,

    // Sampler shared across all textures
    sampler: vk::Sampler,

    // Uniform buffer (one, updated every frame)
    uniform_buffer: vk::Buffer,
    uniform_memory: vk::DeviceMemory,
    uniform_mapped: *mut u8,

    // Per-image texture cache
    cache: HashMap<PathBuf, CachedTexture>,
    cache_order: Vec<PathBuf>,
    cache_memory_used: u64,
    cache_memory_budget: u64,

    // Currently active texture path
    active_path: Option<PathBuf>,

    // Framebuffer (rebuilt when render target size changes)
    framebuffer: vk::Framebuffer,
    framebuffer_width: u32,
    framebuffer_height: u32,

    // Per-frame command buffer and fence
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,

    // The render target we draw into each frame
    pub render_target: DmabufImage,

    pub dirty: bool,
    pub image_dims: (f32, f32),
}

impl VkRenderer {
    pub fn new(context: Arc<VkContext>, width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);

        unsafe {
            let pipeline = VkPipeline::new(context.clone());

            // ── Descriptor pool ──────────────────────────────────────────────
            // We allocate one descriptor set per cached image plus one for the
            // default (blank) texture.  Start with capacity for 64 images.
            let pool_sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: 64,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: 64,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLER,
                    descriptor_count: 64,
                },
            ];

            let descriptor_pool = context
                .device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(64)
                        .pool_sizes(&pool_sizes)
                        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET),
                    None,
                )
                .expect("Failed to create descriptor pool");

            // ── Sampler ──────────────────────────────────────────────────────
            let sampler = context
                .device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .min_lod(0.0)
                        .max_lod(0.0)
                        .unnormalized_coordinates(false),
                    None,
                )
                .expect("Failed to create sampler");

            // ── Uniform buffer ───────────────────────────────────────────────
            let uniform_size = std::mem::size_of::<Uniforms>() as u64;

            let uniform_buffer = context
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(uniform_size)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .expect("Failed to create uniform buffer");

            let uniform_req = context
                .device
                .get_buffer_memory_requirements(uniform_buffer);

            let uniform_mem_idx = context
                .find_memory_type_index(
                    &uniform_req,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
                .expect("No HOST_VISIBLE memory for uniform buffer");

            let uniform_memory = context
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(uniform_req.size)
                        .memory_type_index(uniform_mem_idx),
                    None,
                )
                .expect("Failed to allocate uniform buffer memory");

            context
                .device
                .bind_buffer_memory(uniform_buffer, uniform_memory, 0)
                .expect("Failed to bind uniform buffer memory");

            // Keep it persistently mapped — valid for HOST_COHERENT memory
            let uniform_mapped = context
                .device
                .map_memory(uniform_memory, 0, uniform_size, vk::MemoryMapFlags::empty())
                .expect("Failed to map uniform buffer") as *mut u8;

            // ── Default 1×1 blank texture ────────────────────────────────────
            // Used when no image is loaded so the descriptor set is always valid
            let blank_descriptor = create_blank_texture_descriptor(
                &context,
                descriptor_pool,
                pipeline.descriptor_set_layout,
                uniform_buffer,
                sampler,
            );

            // ── Render target ─────────────────────────────────────────────────
            let render_target = DmabufImage::new(context.clone(), width, height);

            // ── Framebuffer ───────────────────────────────────────────────────
            let framebuffer = create_framebuffer(
                &context.device,
                pipeline.render_pass,
                render_target.render_image_view,
                width,
                height,
            );

            // ── Command buffer ────────────────────────────────────────────────
            let command_buffer = context.alloc_command_buffer();

            // ── Fence (start signalled so the first frame doesn't block) ─────
            let fence = context
                .device
                .create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
                .expect("Failed to create render fence");

            // ── Cache budget ──────────────────────────────────────────────────
            // Use a conservative 512 MB; callers can adjust
            let cache_memory_budget = 512 * 1024 * 1024;

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
                framebuffer,
                framebuffer_width: width,
                framebuffer_height: height,
                command_buffer,
                fence,
                render_target,
                dirty: true,
                image_dims: (1.0, 1.0),
            };

            // Store the blank descriptor so the first render works
            renderer
                .cache
                .insert(PathBuf::from("__blank__"), blank_descriptor);
            renderer.cache_order.push(PathBuf::from("__blank__"));
            renderer.active_path = Some(PathBuf::from("__blank__"));

            renderer
        }
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Resize the render target.  Rebuilds the DmabufImage and framebuffer.
    /// Returns the new DMA-BUF fd (duplicated for GTK) and stride.
    pub fn resize(&mut self, width: u32, height: u32) -> (std::os::fd::RawFd, u32) {
        let width = width.max(1);
        let height = height.max(1);

        if width == self.framebuffer_width && height == self.framebuffer_height {
            return (
                self.render_target.export_fd_for_gtk(),
                self.render_target.stride,
            );
        }

        unsafe {
            // Wait for any in-flight work to finish before destroying resources
            self.wait_fence();

            self.context
                .device
                .destroy_framebuffer(self.framebuffer, None);

            // render_target drops here, closing old fd
            self.render_target = DmabufImage::new(self.context.clone(), width, height);

            self.framebuffer = create_framebuffer(
                &self.context.device,
                self.pipeline.render_pass,
                self.render_target.render_image_view,
                width,
                height,
            );

            self.framebuffer_width = width;
            self.framebuffer_height = height;
            self.dirty = true;
        }

        (
            self.render_target.export_fd_for_gtk(),
            self.render_target.stride,
        )
    }

    /// Upload RGBA bytes for `path` and make it the active texture.
    /// Returns the image dimensions.
    pub fn upload_and_activate(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) -> (u32, u32) {
        self.upload_texture(path, rgba, w, h);
        self.activate(path);
        (w, h)
    }

    /// Upload RGBA bytes into the cache without activating.
    pub fn cache_only(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) {
        self.upload_texture(path, rgba, w, h);
    }

    /// Switch the active texture to an already-cached image.
    /// Returns the cached dimensions if found.
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

    /// Render the current frame into the DmabufImage, then blit to the export
    /// image.  Call `export_fd_for_gtk` afterward to get the fd to hand GTK.
    pub fn render(&mut self, camera: &Camera) {
        if !self.dirty {
            return;
        }

        let active_path = match &self.active_path {
            Some(p) => p.clone(),
            None => return,
        };

        let descriptor_set = match self.cache.get(&active_path) {
            Some(c) => c.descriptor_set,
            None => return,
        };

        unsafe {
            self.wait_fence();
            self.write_uniforms(camera);
            self.record_and_submit(descriptor_set);
            self.render_target.blit_render_to_export();
        }

        self.dirty = false;
    }

    /// Return a duplicated DMA-BUF fd for the export image.
    /// GTK takes ownership of this fd.
    pub fn export_fd_for_gtk(&self) -> std::os::fd::RawFd {
        self.render_target.export_fd_for_gtk()
    }

    pub fn render_target_stride(&self) -> u32 {
        self.render_target.stride
    }

    pub fn render_target_fourcc(&self) -> u32 {
        self.render_target.format_fourcc
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn activate(&mut self, path: &Path) {
        if let Some(c) = self.cache.get(path) {
            self.image_dims = (c.dims.0 as f32, c.dims.1 as f32);
            self.active_path = Some(path.to_owned());
            self.dirty = true;

            // Move to front of LRU order
            self.cache_order.retain(|p| p != path);
            self.cache_order.insert(0, path.to_owned());
        }
    }

    unsafe fn wait_fence(&self) {
        self.context
            .device
            .wait_for_fences(std::slice::from_ref(&self.fence), true, u64::MAX)
            .expect("Fence wait failed");
        self.context
            .device
            .reset_fences(std::slice::from_ref(&self.fence))
            .expect("Fence reset failed");
    }

    unsafe fn write_uniforms(&self, camera: &Camera) {
        let scale = camera.fit_scale(self.image_dims.0, self.image_dims.1);
        let uniforms = Uniforms {
            scale,
            rotation: camera.rotation,
            zoom: camera.zoom,
            pan: [camera.position.x, camera.position.y],
            _padding: [0.0; 2],
        };
        std::ptr::copy_nonoverlapping(
            &uniforms as *const Uniforms as *const u8,
            self.uniform_mapped,
            std::mem::size_of::<Uniforms>(),
        );
    }

    unsafe fn record_and_submit(&self, descriptor_set: vk::DescriptorSet) {
        let cmd = self.command_buffer;

        self.context
            .device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .expect("Failed to begin command buffer");

        // ── Render pass ──────────────────────────────────────────────────────
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                // Dark grey background  (0.051 ≈ #0D0D0D)
                float32: [0.051, 0.051, 0.051, 1.0],
            },
        }];

        let render_pass_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.pipeline.render_pass)
            .framebuffer(self.framebuffer)
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

        // Dynamic viewport and scissor
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

        // 6 vertices → 2 triangles → fullscreen quad (no vertex buffer needed)
        self.context.device.cmd_draw(cmd, 6, 1, 0, 0);

        self.context.device.cmd_end_render_pass(cmd);

        self.context
            .device
            .end_command_buffer(cmd)
            .expect("Failed to end command buffer");

        // ── Submit ───────────────────────────────────────────────────────────
        let submit_info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));

        self.context
            .device
            .queue_submit(
                self.context.queue,
                std::slice::from_ref(&submit_info),
                self.fence,
            )
            .expect("Queue submit failed");
    }

    fn upload_texture(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) {
        // Evict if already present (re-upload of same path)
        if let Some(old) = self.cache.remove(path) {
            unsafe { old.destroy(&self.context.device) };
            self.cache_memory_used = self.cache_memory_used.saturating_sub(old.memory_bytes);
            self.cache_order.retain(|p| p != path);
        }

        let mem = (w as u64) * (h as u64) * 4;

        // Evict LRU entries until we have room (never evict the blank placeholder)
        while self.cache_memory_used + mem > self.cache_memory_budget {
            let oldest = match self.cache_order.last().cloned() {
                Some(p) if p != PathBuf::from("__blank__") => p,
                _ => break,
            };
            if let Some(evicted) = self.cache.remove(&oldest) {
                unsafe { evicted.destroy(&self.context.device) };
                self.cache_memory_used =
                    self.cache_memory_used.saturating_sub(evicted.memory_bytes);
                self.cache_order.pop();
            }
        }

        unsafe {
            let cached = upload_rgba_texture(
                &self.context,
                self.descriptor_pool,
                self.pipeline.descriptor_set_layout,
                self.uniform_buffer,
                self.sampler,
                rgba,
                w,
                h,
            );
            self.cache_memory_used += mem;
            self.cache_order.insert(0, path.to_owned());
            self.cache.insert(path.to_owned(), cached);
        }
    }
}

impl Drop for VkRenderer {
    fn drop(&mut self) {
        unsafe {
            // Wait for any in-flight work
            let _ = self.context.device.wait_for_fences(
                std::slice::from_ref(&self.fence),
                true,
                u64::MAX,
            );

            // Destroy all cached textures
            for (_, tex) in self.cache.drain() {
                tex.destroy(&self.context.device);
            }

            self.context.device.destroy_fence(self.fence, None);
            self.context.device.free_command_buffers(
                self.context.command_pool,
                std::slice::from_ref(&self.command_buffer),
            );
            self.context
                .device
                .destroy_framebuffer(self.framebuffer, None);
            self.context.device.unmap_memory(self.uniform_memory);
            self.context
                .device
                .destroy_buffer(self.uniform_buffer, None);
            self.context.device.free_memory(self.uniform_memory, None);
            self.context.device.destroy_sampler(self.sampler, None);
            self.context
                .device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            // pipeline and render_target drop via their own Drop impls
        }
    }
}

// ── Free functions ────────────────────────────────────────────────────────────

unsafe fn create_framebuffer(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    image_view: vk::ImageView,
    width: u32,
    height: u32,
) -> vk::Framebuffer {
    device
        .create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(std::slice::from_ref(&image_view))
                .width(width)
                .height(height)
                .layers(1),
            None,
        )
        .expect("Failed to create framebuffer")
}

/// Create and upload a single RGBA texture, allocate a descriptor set for it.
unsafe fn upload_rgba_texture(
    context: &VkContext,
    descriptor_pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    sampler: vk::Sampler,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> CachedTexture {
    // ── Staging buffer ───────────────────────────────────────────────────────
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
        .expect("Failed to create staging buffer");

    let staging_req = context
        .device
        .get_buffer_memory_requirements(staging_buffer);

    let staging_mem_idx = context
        .find_memory_type_index(
            &staging_req,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .expect("No HOST_VISIBLE memory for staging buffer");

    let staging_memory = context
        .device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(staging_req.size)
                .memory_type_index(staging_mem_idx),
            None,
        )
        .expect("Failed to allocate staging buffer memory");

    context
        .device
        .bind_buffer_memory(staging_buffer, staging_memory, 0)
        .expect("Failed to bind staging buffer");

    // Copy RGBA bytes into staging buffer
    let ptr = context
        .device
        .map_memory(staging_memory, 0, data_size, vk::MemoryMapFlags::empty())
        .expect("Failed to map staging buffer") as *mut u8;
    std::ptr::copy_nonoverlapping(rgba.as_ptr(), ptr, rgba.len());
    context.device.unmap_memory(staging_memory);

    // ── Device-local texture image ────────────────────────────────────────────
    let image = context
        .device
        .create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )
        .expect("Failed to create texture image");

    let tex_req = context.device.get_image_memory_requirements(image);

    let tex_mem_idx = context
        .find_memory_type_index(&tex_req, vk::MemoryPropertyFlags::DEVICE_LOCAL)
        .expect("No DEVICE_LOCAL memory for texture");

    let memory = context
        .device
        .allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(tex_req.size)
                .memory_type_index(tex_mem_idx),
            None,
        )
        .expect("Failed to allocate texture memory");

    context
        .device
        .bind_image_memory(image, memory, 0)
        .expect("Failed to bind texture image memory");

    // ── Upload: staging → device-local ───────────────────────────────────────
    {
        let cmd = context.begin_one_shot_commands();

        // Transition UNDEFINED → TRANSFER_DST_OPTIMAL
        super::dmabuf::image_layout_transition(
            &context.device,
            cmd,
            image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::empty(),
            vk::AccessFlags::TRANSFER_WRITE,
        );

        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
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

        // Transition TRANSFER_DST_OPTIMAL → SHADER_READ_ONLY_OPTIMAL
        super::dmabuf::image_layout_transition(
            &context.device,
            cmd,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::SHADER_READ,
        );

        context.end_one_shot_commands(cmd);
    }

    // Clean up staging resources
    context.device.destroy_buffer(staging_buffer, None);
    context.device.free_memory(staging_memory, None);

    // ── Image view ────────────────────────────────────────────────────────────
    let image_view = context
        .device
        .create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                ),
            None,
        )
        .expect("Failed to create texture image view");

    // ── Descriptor set ────────────────────────────────────────────────────────
    let descriptor_set = allocate_descriptor_set(
        &context.device,
        descriptor_pool,
        layout,
        uniform_buffer,
        image_view,
        sampler,
    );

    let memory_bytes = (w as u64) * (h as u64) * 4;

    CachedTexture {
        image,
        image_view,
        memory,
        descriptor_set,
        dims: (w, h),
        memory_bytes,
    }
}

/// Create a 1×1 transparent black texture and a descriptor set for it.
/// Used as the default before any image is loaded.
unsafe fn create_blank_texture_descriptor(
    context: &VkContext,
    descriptor_pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    sampler: vk::Sampler,
) -> CachedTexture {
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
    )
}

/// Allocate one descriptor set and write the uniform buffer, image, and sampler into it.
unsafe fn allocate_descriptor_set(
    device: &ash::Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
    uniform_buffer: vk::Buffer,
    image_view: vk::ImageView,
    sampler: vk::Sampler,
) -> vk::DescriptorSet {
    let descriptor_set = device
        .allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(std::slice::from_ref(&layout)),
        )
        .expect("Failed to allocate descriptor set")[0];

    // Binding 0 — uniform buffer
    let uniform_buffer_info = vk::DescriptorBufferInfo::default()
        .buffer(uniform_buffer)
        .offset(0)
        .range(std::mem::size_of::<Uniforms>() as u64);

    // Binding 1 — sampled image
    let image_info = vk::DescriptorImageInfo::default()
        .image_view(image_view)
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

    // Binding 2 — sampler
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

    descriptor_set
}
