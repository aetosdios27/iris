use super::camera::Camera;
use super::gpu::GpuContext;
use bytemuck::{Pod, Zeroable};
use image::GenericImageView;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use wgpu::*;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Uniforms {
    scale: [f32; 2],
    rotation: f32,
    zoom: f32,
    pan: [f32; 2],
    _padding: [f32; 2],
}

struct CachedImage {
    dims: (f32, f32),
    texture: Texture,
    memory_bytes: u64,
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

    pub image_dims: (f32, f32),
    pub dirty: bool,

    cache: HashMap<PathBuf, CachedImage>,
    cache_order: Vec<PathBuf>,
    cache_memory_used: u64,
    cache_memory_budget: u64,
}

impl IrisRenderer {
    pub fn new(gpu: Arc<GpuContext>, width: u32, height: u32) -> Self {
        let shader = gpu.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Iris Shader"),
            source: ShaderSource::Wgsl(include_str!("./shaders/image.wgsl").into()),
        });

        let uniform_buffer = gpu.device.create_buffer(&BufferDescriptor {
            label: Some("Uniform Buffer"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = gpu.device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = gpu
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("Main Layout"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
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
            &[0, 0, 0, 255],
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
        let cache_memory_budget = gpu.cache_budget;

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
            dirty: true,
            cache: HashMap::new(),
            cache_order: Vec::new(),
            cache_memory_used: 0,
            cache_memory_budget,
        }
    }

    /// Downscale if exceeds GPU limits
    fn fit_to_gpu_limits(&self, rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
        let max = self.gpu.max_texture_size;

        if w <= max && h <= max {
            return (rgba.to_vec(), w, h);
        }

        let scale = (max as f32 / w as f32).min(max as f32 / h as f32);
        let new_w = ((w as f32 * scale) as u32).max(1);
        let new_h = ((h as f32 * scale) as u32).max(1);

        println!(
            "[resize] {}×{} exceeds GPU limit {}px, downscaling to {}×{}",
            w, h, max, new_w, new_h
        );

        let img_buf =
            image::RgbaImage::from_raw(w, h, rgba.to_vec()).expect("Failed to create image buffer");
        let resized = image::imageops::resize(
            &img_buf,
            new_w,
            new_h,
            image::imageops::FilterType::Lanczos3,
        );

        (resized.into_raw(), new_w, new_h)
    }

    /// Activate a cached image for display. Returns dims if found.
    pub fn activate_cached(&mut self, path: &Path) -> Option<(f32, f32)> {
        if let Some(cached) = self.cache.get(path) {
            self.image_dims = cached.dims;

            let view = cached
                .texture
                .create_view(&TextureViewDescriptor::default());
            self.bind_group = self.gpu.device.create_bind_group(&BindGroupDescriptor {
                label: Some("Cached Bind Group"),
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
            self.dirty = true;

            self.cache_order.retain(|p| p != path);
            self.cache_order.insert(0, path.to_owned());

            Some(cached.dims)
        } else {
            None
        }
    }

    /// Cache-only: upload texture to GPU cache, do NOT change bind_group or image_dims.
    /// Used by prefetch so it never affects current display.
    pub fn cache_only(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) {
        let (final_rgba, final_w, final_h) = self.fit_to_gpu_limits(rgba, w, h);
        let mem = (final_w as u64) * (final_h as u64) * 4;

        let texture_size = Extent3d {
            width: final_w,
            height: final_h,
            depth_or_array_layers: 1,
        };

        let texture = self.gpu.device.create_texture(&TextureDescriptor {
            label: Some("Cached Texture"),
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
            &final_rgba,
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * final_w),
                rows_per_image: Some(final_h),
            },
            texture_size,
        );

        // Cache management — evict if needed
        let path_buf = path.to_owned();
        if let Some(old) = self.cache.remove(&path_buf) {
            self.cache_memory_used = self.cache_memory_used.saturating_sub(old.memory_bytes);
            self.cache_order.retain(|p| p != &path_buf);
        }

        while self.cache_memory_used + mem > self.cache_memory_budget
            && !self.cache_order.is_empty()
        {
            if let Some(oldest) = self.cache_order.pop() {
                if let Some(evicted) = self.cache.remove(&oldest) {
                    self.cache_memory_used =
                        self.cache_memory_used.saturating_sub(evicted.memory_bytes);
                }
            }
        }

        self.cache_order.insert(0, path_buf.clone());
        self.cache_memory_used += mem;
        self.cache.insert(
            path_buf,
            CachedImage {
                dims: (w as f32, h as f32), // Original dims
                texture,
                memory_bytes: mem,
            },
        );

        // NOTE: bind_group, image_dims, dirty are NOT modified
    }

    /// Upload, cache, AND activate for display.
    /// Used when loading the current image (not prefetch).
    pub fn upload_and_activate(&mut self, path: &Path, rgba: &[u8], w: u32, h: u32) {
        // First cache it
        self.cache_only(path, rgba, w, h);
        // Then activate it (sets bind_group, image_dims, dirty)
        self.activate_cached(path);
    }

    pub fn is_cached(&self, path: &Path) -> bool {
        self.cache.contains_key(path)
    }

    pub fn cache_stats(&self) -> (usize, u64, u64) {
        (
            self.cache.len(),
            self.cache_memory_used,
            self.cache_memory_budget,
        )
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
            self.dirty = true;
        }
    }

    pub fn render(&mut self, camera: &Camera) {
        let scale = camera.fit_scale(self.image_dims.0, self.image_dims.1);

        let uniforms = Uniforms {
            scale,
            rotation: camera.rotation,
            zoom: camera.zoom,
            pan: [camera.position.x, camera.position.y],
            _padding: [0.0; 2],
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
        self.dirty = false;
    }
}
