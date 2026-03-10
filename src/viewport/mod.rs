use futures::channel::oneshot;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{GraphicsOffload, Picture};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

pub mod camera;
pub mod vk;

use camera::Camera;
use vk::context::VkContext;
use vk::renderer::VkRenderer;

pub struct Viewport {
    pub widget: gtk4::Box,
    picture: Picture,
    _offload: GraphicsOffload,
    camera: Rc<RefCell<Camera>>,
    renderer: Rc<RefCell<VkRenderer>>,
    current_target: Rc<RefCell<Option<PathBuf>>>,
    drag_start_x: Rc<Cell<f64>>,
    drag_start_y: Rc<Cell<f64>>,
    drag_cam_x: Rc<Cell<f32>>,
    drag_cam_y: Rc<Cell<f32>>,
}

impl Viewport {
    pub fn new() -> Self {
        // ── Vulkan init ───────────────────────────────────────────────────────
        let vk_context = VkContext::new();
        let renderer = Rc::new(RefCell::new(VkRenderer::new(vk_context, 1, 1)));

        // ── GTK widgets ───────────────────────────────────────────────────────
        let picture = Picture::builder()
            .hexpand(true)
            .vexpand(true)
            .content_fit(gtk4::ContentFit::Fill)
            .build();

        let offload = GraphicsOffload::builder()
            .child(&picture)
            .enabled(gtk4::GraphicsOffloadEnabled::Enabled)
            .build();

        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.append(&offload);

        let camera = Rc::new(RefCell::new(Camera::new()));
        let current_target: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));

        let drag_start_x = Rc::new(Cell::new(0.0f64));
        let drag_start_y = Rc::new(Cell::new(0.0f64));
        let drag_cam_x = Rc::new(Cell::new(0.0f32));
        let drag_cam_y = Rc::new(Cell::new(0.0f32));

        // ── Scroll zoom ───────────────────────────────────────────────────────
        {
            let sc = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
            let r2 = renderer.clone();
            let c2 = camera.clone();
            let p2 = picture.clone();
            sc.connect_scroll(move |_, _, dy| {
                {
                    let mut cam = c2.borrow_mut();
                    if dy > 0.0 {
                        cam.zoom = (cam.zoom / 1.15).max(0.1);
                    } else {
                        cam.zoom = (cam.zoom * 1.15).min(50.0);
                    }
                }
                trigger_render(&r2, &c2, &p2);
                glib::Propagation::Stop
            });
            widget.add_controller(sc);
        }

        // ── Drag pan ──────────────────────────────────────────────────────────
        {
            let dc = gtk4::GestureDrag::new();
            dc.set_button(1);

            let cb = camera.clone();
            let dsx = drag_start_x.clone();
            let dsy = drag_start_y.clone();
            let dcx = drag_cam_x.clone();
            let dcy = drag_cam_y.clone();
            dc.connect_drag_begin(move |_, x, y| {
                let cam = cb.borrow();
                dsx.set(x);
                dsy.set(y);
                dcx.set(cam.position.x);
                dcy.set(cam.position.y);
            });

            let cu = camera.clone();
            let r2 = renderer.clone();
            let p2 = picture.clone();
            let dcx2 = drag_cam_x.clone();
            let dcy2 = drag_cam_y.clone();
            dc.connect_drag_update(move |_, dx, dy| {
                {
                    let mut cam = cu.borrow_mut();
                    let vw = cam.viewport_width as f32;
                    let vh = cam.viewport_height as f32;
                    if vw > 0.0 && vh > 0.0 {
                        cam.position.x = dcx2.get() - (dx as f32 / vw) * 2.0 / cam.zoom;
                        cam.position.y = dcy2.get() + (dy as f32 / vh) * 2.0 / cam.zoom;
                    }
                }
                trigger_render(&r2, &cu, &p2);
            });

            widget.add_controller(dc);
        }

        // ── Double-click reset ────────────────────────────────────────────────
        {
            let cc = gtk4::GestureClick::new();
            cc.set_button(1);
            let c2 = camera.clone();
            let r2 = renderer.clone();
            let p2 = picture.clone();
            cc.connect_released(move |_, n, _, _| {
                if n == 2 {
                    {
                        let mut cam = c2.borrow_mut();
                        cam.zoom = 1.0;
                        cam.position.x = 0.0;
                        cam.position.y = 0.0;
                    }
                    trigger_render(&r2, &c2, &p2);
                }
            });
            widget.add_controller(cc);
        }

        Self {
            widget,
            picture,
            _offload: offload,
            camera,
            renderer,
            current_target,
            drag_start_x,
            drag_start_y,
            drag_cam_x,
            drag_cam_y,
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    pub fn load_image<F>(&self, path: PathBuf, on_dims: F)
    where
        F: FnOnce(u32, u32) + 'static,
    {
        *self.current_target.borrow_mut() = Some(path.clone());

        // Reset camera on every new image
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = 1.0;
            cam.position.x = 0.0;
            cam.position.y = 0.0;
        }

        // ── Cache hit ─────────────────────────────────────────────────────────
        {
            let mut r = self.renderer.borrow_mut();
            if let Some(dims) = r.activate_cached(&path) {
                let (w, h) = (dims.0 as u32, dims.1 as u32);
                r.dirty = true;
                r.render(&self.camera.borrow());
                let fd = r.export_fd_for_gtk();
                let stride = r.render_target_stride();
                let fourcc = r.render_target_fourcc();
                let rw = r.render_target.width;
                let rh = r.render_target.height;
                drop(r);
                push_dmabuf_to_picture(&self.picture, rw, rh, fourcc, fd, stride);
                on_dims(w, h);
                return;
            }
        }

        // ── Cache miss: decode on rayon thread, upload on GTK main thread ─────
        let (tx, rx) = oneshot::channel::<Result<image::DynamicImage, image::ImageError>>();
        let path_load = path.clone();
        rayon::spawn(move || {
            let _ = tx.send(image::open(&path_load));
        });

        let r2 = self.renderer.clone();
        let c2 = self.camera.clone();
        let p2 = self.picture.clone();
        let tracker = self.current_target.clone();

        glib::spawn_future_local(async move {
            let Ok(Ok(img)) = rx.await else { return };

            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());

            // If the user navigated away while decoding, cache silently
            let still_target = {
                let t = tracker.borrow();
                t.as_deref() == Some(path.as_path())
            };

            if !still_target {
                r2.borrow_mut().cache_only(&path, rgba.as_raw(), w, h);
                return;
            }

            let (fd, stride, fourcc, rw, rh) = {
                let mut r = r2.borrow_mut();
                r.upload_and_activate(&path, rgba.as_raw(), w, h);
                r.dirty = true;
                r.render(&c2.borrow());
                let fd = r.export_fd_for_gtk();
                let stride = r.render_target_stride();
                let fourcc = r.render_target_fourcc();
                let rw = r.render_target.width;
                let rh = r.render_target.height;
                (fd, stride, fourcc, rw, rh)
            };

            push_dmabuf_to_picture(&p2, rw, rh, fourcc, fd, stride);
            on_dims(w, h);
        });
    }

    pub fn prefetch(&self, path: PathBuf) {
        if self.renderer.borrow().is_cached(&path) {
            return;
        }

        let (tx, rx) = oneshot::channel::<Result<image::DynamicImage, image::ImageError>>();
        let path_load = path.clone();
        rayon::spawn(move || {
            let _ = tx.send(image::open(&path_load));
        });

        let r2 = self.renderer.clone();
        glib::spawn_future_local(async move {
            let Ok(Ok(img)) = rx.await else { return };
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            r2.borrow_mut().cache_only(&path, rgba.as_raw(), w, h);
        });
    }

    pub fn set_rotation(&self, degrees: f32) {
        self.camera.borrow_mut().set_rotation_degrees(degrees);
        self.renderer.borrow_mut().dirty = true;
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn zoom_in(&self) {
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = (cam.zoom * 1.25).min(50.0);
        }
        self.renderer.borrow_mut().dirty = true;
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn zoom_out(&self) {
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = (cam.zoom / 1.25).max(0.1);
        }
        self.renderer.borrow_mut().dirty = true;
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn reset_view(&self) {
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = 1.0;
            cam.position.x = 0.0;
            cam.position.y = 0.0;
        }
        self.renderer.borrow_mut().dirty = true;
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }
}

