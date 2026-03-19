struct Uniforms {
    scale: vec2<f32>,
    rotation: f32,
    zoom: f32,
    pan: vec2<f32>,
    tone_map_enabled: f32,
    hdr_output_enabled: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var t_diffuse: texture_2d<f32>;
@group(0) @binding(2) var s_diffuse: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn rotate2d(angle: f32) -> mat2x2<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return mat2x2<f32>(
        vec2<f32>(c, -s),
        vec2<f32>(s, c)
    );
}

fn aces_tonemap(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    var pos = array<vec2<f32>, 6>(
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0)
    );

    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0)
    );

    var out: VertexOutput;
    var p = pos[idx];

    p = p * u.scale;
    p = rotate2d(u.rotation) * p;
    p = p * u.zoom;
    p = p + u.pan;

    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(t_diffuse, s_diffuse, in.uv);

    if (u.tone_map_enabled > 0.5) {
        color = vec4<f32>(aces_tonemap(color.rgb), color.a);
    }

    // HDR output path placeholder:
    // once the export surface is negotiated to HDR-capable formats and
    // GdkColorState/HDR compositing is wired in, this branch can emit
    // scene-referred values instead of SDR-clamped output.
    if (u.hdr_output_enabled > 0.5) {
        return color;
    }

    return vec4<f32>(clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), color.a);
}
