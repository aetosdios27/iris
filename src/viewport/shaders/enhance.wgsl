struct Params {
    width: u32,
    height: u32,
    low: f32,
    high: f32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(16, 16)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let pos = vec2<i32>(vec2<u32>(gid.x, gid.y));
    let color = textureLoad(input_tex, pos, 0);
    let range = max(params.high - params.low, 0.001);
    let adjusted = (color.rgb - vec3<f32>(params.low)) / range;
    let clamped = clamp(adjusted, vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(output_tex, pos, vec4<f32>(clamped, color.a));
}
