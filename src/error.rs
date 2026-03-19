/// Errors produced by the Vulkan rendering pipeline.
///
/// All variants carry a human-readable message string so callers can surface
/// them directly in a `adw::Toast` without any additional formatting.
#[derive(Debug)]
pub enum IrisError {
    /// A Vulkan API call returned an error code.
    Vk {
        call: &'static str,
        code: ash::vk::Result,
    },
    /// A Vulkan memory type matching the requested flags was not found.
    NoMemoryType(&'static str),
    /// The DMA-BUF fd could not be exported from device memory.
    DmaBufExport(ash::vk::Result),
    /// The blit semaphore could not be exported as a sync_fd.
    SyncFdExport(ash::vk::Result),
    /// A framebuffer could not be created (usually after resize).
    Framebuffer(ash::vk::Result),
    /// Image upload failed (staging buffer, texture image, etc.).
    Upload {
        stage: &'static str,
        code: ash::vk::Result,
    },
    /// An image was too large and downscaling failed.
    Downscale(String),
    /// Any other error with a plain message.
    Other(String),
}

impl IrisError {
    /// A short message suitable for display in an `adw::Toast`.
    pub fn to_toast_message(&self) -> String {
        match self {
            IrisError::Vk { call, code } => format!("GPU error in {call}: {code}"),
            IrisError::NoMemoryType(ctx) => format!("No suitable GPU memory for {ctx}"),
            IrisError::DmaBufExport(code) => format!("DMA-BUF export failed: {code}"),
            IrisError::SyncFdExport(code) => format!("Sync-fd export failed: {code}"),
            IrisError::Framebuffer(code) => format!("Framebuffer creation failed: {code}"),
            IrisError::Upload { stage, code } => {
                format!("Texture upload failed at {stage}: {code}")
            }
            IrisError::Downscale(msg) => format!("Image downscale failed: {msg}"),
            IrisError::Other(msg) => msg.clone(),
        }
    }
}

impl std::fmt::Display for IrisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_toast_message())
    }
}

impl std::error::Error for IrisError {}

pub type IrisResult<T> = Result<T, IrisError>;

// ── Convenience macro ─────────────────────────────────────────────────────────

/// Convert an `ash::vk::Result` from a named Vulkan call into an `IrisError::Vk`.
///
/// Usage: `vk_check!(device.create_fence(...), "create_fence")?`
#[macro_export]
macro_rules! vk_check {
    ($expr:expr, $call:literal) => {
        $expr.map_err(|code| $crate::error::IrisError::Vk { call: $call, code })
    };
}
