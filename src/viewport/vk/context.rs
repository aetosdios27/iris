use ash::{Device, Entry, Instance, vk};
use std::ffi::CString;
use std::sync::Arc;

pub struct VkContext {
    pub entry: Entry,
    pub instance: Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub queue: vk::Queue,
    pub queue_family_index: u32,
    pub memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub command_pool: vk::CommandPool,
}

impl VkContext {
    pub fn new() -> Arc<Self> {
        unsafe {
            let entry = Entry::load().expect("Failed to load Vulkan library");

            // ── Instance ────────────────────────────────────────────────────────
            let instance_extensions = [
                ash::vk::KHR_GET_PHYSICAL_DEVICE_PROPERTIES2_NAME.as_ptr(),
                ash::vk::KHR_EXTERNAL_MEMORY_CAPABILITIES_NAME.as_ptr(),
            ];

            let app_name = CString::new("Iris").unwrap();
            let app_info = vk::ApplicationInfo::default()
                .application_name(app_name.as_c_str())
                .api_version(vk::make_api_version(0, 1, 2, 0));

            let create_info = vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .enabled_extension_names(&instance_extensions);

            let instance = entry
                .create_instance(&create_info, None)
                .expect("Failed to create Vulkan instance");

            // ── Physical device ─────────────────────────────────────────────────
            let physical_devices = instance
                .enumerate_physical_devices()
                .expect("Failed to enumerate physical devices");

            assert!(
                !physical_devices.is_empty(),
                "No Vulkan-capable GPU found on this system"
            );

            // Prefer discrete GPU, fall back to integrated, then anything
            let physical_device = physical_devices
                .iter()
                .copied()
                .find(|&p| {
                    instance.get_physical_device_properties(p).device_type
                        == vk::PhysicalDeviceType::DISCRETE_GPU
                })
                .or_else(|| {
                    physical_devices.iter().copied().find(|&p| {
                        instance.get_physical_device_properties(p).device_type
                            == vk::PhysicalDeviceType::INTEGRATED_GPU
                    })
                })
                .unwrap_or(physical_devices[0]);

            let props = instance.get_physical_device_properties(physical_device);
            let name = std::ffi::CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy();
            println!("[Iris] Vulkan GPU: {}", name);

            // ── Queue family ────────────────────────────────────────────────────
            let queue_family_index = instance
                .get_physical_device_queue_family_properties(physical_device)
                .iter()
                .enumerate()
                .find(|(_, info)| info.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .map(|(i, _)| i as u32)
                .expect("No graphics queue family found");

            // ── Logical device ──────────────────────────────────────────────────
            let device_extensions = [
                ash::vk::KHR_EXTERNAL_MEMORY_NAME.as_ptr(),
                ash::vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
                ash::vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
            ];

            let priorities = [1.0_f32];
            let queue_info = vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family_index)
                .queue_priorities(&priorities);

            let device_create_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(std::slice::from_ref(&queue_info))
                .enabled_extension_names(&device_extensions);

            let device = instance
                .create_device(physical_device, &device_create_info, None)
                .expect("Failed to create logical Vulkan device");

            let queue = device.get_device_queue(queue_family_index, 0);
            let memory_properties = instance.get_physical_device_memory_properties(physical_device);

            // ── Command pool ────────────────────────────────────────────────────
            // RESET_COMMAND_BUFFER allows us to re-record commands every frame
            let command_pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(queue_family_index)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .expect("Failed to create command pool");

            Arc::new(Self {
                entry,
                instance,
                physical_device,
                device,
                queue,
                queue_family_index,
                memory_properties,
                command_pool,
            })
        }
    }

    // ── Memory helpers ───────────────────────────────────────────────────────

    pub fn find_memory_type_index(
        &self,
        memory_req: &vk::MemoryRequirements,
        flags: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        self.memory_properties
            .memory_types
            .iter()
            .enumerate()
            .find(|(i, mem_type)| {
                (memory_req.memory_type_bits & (1 << i)) != 0
                    && mem_type.property_flags.contains(flags)
            })
            .map(|(i, _)| i as u32)
    }

    /// Allocate a single primary command buffer from our pool.
    pub unsafe fn alloc_command_buffer(&self) -> vk::CommandBuffer {
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);

        self.device
            .allocate_command_buffers(&alloc_info)
            .expect("Failed to allocate command buffer")[0]
    }

    /// Submit a command buffer to the graphics queue and wait for it to finish.
    /// Useful for one-shot upload / blit commands.
    pub unsafe fn submit_and_wait(&self, cmd: vk::CommandBuffer) {
        let submit_info = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));

        let fence = self
            .device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .expect("Failed to create fence");

        self.device
            .queue_submit(self.queue, std::slice::from_ref(&submit_info), fence)
            .expect("Queue submit failed");

        self.device
            .wait_for_fences(std::slice::from_ref(&fence), true, u64::MAX)
            .expect("Fence wait failed");

        self.device.destroy_fence(fence, None);
    }

    /// Begin a one-shot command buffer (records immediately, caller must call
    /// `submit_and_wait` then free the buffer).
    pub unsafe fn begin_one_shot_commands(&self) -> vk::CommandBuffer {
        let cmd = self.alloc_command_buffer();
        self.device
            .begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .expect("Failed to begin command buffer");
        cmd
    }

    /// End and submit a one-shot command buffer, wait for completion, then free it.
    pub unsafe fn end_one_shot_commands(&self, cmd: vk::CommandBuffer) {
        self.device
            .end_command_buffer(cmd)
            .expect("Failed to end command buffer");
        self.submit_and_wait(cmd);
        self.device
            .free_command_buffers(self.command_pool, std::slice::from_ref(&cmd));
    }
}

impl Drop for VkContext {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