// ── Module-level helpers ──────────────────────────────────────────────────────

/// Check whether the picture widget has been resized since the last render,
/// and if so resize the Vulkan render target to match before rendering.
fn sync_size(renderer: &Rc<RefCell<VkRenderer>>, camera: &Rc<RefCell<Camera>>, picture: &Picture) {
    let pw = picture.width() as u32;
    let ph = picture.height() as u32;
    if pw == 0 || ph == 0 {
        return;
    }
    let current_w = renderer.borrow().render_target.width;
    let current_h = renderer.borrow().render_target.height;
    if pw != current_w || ph != current_h {
        renderer.borrow_mut().resize(pw, ph);
        camera.borrow_mut().set_viewport_size(pw, ph);
    }
}

/// Render a frame and push the result DmabufTexture to the GTK Picture.
fn trigger_render(
    renderer: &Rc<RefCell<VkRenderer>>,
    camera: &Rc<RefCell<Camera>>,
    picture: &Picture,
) {
    sync_size(renderer, camera, picture);

    let mut r = renderer.borrow_mut();
    r.dirty = true;
    r.render(&camera.borrow());
    let fd = r.export_fd_for_gtk();
    let stride = r.render_target_stride();
    let fourcc = r.render_target_fourcc();
    let w = r.render_target.width;
    let h = r.render_target.height;
    drop(r);
    push_dmabuf_to_picture(picture, w, h, fourcc, fd, stride);
}

/// Build a `GdkDmabufTexture` and set it on the `Picture`.
/// GTK takes ownership of `fd`; we always pass a freshly duplicated fd.
fn push_dmabuf_to_picture(
    picture: &Picture,
    width: u32,
    height: u32,
    fourcc: u32,
    fd: std::os::fd::RawFd,
    stride: u32,
) {
    if width == 0 || height == 0 {
        return;
    }

    let builder = gtk4::gdk::DmabufTextureBuilder::new();
    builder.set_width(width);
    builder.set_height(height);
    builder.set_fourcc(fourcc);
    builder.set_modifier(0); // DRM_FORMAT_MOD_LINEAR
    builder.set_n_planes(1);
    builder.set_fd(0, fd);
    builder.set_stride(0, stride);
    builder.set_offset(0, 0);

    match unsafe { builder.build() } {
        Ok(texture) => picture.set_paintable(Some(&texture)),
        Err(e) => eprintln!("[Iris] DmabufTexture build failed: {e}"),
    }
}
