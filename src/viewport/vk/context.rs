use ash::{Device, Entry, Instance, vk};
use std::ffi::CString;
use std::sync::Arc;

use crate::error::{IrisError, IrisResult};
use crate::vk_check;

pub struct VkContext {
    pub entry: Entry,
    pub instance: Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: Device,
    pub queue: vk::Queue,
    pub queue_family_index: u32,
    pub memory_properties: vk::PhysicalDeviceMemoryProperties,
    pub command_pool: vk::CommandPool,
    pub device_limits: vk::PhysicalDeviceLimits,
}

impl VkContext {
    pub fn new() -> IrisResult<Arc<Self>> {
        unsafe {
            let entry = Entry::load()
                .map_err(|e| IrisError::Other(format!("Failed to load Vulkan: {e}")))?;

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

            let instance = vk_check!(
                entry.create_instance(&create_info, None),
                "vkCreateInstance"
            )?;

            let physical_devices = vk_check!(
                instance.enumerate_physical_devices(),
                "vkEnumeratePhysicalDevices"
            )?;

            if physical_devices.is_empty() {
                return Err(IrisError::Other(
                    "No Vulkan-capable GPU found on this system".into(),
                ));
            }

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
            println!("[Iris] Vulkan GPU: {name}");

            let queue_family_index = instance
                .get_physical_device_queue_family_properties(physical_device)
                .iter()
                .enumerate()
                .find(|(_, info)| info.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .map(|(i, _)| i as u32)
                .ok_or_else(|| IrisError::Other("No graphics queue family found".into()))?;

            let device_extensions = [
                ash::vk::KHR_EXTERNAL_MEMORY_NAME.as_ptr(),
                ash::vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
                ash::vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
                ash::vk::KHR_EXTERNAL_SEMAPHORE_NAME.as_ptr(),
                ash::vk::KHR_EXTERNAL_SEMAPHORE_FD_NAME.as_ptr(),
            ];

            let priorities = [1.0_f32];
            let queue_info = vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family_index)
                .queue_priorities(&priorities);

            let device_create_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(std::slice::from_ref(&queue_info))
                .enabled_extension_names(&device_extensions);

            let device = vk_check!(
                instance.create_device(physical_device, &device_create_info, None),
                "vkCreateDevice"
            )?;

            let queue = device.get_device_queue(queue_family_index, 0);
            let memory_properties = instance.get_physical_device_memory_properties(physical_device);

            let command_pool = vk_check!(
                device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(queue_family_index)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                ),
                "vkCreateCommandPool"
            )?;

            Ok(Arc::new(Self {
                entry,
                instance,
                physical_device,
                device,
                queue,
                queue_family_index,
                memory_properties,
                command_pool,
                device_limits: props.limits,
            }))
        }
    }

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

    pub fn vram_budget_bytes(&self) -> u64 {
        let heap_count = self.memory_properties.memory_heap_count as usize;
        let largest_device_local = self.memory_properties.memory_heaps[..heap_count]
            .iter()
            .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
            .map(|h| h.size)
            .max()
            .unwrap_or(512 * 1024 * 1024);
        (largest_device_local / 2).clamp(256 * 1024 * 1024, 4 * 1024 * 1024 * 1024)
    }

    pub unsafe fn alloc_command_buffer(&self) -> IrisResult<vk::CommandBuffer> {
        let bufs = vk_check!(
            self.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            ),
            "vkAllocateCommandBuffers"
        )?;
        Ok(bufs[0])
    }

    pub unsafe fn submit_and_wait(&self, cmd: vk::CommandBuffer) -> IrisResult<()> {
        let fence = vk_check!(
            self.device
                .create_fence(&vk::FenceCreateInfo::default(), None),
            "vkCreateFence"
        )?;
        vk_check!(
            self.device.queue_submit(
                self.queue,
                std::slice::from_ref(
                    &vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd))
                ),
                fence,
            ),
            "vkQueueSubmit"
        )?;
        vk_check!(
            self.device
                .wait_for_fences(std::slice::from_ref(&fence), true, u64::MAX),
            "vkWaitForFences"
        )?;
        self.device.destroy_fence(fence, None);
        Ok(())
    }

    pub unsafe fn begin_one_shot_commands(&self) -> IrisResult<vk::CommandBuffer> {
        let cmd = self.alloc_command_buffer()?;
        vk_check!(
            self.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            ),
            "vkBeginCommandBuffer"
        )?;
        Ok(cmd)
    }

    pub unsafe fn end_one_shot_commands(&self, cmd: vk::CommandBuffer) -> IrisResult<()> {
        vk_check!(self.device.end_command_buffer(cmd), "vkEndCommandBuffer")?;
        self.submit_and_wait(cmd)?;
        self.device
            .free_command_buffers(self.command_pool, std::slice::from_ref(&cmd));
        Ok(())
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
