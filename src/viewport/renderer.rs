use super::camera::Camera;
use super::gpu::GpuContext;
use bytemuck::{Pod, Zeroable};
use image::GenericImageView;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use wgpu::*;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    // New: Width/Height of the image in world space
    image_scale: [f32; 2],
    // Padding to keep struct 16-byte aligned (WGPU requirement)
    padding: [f32; 2],
}

pub struct IrisRenderer {
    pub gpu: Arc<GpuContext>,
    pipeline: RenderPipeline,
    bind_group_layout: BindGroupLayout,
    bind_group: BindGroup,
    uniform_buffer: Buffer,
    sampler: Sampler,

    pub output_texture: Texture,
    pub output_buffer: Buffer,
    pub width: u32,
    pub height: u32,
    pub padded_bytes_per_row: u32,

    // Current image dimensions
    pub image_dims: (f32, f32),
}

impl IrisRenderer {
    pub fn new(gpu: Arc<GpuContext>, width: u32, height: u32) -> Self {
        let shader = gpu.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Iris Shader"),
            source: ShaderSource::Wgsl(include_str!("./shaders/image.wgsl").into()),
        });

        // 1. Uniforms
        let uniform_buffer = gpu.device.create_buffer(&BufferDescriptor {
            label: Some("Uniform Buffer"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 2. Sampler
        let sampler = gpu.device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        // 3. Layout
        let bind_group_layout = gpu
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("Main Layout"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::VERTEX,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            multisampled: false,
                            view_dimension: TextureViewDimension::D2,
                            sample_type: TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        // 4. Default Texture
        let default_tex = gpu.device.create_texture_with_data(
            &gpu.queue,
            &TextureDescriptor {
                label: Some("Default Texture"),
                size: Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Rgba8UnormSrgb,
                usage: TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::MipMajor,
            &[255, 255, 255, 255],
        );
        let default_view = default_tex.create_view(&TextureViewDescriptor::default());

        let bind_group = gpu.device.create_bind_group(&BindGroupDescriptor {
            label: Some("Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&default_view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
        });

        // 5. Pipeline
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("Pipeline Layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = gpu
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("Render Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                    compilation_options: PipelineCompilationOptions::default(),
                },
                fragment: Some(FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(ColorTargetState {
                        format: gpu.texture_format,
                        blend: Some(BlendState::ALPHA_BLENDING),
                        write_mask: ColorWrites::ALL,
                    })],
                    compilation_options: PipelineCompilationOptions::default(),
                }),
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let (output_texture, output_buffer, padded) = Self::create_targets(&gpu, width, height);

        Self {
            gpu,
            pipeline,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            sampler,
            output_texture,
            output_buffer,
            width,
            height,
            padded_bytes_per_row: padded,
            image_dims: (1.0, 1.0),
        }
    }

    pub fn load_image(&mut self, img: &image::DynamicImage) {
        let rgba = img.to_rgba8();
        let (w, h) = img.dimensions();

        // Save dimensions to fix aspect ratio
        self.image_dims = (w as f32, h as f32);

        let texture_size = Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let texture = self.gpu.device.create_texture(&TextureDescriptor {
            label: Some("Image Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        self.gpu.queue.write_texture(
            ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &rgba,
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            texture_size,
        );

        let view = texture.create_view(&TextureViewDescriptor::default());

        self.bind_group = self.gpu.device.create_bind_group(&BindGroupDescriptor {
            label: Some("Image Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    fn create_targets(gpu: &GpuContext, width: u32, height: u32) -> (Texture, Buffer, u32) {
        let texture_size = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = gpu.device.create_texture(&TextureDescriptor {
            label: Some("Output Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: gpu.texture_format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let unpadded = width * 4;
        let align = 256;
        let padding = (align - unpadded % align) % align;
        let padded = unpadded + padding;

        let buffer = gpu.device.create_buffer(&BufferDescriptor {
            label: Some("Output Buffer"),
            size: (padded * height) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        (texture, buffer, padded)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 && (width != self.width || height != self.height) {
            self.width = width;
            self.height = height;
            let (tex, buf, padded) = Self::create_targets(&self.gpu, width, height);
            self.output_texture = tex;
            self.output_buffer = buf;
            self.padded_bytes_per_row = padded;
        }
    }

    pub fn render(&mut self, camera: &Camera) {
        // Calculate Aspect Ratio Correction
        // We want the image to maintain its native ratio
        // We normalize it so the longest side is 1.0 (or keep it native size, but let's do 1:1 units first)

        let aspect = self.image_dims.0 / self.image_dims.1;

        // This makes the quad match the image shape
        let scale = if aspect > 1.0 {
            [1.0, 1.0 / aspect] // Wide image
        } else {
            [aspect, 1.0] // Tall image
        };

        let uniforms = Uniforms {
            view_proj: camera.build_view_projection_matrix().to_cols_array_2d(),
            image_scale: scale,
            padding: [0.0; 2],
        };

        self.gpu
            .queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Encoder"),
            });
        let view = self
            .output_texture
            .create_view(&TextureViewDescriptor::default());

        {
            let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.051,
                            g: 0.051,
                            b: 0.051,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &self.bind_group, &[]);
            // FIX: Draw 6 vertices for the Quad (not 3)
            rpass.draw(0..6, 0..1);
        }

        encoder.copy_texture_to_buffer(
            ImageCopyTexture {
                texture: &self.output_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            ImageCopyBuffer {
                buffer: &self.output_buffer,
                layout: ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        self.gpu.queue.submit(Some(encoder.finish()));
    }
}
