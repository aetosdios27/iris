struct Params {
    width: u32,
    height: u32,
    spatial_sigma: f32,
    range_sigma: f32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@compute @workgroup_size(16, 16)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) {
        return;
    }
    let pos = vec2<i32>(vec2<u32>(gid.x, gid.y));
    let center = textureLoad(input_tex, pos, 0);
    let cl = luma(center.rgb);
    let ss2 = 2.0 * params.spatial_sigma * params.spatial_sigma;
    let rs2 = 2.0 * params.range_sigma * params.range_sigma;

    var sum = vec3<f32>(0.0);
    var wsum = 0.0;

    for (var dy = -3; dy <= 3; dy++) {
        for (var dx = -3; dx <= 3; dx++) {
            let sp = clamp(pos + vec2<i32>(dx, dy), vec2<i32>(0),
                vec2<i32>(i32(params.width) - 1, i32(params.height) - 1));
            let s = textureLoad(input_tex, sp, 0);
            let sd = f32(dx * dx + dy * dy);
            let sw = exp(-sd / ss2);
            let rd = luma(s.rgb) - cl;
            let rw = exp(-(rd * rd) / rs2);
            let w = sw * rw;
            sum += s.rgb * w;
            wsum += w;
        }
    }

    let result = sum / max(wsum, 0.001);
    textureStore(output_tex, pos, vec4<f32>(result, center.a));
}
