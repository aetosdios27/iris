use std::sync::Arc;
use wgpu::*;

pub struct GpuContext {
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub texture_format: TextureFormat,
    pub cache_budget: u64,
    pub max_texture_size: u32,
}

impl GpuContext {
    pub async fn new() -> Self {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::VULKAN | Backends::GL,
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find suitable GPU adapter");

        let info = adapter.get_info();
        let cache_budget = detect_cache_budget(&info);

        println!("GPU: {} ({:?})", info.name, info.device_type);
        println!("Cache budget: {:.0} MB", cache_budget as f64 / 1_048_576.0,);

        let (device, queue) = adapter
            .request_device(
                &DeviceDescriptor {
                    label: Some("Iris Device"),
                    required_features: Features::empty(),
                    required_limits: Limits::default(),
                    memory_hints: Default::default(),
                },
                None,
            )
            .await
            .expect("Failed to create device");

        let max_texture_size = device.limits().max_texture_dimension_2d;
        println!("Max texture size: {}px", max_texture_size);

        Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
            texture_format: TextureFormat::Rgba8UnormSrgb,
            cache_budget,
            max_texture_size,
        }
    }
}

fn detect_cache_budget(info: &AdapterInfo) -> u64 {
    let available = detect_available_memory();

    let budget = match info.device_type {
        DeviceType::DiscreteGpu => {
            let b = available * 40 / 100;
            b.min(4 * 1024 * 1024 * 1024)
        }
        DeviceType::IntegratedGpu => {
            let b = available * 8 / 100;
            b.min(1536 * 1024 * 1024)
        }
        DeviceType::VirtualGpu => {
            let b = available * 5 / 100;
            b.min(512 * 1024 * 1024)
        }
        _ => {
            let b = available * 3 / 100;
            b.min(256 * 1024 * 1024)
        }
    };

    budget.max(128 * 1024 * 1024)
}

fn detect_available_memory() -> u64 {
    if let Some(vram) = detect_vram_sysfs() {
        return vram;
    }
    if let Some(vram) = detect_vram_nvidia() {
        return vram;
    }
    detect_system_ram()
}

fn detect_vram_sysfs() -> Option<u64> {
    let drm_dir = std::fs::read_dir("/sys/class/drm").ok()?;
    for entry in drm_dir.filter_map(|e| e.ok()) {
        let vram_path = entry.path().join("device/mem_info_vram_total");
        if let Ok(content) = std::fs::read_to_string(&vram_path) {
            if let Ok(bytes) = content.trim().parse::<u64>() {
                if bytes > 0 {
                    return Some(bytes);
                }
            }
        }
    }
    None
}

fn detect_vram_nvidia() -> Option<u64> {
    let nvidia_dir = std::fs::read_dir("/proc/driver/nvidia/gpus").ok()?;
    for entry in nvidia_dir.filter_map(|e| e.ok()) {
        let info_path = entry.path().join("information");
        if let Ok(content) = std::fs::read_to_string(&info_path) {
            for line in content.lines() {
                if line.contains("Video Memory") || line.contains("FB Memory") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    for (i, part) in parts.iter().enumerate() {
                        if let Ok(mb) = part.parse::<u64>() {
                            if i + 1 < parts.len() {
                                let unit = parts[i + 1].to_uppercase();
                                if unit.starts_with("GB") || unit.starts_with("GIB") {
                                    return Some(mb * 1024 * 1024 * 1024);
                                }
                            }
                            return Some(mb * 1024 * 1024);
                        }
                    }
                }
            }
        }
    }
    None
}

fn detect_system_ram() -> u64 {
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<u64>() {
                        return kb * 1024;
                    }
                }
            }
        }
    }
    8 * 1024 * 1024 * 1024
}
