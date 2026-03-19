use ash::vk;
use std::os::fd::RawFd;
use std::sync::Arc;

use super::context::VkContext;
use crate::error::{IrisError, IrisResult};
use crate::vk_check;

pub struct DmabufImage {
    pub context: Arc<VkContext>,

    pub render_image: vk::Image,
    pub render_image_view: vk::ImageView,
    render_memory: vk::DeviceMemory,

    pub export_image: vk::Image,
    export_memory: vk::DeviceMemory,
    export_memory_size: vk::DeviceSize,

    fd: RawFd,

    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format_fourcc: u32,

    blit_command_buffer: vk::CommandBuffer,
    blit_fence: vk::Fence,
    blit_semaphore: vk::Semaphore,

    vk_format: vk::Format,
}

impl DmabufImage {
    pub fn new(
        context: Arc<VkContext>,
        width: u32,
        height: u32,
        vk_format: vk::Format,
        format_fourcc: u32,
    ) -> IrisResult<Self> {
        unsafe {
            // ── 1. Render image ───────────────────────────────────────────────
            let render_image = vk_check!(
                context.device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk_format)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::TRANSFER_SRC
                                | vk::ImageUsageFlags::SAMPLED,
                        )
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                ),
                "vkCreateImage(render)"
            )?;

            let render_req = context.device.get_image_memory_requirements(render_image);
            let render_mem_idx = context
                .find_memory_type_index(&render_req, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .ok_or(IrisError::NoMemoryType("render image"))?;

            let render_memory = vk_check!(
                context.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(render_req.size)
                        .memory_type_index(render_mem_idx),
                    None,
                ),
                "vkAllocateMemory(render)"
            )?;

            vk_check!(
                context
                    .device
                    .bind_image_memory(render_image, render_memory, 0),
                "vkBindImageMemory(render)"
            )?;

            // ── 2. Render image view ──────────────────────────────────────────
            let render_image_view = vk_check!(
                context.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(render_image)
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
                "vkCreateImageView(render)"
            )?;

            // ── 3. Transition render image ────────────────────────────────────
            {
                let cmd = context.begin_one_shot_commands()?;
                image_layout_transition(
                    &context.device,
                    cmd,
                    render_image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                );
                context.end_one_shot_commands(cmd)?;
            }

            // ── 4. Export image ───────────────────────────────────────────────
            let mut ext_img_info = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let export_image = vk_check!(
                context.device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk_format)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::LINEAR)
                        .usage(vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED)
                        .push_next(&mut ext_img_info),
                    None,
                ),
                "vkCreateImage(export)"
            )?;

            let export_req = context.device.get_image_memory_requirements(export_image);
            let export_mem_idx = context
                .find_memory_type_index(
                    &export_req,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
                .ok_or(IrisError::NoMemoryType("export image"))?;

            let mut export_alloc_info = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let export_memory = vk_check!(
                context.device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(export_req.size)
                        .memory_type_index(export_mem_idx)
                        .push_next(&mut export_alloc_info),
                    None,
                ),
                "vkAllocateMemory(export)"
            )?;

            let export_memory_size = export_req.size;

            vk_check!(
                context
                    .device
                    .bind_image_memory(export_image, export_memory, 0),
                "vkBindImageMemory(export)"
            )?;

            // ── 5. Transition export image ────────────────────────────────────
            {
                let cmd = context.begin_one_shot_commands()?;
                image_layout_transition(
                    &context.device,
                    cmd,
                    export_image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::TRANSFER_WRITE,
                );
                context.end_one_shot_commands(cmd)?;
            }

            // ── 6. Extract DMA-BUF fd ─────────────────────────────────────────
            let ext_mem_fd =
                ash::khr::external_memory_fd::Device::new(&context.instance, &context.device);

            let fd = ext_mem_fd
                .get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(export_memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
                )
                .map_err(IrisError::DmaBufExport)?;

            // ── 7. Query stride ───────────────────────────────────────────────
            let layout = context.device.get_image_subresource_layout(
                export_image,
                vk::ImageSubresource {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    array_layer: 0,
                },
            );
            let stride = layout.row_pitch as u32;

            // ── 8. Async blit resources ───────────────────────────────────────
            let blit_command_buffer = context.alloc_command_buffer()?;

            let blit_fence = vk_check!(
                context.device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                ),
                "vkCreateFence(blit)"
            )?;

            let mut export_sem_info = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
            let blit_semaphore = vk_check!(
                context.device.create_semaphore(
                    &vk::SemaphoreCreateInfo::default().push_next(&mut export_sem_info),
                    None,
                ),
                "vkCreateSemaphore(blit)"
            )?;

            Ok(Self {
                context,
                render_image,
                render_image_view,
                render_memory,
                export_image,
                export_memory,
                export_memory_size,
                fd,
                width,
                height,
                stride,
                format_fourcc,
                blit_command_buffer,
                blit_fence,
                blit_semaphore,
                vk_format,
            })
        }
    }

    /// Blit render → export asynchronously and return a Linux sync_fd.
    pub fn blit_render_to_export_async(&self) -> IrisResult<RawFd> {
        unsafe {
            vk_check!(
                self.context.device.wait_for_fences(
                    std::slice::from_ref(&self.blit_fence),
                    true,
                    u64::MAX
                ),
                "vkWaitForFences(blit)"
            )?;
            vk_check!(
                self.context
                    .device
                    .reset_fences(std::slice::from_ref(&self.blit_fence)),
                "vkResetFences(blit)"
            )?;

            let cmd = self.blit_command_buffer;
            vk_check!(
                self.context.device.begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                ),
                "vkBeginCommandBuffer(blit)"
            )?;

            image_layout_transition(
                &self.context.device,
                cmd,
                self.export_image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            );
            image_layout_transition(
                &self.context.device,
                cmd,
                self.render_image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                vk::AccessFlags::TRANSFER_READ,
            );

            let subresource = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1);

            let region = vk::ImageBlit::default()
                .src_subresource(subresource)
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: self.width as i32,
                        y: self.height as i32,
                        z: 1,
                    },
                ])
                .dst_subresource(subresource)
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: self.width as i32,
                        y: self.height as i32,
                        z: 1,
                    },
                ]);

            self.context.device.cmd_blit_image(
                cmd,
                self.render_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.export_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                std::slice::from_ref(&region),
                vk::Filter::LINEAR,
            );

            image_layout_transition(
                &self.context.device,
                cmd,
                self.render_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::TRANSFER_READ,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            );
            image_layout_transition(
                &self.context.device,
                cmd,
                self.export_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::empty(),
            );

            vk_check!(
                self.context.device.end_command_buffer(cmd),
                "vkEndCommandBuffer(blit)"
            )?;

            let signal_semaphores = [self.blit_semaphore];
            let submit_info = vk::SubmitInfo::default()
                .command_buffers(std::slice::from_ref(&cmd))
                .signal_semaphores(&signal_semaphores);

            vk_check!(
                self.context.device.queue_submit(
                    self.context.queue,
                    std::slice::from_ref(&submit_info),
                    self.blit_fence,
                ),
                "vkQueueSubmit(blit)"
            )?;

            let ext_sem_fd = ash::khr::external_semaphore_fd::Device::new(
                &self.context.instance,
                &self.context.device,
            );
            ext_sem_fd
                .get_semaphore_fd(
                    &vk::SemaphoreGetFdInfoKHR::default()
                        .semaphore(self.blit_semaphore)
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
                )
                .map_err(IrisError::SyncFdExport)
        }
    }

    /// Blit an external source image (already in TRANSFER_SRC_OPTIMAL) to export.
    pub fn blit_external_to_export_async(&self, source: vk::Image) -> IrisResult<RawFd> {
        unsafe {
            vk_check!(
                self.context.device.wait_for_fences(
                    std::slice::from_ref(&self.blit_fence),
                    true,
                    u64::MAX
                ),
                "vkWaitForFences(blit_ext)"
            )?;
            vk_check!(
                self.context
                    .device
                    .reset_fences(std::slice::from_ref(&self.blit_fence)),
                "vkResetFences(blit_ext)"
            )?;

            let cmd = self.blit_command_buffer;
            vk_check!(
                self.context.device.begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                ),
                "vkBeginCommandBuffer(blit_ext)"
            )?;

            image_layout_transition(
                &self.context.device,
                cmd,
                self.export_image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            );

            let subresource = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1);

            let region = vk::ImageBlit::default()
                .src_subresource(subresource)
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: self.width as i32,
                        y: self.height as i32,
                        z: 1,
                    },
                ])
                .dst_subresource(subresource)
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: self.width as i32,
                        y: self.height as i32,
                        z: 1,
                    },
                ]);

            self.context.device.cmd_blit_image(
                cmd,
                source,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.export_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                std::slice::from_ref(&region),
                vk::Filter::LINEAR,
            );

            image_layout_transition(
                &self.context.device,
                cmd,
                self.export_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::GENERAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::empty(),
            );

            vk_check!(
                self.context.device.end_command_buffer(cmd),
                "vkEndCommandBuffer(blit_ext)"
            )?;

            let signal_semaphores = [self.blit_semaphore];
            let submit_info = vk::SubmitInfo::default()
                .command_buffers(std::slice::from_ref(&cmd))
                .signal_semaphores(&signal_semaphores);

            vk_check!(
                self.context.device.queue_submit(
                    self.context.queue,
                    std::slice::from_ref(&submit_info),
                    self.blit_fence,
                ),
                "vkQueueSubmit(blit_ext)"
            )?;

            let ext_sem_fd = ash::khr::external_semaphore_fd::Device::new(
                &self.context.instance,
                &self.context.device,
            );
            ext_sem_fd
                .get_semaphore_fd(
                    &vk::SemaphoreGetFdInfoKHR::default()
                        .semaphore(self.blit_semaphore)
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD),
                )
                .map_err(IrisError::SyncFdExport)
        }
    }

    /// Duplicate the DMA-BUF fd for GTK.
    pub fn export_fd_for_gtk(&self) -> IrisResult<RawFd> {
        let dup = unsafe { libc::dup(self.fd) };
        if dup < 0 {
            Err(IrisError::Other("libc::dup failed for DMA-BUF fd".into()))
        } else {
            Ok(dup)
        }
    }

    /// Map the HOST_VISIBLE export image and copy pixels to a `Vec<u8>`.
    pub fn read_pixels(&self) -> IrisResult<Vec<u8>> {
        unsafe {
            let ptr = vk_check!(
                self.context.device.map_memory(
                    self.export_memory,
                    0,
                    self.export_memory_size,
                    vk::MemoryMapFlags::empty(),
                ),
                "vkMapMemory(read_pixels)"
            )? as *const u8;

            let pixel_bytes = (self.stride * self.height) as usize;
            let pixels = std::slice::from_raw_parts(ptr, pixel_bytes).to_vec();
            self.context.device.unmap_memory(self.export_memory);
            Ok(pixels)
        }
    }
}

impl Drop for DmabufImage {
    fn drop(&mut self) {
        unsafe {
            let _ = self.context.device.wait_for_fences(
                std::slice::from_ref(&self.blit_fence),
                true,
                u64::MAX,
            );
            self.context
                .device
                .destroy_semaphore(self.blit_semaphore, None);
            self.context.device.destroy_fence(self.blit_fence, None);
            self.context.device.free_command_buffers(
                self.context.command_pool,
                std::slice::from_ref(&self.blit_command_buffer),
            );
            self.context
                .device
                .destroy_image_view(self.render_image_view, None);
            self.context.device.destroy_image(self.render_image, None);
            self.context.device.free_memory(self.render_memory, None);
            self.context.device.destroy_image(self.export_image, None);
            self.context.device.free_memory(self.export_memory, None);
            libc::close(self.fd);
        }
    }
}

// ── Layout transition helper ──────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub unsafe fn image_layout_transition(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
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
                .base_mip_level(0)
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
