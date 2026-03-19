struct Params {
    width: u32,
    height: u32,
    amount: f32,
    _pad: f32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn safe_load(p: vec2<i32>) -> vec4<f32> {
    let c = clamp(p, vec2<i32>(0), vec2<i32>(i32(params.width) - 1, i32(params.height) - 1));
    return textureLoad(input_tex, c, 0);
}

@compute @workgroup_size(16, 16)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let pos = vec2<i32>(vec2<u32>(gid.x, gid.y));
    let center = safe_load(pos);

    var blur = vec4<f32>(0.0);
    blur += safe_load(pos + vec2<i32>(-1, -1)) * 0.0625;
    blur += safe_load(pos + vec2<i32>( 0, -1)) * 0.125;
    blur += safe_load(pos + vec2<i32>( 1, -1)) * 0.0625;
    blur += safe_load(pos + vec2<i32>(-1,  0)) * 0.125;
    blur += center * 0.25;
    blur += safe_load(pos + vec2<i32>( 1,  0)) * 0.125;
    blur += safe_load(pos + vec2<i32>(-1,  1)) * 0.0625;
    blur += safe_load(pos + vec2<i32>( 0,  1)) * 0.125;
    blur += safe_load(pos + vec2<i32>( 1,  1)) * 0.0625;

    let sharp = center + (center - blur) * params.amount;
    let result = clamp(sharp, vec4<f32>(0.0), vec4<f32>(1.0));
    textureStore(output_tex, pos, vec4<f32>(result.rgb, center.a));
}
