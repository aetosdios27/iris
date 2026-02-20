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
            .can_focus(true)
            .content_fit(gtk4::ContentFit::Fill)
            .build();

        let camera = Rc::new(RefCell::new(Camera::new()));
        let renderer: Rc<RefCell<Option<IrisRenderer>>> = Rc::new(RefCell::new(None));

        // 1. Initialize GPU
        let r_init = renderer.clone();
        glib::spawn_future_local(async move {
            let gpu = Arc::new(GpuContext::new().await);
            let new_renderer = IrisRenderer::new(gpu, 1, 1);
            *r_init.borrow_mut() = Some(new_renderer);
        });

        // 2. The Game Loop
        let r_tick = renderer.clone();
        let c_tick = camera.clone();

        widget.add_tick_callback(move |pic, _clock| {
            let mut r_ref = r_tick.borrow_mut();

            if let Some(renderer) = r_ref.as_mut() {
                // A. Resize Logic
                let current_width = pic.width().max(1) as u32;
                let current_height = pic.height().max(1) as u32;

                if renderer.width != current_width || renderer.height != current_height {
                    renderer.resize(current_width, current_height);
                    c_tick
                        .borrow_mut()
                        .set_viewport_size(current_width, current_height);
                }

                // B. Render Frame
                renderer.render(&c_tick.borrow());

                // C. Readback
                let buffer_slice = renderer.output_buffer.slice(..);
                let (sender, receiver) = futures::channel::oneshot::channel();

                buffer_slice.map_async(wgpu::MapMode::Read, move |v| sender.send(v).unwrap());
                renderer.gpu.device.poll(wgpu::Maintain::Wait);

                if let Ok(_) = futures::executor::block_on(receiver) {
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

    // NEW: Public API to load an image using Futures
    pub fn load_image(&self, path: PathBuf) {
        // 1. Create a oneshot channel
        let (sender, receiver) = futures::channel::oneshot::channel();
        let r_load = self.renderer.clone();

        // 2. Spawn background thread for heavy IO/Decoding
        std::thread::spawn(move || {
            let img = image::open(&path);
            // Send the result to the future
            let _ = sender.send(img);
        });

        // 3. Spawn a LOCAL future (Main Thread) to await the result
        // Because this is 'local', we can capture 'r_load' (Rc) safely.
        glib::spawn_future_local(async move {
            // Wait for the thread to finish
            if let Ok(load_result) = receiver.await {
                if let Ok(image) = load_result {
                    if let Some(renderer) = r_load.borrow_mut().as_mut() {
                        // Upload to GPU (on Main Thread)
                        renderer.load_image(&image);
                    }
                } else {
                    eprintln!("Failed to decode image");
                }
            }
        });
    }
}
