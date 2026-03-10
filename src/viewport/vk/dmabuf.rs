use ash::vk;
use std::os::fd::RawFd;
use std::sync::Arc;

use super::context::VkContext;

/// A pair of Vulkan images:
///
/// - `render_image`  — DEVICE_LOCAL, OPTIMAL tiling, COLOR_ATTACHMENT_OPTIMAL.
///                     The GPU renders into this every frame.
/// - `export_image`  — HOST_VISIBLE, LINEAR tiling, exported as a DMA-BUF fd.
///                     After each render we blit render → export so the
///                     Wayland compositor can scan it out zero-copy.
pub struct DmabufImage {
    pub context: Arc<VkContext>,

    // ── Render target (OPTIMAL, GPU-only) ───────────────────────────────────
    pub render_image: vk::Image,
    pub render_image_view: vk::ImageView,
    render_memory: vk::DeviceMemory,

    // ── Export target (LINEAR, DMA-BUF) ─────────────────────────────────────
    pub export_image: vk::Image,
    export_memory: vk::DeviceMemory,

    // The raw Linux file descriptor for the export image's memory.
    // Kept alive as long as this struct is alive; duplicated before handing to GTK.
    fd: RawFd,

    pub width: u32,
    pub height: u32,
    pub stride: u32,

    // DRM_FORMAT_ABGR8888 — matches VK_FORMAT_R8G8B8A8_UNORM byte order
    pub format_fourcc: u32,
}

// ── Format used everywhere ───────────────────────────────────────────────────
const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

impl DmabufImage {
    pub fn new(context: Arc<VkContext>, width: u32, height: u32) -> Self {
        unsafe {
            // ── 1. Render image (OPTIMAL, DEVICE_LOCAL) ──────────────────────
            // Pure GPU-side; no CPU access, no export handle needed.
            let render_image = context
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(FORMAT)
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
                            vk::ImageUsageFlags::COLOR_ATTACHMENT
                                | vk::ImageUsageFlags::TRANSFER_SRC,
                        )
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .expect("Failed to create render image");

            let render_req = context.device.get_image_memory_requirements(render_image);
            let render_mem_idx = context
                .find_memory_type_index(&render_req, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .expect("No DEVICE_LOCAL memory type for render image");

            let render_memory = context
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(render_req.size)
                        .memory_type_index(render_mem_idx),
                    None,
                )
                .expect("Failed to allocate render image memory");

            context
                .device
                .bind_image_memory(render_image, render_memory, 0)
                .expect("Failed to bind render image memory");

            // ── 2. Render image view ─────────────────────────────────────────
            let render_image_view = context
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(render_image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(FORMAT)
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
                .expect("Failed to create render image view");

            // ── 3. Transition render image to COLOR_ATTACHMENT_OPTIMAL ────────
            // Do this once at creation so the first render pass finds it in the
            // right layout (the render pass itself uses UNDEFINED → COLOR_ATTACHMENT
            // but after the first frame we re-use the image so we pre-warm it).
            {
                let cmd = context.begin_one_shot_commands();
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
                context.end_one_shot_commands(cmd);
            }

            // ── 4. Export image (LINEAR, HOST_VISIBLE, exportable) ───────────
            // Must be LINEAR so the stride/offset the compositor gets is valid.
            // We use TRANSFER_DST because we blit into it from the render image.
            let mut ext_img_info = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let export_image = context
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(FORMAT)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::LINEAR)
                        .usage(vk::ImageUsageFlags::TRANSFER_DST)
                        .initial_layout(vk::ImageLayout::UNDEFINED)
                        .push_next(&mut ext_img_info),
                    None,
                )
                .expect("Failed to create export image");

            let export_req = context.device.get_image_memory_requirements(export_image);

            // Prefer HOST_COHERENT so we don't need explicit cache flushes.
            let export_mem_idx = context
                .find_memory_type_index(
                    &export_req,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
                .expect("No HOST_VISIBLE | HOST_COHERENT memory type for export image");

            let mut export_alloc_info = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);

            let export_memory = context
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(export_req.size)
                        .memory_type_index(export_mem_idx)
                        .push_next(&mut export_alloc_info),
                    None,
                )
                .expect("Failed to allocate export image memory");

            context
                .device
                .bind_image_memory(export_image, export_memory, 0)
                .expect("Failed to bind export image memory");

            // ── 5. Transition export image to TRANSFER_DST_OPTIMAL ───────────
            {
                let cmd = context.begin_one_shot_commands();
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
                context.end_one_shot_commands(cmd);
            }

            // ── 6. Extract DMA-BUF fd ────────────────────────────────────────
            let ext_mem_fd =
                ash::khr::external_memory_fd::Device::new(&context.instance, &context.device);

            let fd = ext_mem_fd
                .get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(export_memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
                )
                .expect("Failed to extract DMA-BUF fd");

            // ── 7. Query stride from LINEAR layout ───────────────────────────
            let layout = context.device.get_image_subresource_layout(
                export_image,
                vk::ImageSubresource {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    array_layer: 0,
                },
            );
            let stride = layout.row_pitch as u32;

            // DRM_FORMAT_ABGR8888 — matches VK_FORMAT_R8G8B8A8_UNORM
            let format_fourcc = 0x34324241u32;

            Self {
                context,
                render_image,
                render_image_view,
                render_memory,
                export_image,
                export_memory,
                fd,
                width,
                height,
                stride,
                format_fourcc,
            }
        }
    }

    /// Blit the completed render image into the LINEAR export image so the
    /// Wayland compositor can scan it out.  Call this after the render pass
    /// has finished and the fence has been waited on.
    pub fn blit_render_to_export(&self) {
        unsafe {
            let cmd = self.context.begin_one_shot_commands();

            // render_image is currently in COLOR_ATTACHMENT_OPTIMAL after the
            // render pass; transition it to TRANSFER_SRC_OPTIMAL for the blit.
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

            // Transition render image back to COLOR_ATTACHMENT_OPTIMAL for next frame
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

            // Transition export image to GENERAL so GTK / the compositor can read it
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

            self.context.end_one_shot_commands(cmd);

            // After the blit the export image needs to go back to
            // TRANSFER_DST_OPTIMAL so the next frame's blit is valid.
            let cmd2 = self.context.begin_one_shot_commands();
            image_layout_transition(
                &self.context.device,
                cmd2,
                self.export_image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            );
            self.context.end_one_shot_commands(cmd2);
        }
    }

    /// Duplicate the DMA-BUF fd for GTK.  GTK owns and closes its copy;
    /// our original fd stays open until this struct is dropped.
    pub fn export_fd_for_gtk(&self) -> RawFd {
        unsafe {
            let dup = libc::dup(self.fd);
            assert!(dup >= 0, "libc::dup failed for DMA-BUF fd");
            dup
        }
    }
}

impl Drop for DmabufImage {
    fn drop(&mut self) {
        unsafe {
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

// ── Layout transition helper ─────────────────────────────────────────────────

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
