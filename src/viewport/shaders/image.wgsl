struct Uniforms {
    view_proj: mat4x4<f32>,
    // New: Scale factor for the image (width, height)
    image_scale: vec2<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var t_diffuse: texture_2d<f32>;
@group(0) @binding(2) var s_diffuse: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    // A proper Quad (Rectangle) centered at 0,0
    // 2 Triangles, 6 Vertices total
    var pos = array<vec2<f32>, 6>(
        vec2<f32>(-0.5,  0.5), // Top-Left
        vec2<f32>(-0.5, -0.5), // Bottom-Left
        vec2<f32>( 0.5,  0.5), // Top-Right
        vec2<f32>( 0.5,  0.5), // Top-Right
        vec2<f32>(-0.5, -0.5), // Bottom-Left
        vec2<f32>( 0.5, -0.5)  // Bottom-Right
    );

    // UV Coordinates (0,0 is Top-Left in WGPU)
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), // TL
        vec2<f32>(0.0, 1.0), // BL
        vec2<f32>(1.0, 0.0), // TR
        vec2<f32>(1.0, 0.0), // TR
        vec2<f32>(0.0, 1.0), // BL
        vec2<f32>(1.0, 1.0)  // BR
    );

    var out: VertexOutput;

    // 1. Get base position
    let raw_pos = pos[in_vertex_index];

    // 2. Scale it by the image dimensions (aspect ratio correction)
    let scaled_pos = raw_pos * uniforms.image_scale;

    // 3. Apply Camera View/Projection
    out.position = uniforms.view_proj * vec4<f32>(scaled_pos, 0.0, 1.0);
    out.uv = uvs[in_vertex_index];

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(t_diffuse, s_diffuse, in.uv);
}
