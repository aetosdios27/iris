use ash::vk;
use naga::{
    back::spv,
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

pub fn compile_wgsl(device: &ash::Device, source: &str) -> vk::ShaderModule {
    // 1. Parse WGSL
    let module = wgsl::parse_str(source).expect("Failed to parse WGSL shader");

    // 2. Validate WGSL
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::empty());
    let info = validator
        .validate(&module)
        .expect("Failed to validate WGSL shader");

    // 3. Translate to SPIR-V Bytecode
    let mut words = Vec::new();
    let mut spv_options = spv::Options::default();
    spv_options.flags.insert(spv::WriterFlags::DEBUG);

    let mut writer = spv::Writer::new(&spv_options).unwrap();
    writer
        .write(&module, &info, None, &None, &mut words)
        .expect("Failed to write SPIR-V");

    // 4. Create Vulkan Shader Module
    let create_info = vk::ShaderModuleCreateInfo::default().code(&words);
    unsafe {
        device
            .create_shader_module(&create_info, None)
            .expect("Failed to create Vulkan Shader Module")
    }
}
