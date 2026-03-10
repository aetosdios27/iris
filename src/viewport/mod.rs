use gtk4::prelude::*;
use gtk4::{Picture, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub mod camera;
pub mod gpu;
pub mod renderer;

use camera::Camera;
use gpu::GpuContext;
use renderer::IrisRenderer;

pub struct Viewport {
    pub widget: Picture,
    camera: Rc<RefCell<Camera>>,
    renderer: Rc<RefCell<Option<IrisRenderer>>>,
}

impl Viewport {
    pub fn new() -> Self {
        let widget = Picture::builder()
            .hexpand(true)
            .vexpand(true)
            .content_fit(gtk4::ContentFit::Fill)
            .build();

        let camera = Rc::new(RefCell::new(Camera::new()));
        let renderer: Rc<RefCell<Option<IrisRenderer>>> = Rc::new(RefCell::new(None));

        let r_init = renderer.clone();
        glib::spawn_future_local(async move {
            let gpu = Arc::new(GpuContext::new().await);
            let new_renderer = IrisRenderer::new(gpu, 1, 1);
            *r_init.borrow_mut() = Some(new_renderer);
        });

        let r_tick = renderer.clone();
        let c_tick = camera.clone();

        widget.add_tick_callback(move |pic, _clock| {
            let mut r_ref = r_tick.borrow_mut();

            if let Some(renderer) = r_ref.as_mut() {
                let current_width = pic.width().max(1) as u32;
                let current_height = pic.height().max(1) as u32;

                if renderer.width != current_width || renderer.height != current_height {
                    renderer.resize(current_width, current_height);
                    c_tick
                        .borrow_mut()
                        .set_viewport_size(current_width, current_height);
                    renderer.dirty = true;
                }

                if !renderer.dirty {
                    return glib::ControlFlow::Continue;
                }

                renderer.render(&c_tick.borrow());

                let buffer_slice = renderer.output_buffer.slice(..);
                let (sender, receiver) = futures::channel::oneshot::channel();

                buffer_slice.map_async(wgpu::MapMode::Read, move |v| {
                    let _ = sender.send(v);
                });
                renderer.gpu.device.poll(wgpu::Maintain::Wait);

                if let Ok(Ok(())) = futures::executor::block_on(receiver) {
                    let data = buffer_slice.get_mapped_range();

                    let bytes = glib::Bytes::from(&*data);
                    let texture = gtk4::gdk::MemoryTexture::new(
                        renderer.width as i32,
                        renderer.height as i32,
                        gtk4::gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        renderer.padded_bytes_per_row as usize,
                    );

                    pic.set_paintable(Some(&texture));

                    drop(data);
                    renderer.output_buffer.unmap();
                }
            }
            glib::ControlFlow::Continue
        });

        Self {
            widget,
            camera,
            renderer,
        }
    }

    /// Load image — instant if cached, CPU fast path + GPU upload if not
    pub fn load_image<F>(&self, path: PathBuf, on_dims: F)
    where
        F: FnOnce(u32, u32) + 'static,
    {
        // Tier 1: GPU cache hit — instant
        {
            let mut r = self.renderer.borrow_mut();
            if let Some(renderer) = r.as_mut() {
                if let Some(dims) = renderer.activate_cached(&path) {
                    let (count, used, budget) = renderer.cache_stats();
                    println!(
                        "[cache HIT] {} images, {:.0}MB / {:.0}MB",
                        count,
                        used as f64 / 1_048_576.0,
                        budget as f64 / 1_048_576.0
                    );
                    on_dims(dims.0 as u32, dims.1 as u32);
                    return;
                }
            }
        }

        // Tier 2: Cache miss — decode, CPU display, then GPU upload+activate
        println!(
            "[cache MISS] decoding {:?}",
            path.file_name().unwrap_or_default()
        );

        let (sender, receiver) = futures::channel::oneshot::channel();
        let r_load = self.renderer.clone();
        let path_cache = path.clone();
        let widget = self.widget.clone();

        std::thread::spawn(move || {
            let img = image::open(&path);
            let _ = sender.send(img);
        });

        glib::spawn_future_local(async move {
            if let Ok(Ok(image)) = receiver.await {
                let rgba = image.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());

                // CPU fast path — display immediately
                let bytes = glib::Bytes::from(rgba.as_raw().as_slice());
                let cpu_texture = gtk4::gdk::MemoryTexture::new(
                    w as i32,
                    h as i32,
                    gtk4::gdk::MemoryFormat::R8g8b8a8,
                    &bytes,
                    (w * 4) as usize,
                );
                widget.set_paintable(Some(&cpu_texture));

                // GPU upload AND activate — this sets bind_group
                if let Some(renderer) = r_load.borrow_mut().as_mut() {
                    renderer.upload_and_activate(&path_cache, rgba.as_raw(), w, h);
                }

                on_dims(w, h);
            }
        });
    }

    /// Prefetch into GPU cache without displaying — never touches bind_group
    pub fn prefetch(&self, path: PathBuf) {
        {
            let r = self.renderer.borrow();
            if let Some(renderer) = r.as_ref() {
                if renderer.is_cached(&path) {
                    return;
                }
            }
        }

        let r_load = self.renderer.clone();
        let path_cache = path.clone();

        let (sender, receiver) = futures::channel::oneshot::channel();

        std::thread::spawn(move || {
            let img = image::open(&path);
            let _ = sender.send(img);
        });

        glib::spawn_future_local(async move {
            if let Ok(Ok(image)) = receiver.await {
                let rgba = image.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());

                if let Some(renderer) = r_load.borrow_mut().as_mut() {
                    // Cache only — does NOT modify bind_group, image_dims, or dirty
                    renderer.cache_only(&path_cache, rgba.as_raw(), w, h);
                }
            }
        });
    }

    pub fn set_rotation(&self, degrees: f32) {
        self.camera.borrow_mut().set_rotation_degrees(degrees);
        if let Some(renderer) = self.renderer.borrow_mut().as_mut() {
            renderer.dirty = true;
        }
    }
}
