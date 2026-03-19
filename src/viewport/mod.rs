use futures::channel::oneshot;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{GraphicsOffload, Picture};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use ash::vk as avk;

use crate::color::{ColorInfo, DynamicRange};
use crate::raw;

pub mod camera;
pub mod vk;

use camera::Camera;
use vk::compute::ProcessingPass;
use vk::context::VkContext;
use vk::renderer::VkRenderer;

// ── Decoded image types ───────────────────────────────────────────────────────

enum DecodedImage {
    Rgba8 {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        color: ColorInfo,
    },
    Rgba16 {
        data: Vec<u16>,
        width: u32,
        height: u32,
        color: ColorInfo,
    },
}

impl DecodedImage {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            DecodedImage::Rgba8 { width, height, .. } => (*width, *height),
            DecodedImage::Rgba16 { width, height, .. } => (*width, *height),
        }
    }

    fn dynamic_range(&self) -> DynamicRange {
        match self {
            DecodedImage::Rgba8 { color, .. } => color.dynamic_range,
            DecodedImage::Rgba16 { color, .. } => color.dynamic_range,
        }
    }
}

fn decode_standard_image(path: &Path) -> Option<DecodedImage> {
    let icc = crate::color::extract_icc_profile(path);
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();

    let rgba = crate::color::rgba8_to_srgb_with_icc(img.as_raw(), w, h, icc.as_deref());

    Some(DecodedImage::Rgba8 {
        rgba,
        width: w,
        height: h,
        color: ColorInfo {
            icc_profile: icc,
            dynamic_range: DynamicRange::Sdr,
        },
    })
}

fn decode_raw_image(path: &Path) -> Option<DecodedImage> {
    let raw_img = raw::decode_raw(path)?;
    Some(DecodedImage::Rgba16 {
        data: raw_img.data,
        width: raw_img.width,
        height: raw_img.height,
        color: raw_img.color,
    })
}

// ── Animation types ───────────────────────────────────────────────────────────

struct AnimFrame {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    delay: Duration,
}

enum AnimDecodeResult {
    Single {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    Animated {
        frames: Vec<AnimFrame>,
    },
}

struct AnimationState {
    frame_keys: Vec<PathBuf>,
    delays: Vec<Duration>,
    current_frame: usize,
}

// ── Viewport ──────────────────────────────────────────────────────────────────

pub struct Viewport {
    pub widget: gtk4::Box,
    picture: Picture,
    _offload: GraphicsOffload,
    camera: Rc<RefCell<Camera>>,
    renderer: Rc<RefCell<Option<VkRenderer>>>,
    current_target: Rc<RefCell<Option<PathBuf>>>,
    drag_start_x: Rc<Cell<f64>>,
    drag_start_y: Rc<Cell<f64>>,
    drag_cam_x: Rc<Cell<f32>>,
    drag_cam_y: Rc<Cell<f32>>,
    on_error: Rc<dyn Fn(String)>,
    animation: Rc<RefCell<Option<AnimationState>>>,
    anim_generation: Rc<Cell<u64>>,
}

impl Viewport {
    pub fn new(on_error: impl Fn(String) + 'static) -> Self {
        let on_error: Rc<dyn Fn(String)> = Rc::new(on_error);

        let renderer = try_init_vulkan(&on_error);
        let renderer = Rc::new(RefCell::new(renderer));

        let picture = Picture::builder()
            .hexpand(true)
            .vexpand(true)
            .content_fit(gtk4::ContentFit::Fill)
            .build();

        let offload = GraphicsOffload::builder()
            .child(&picture)
            .enabled(gtk4::GraphicsOffloadEnabled::Enabled)
            .build();

        let size_sensor = gtk4::DrawingArea::new();
        size_sensor.set_hexpand(true);
        size_sensor.set_vexpand(true);

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&offload));
        overlay.add_overlay(&size_sensor);
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.append(&overlay);

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

        // ── Automatic resize ──────────────────────────────────────────────────
        {
            let r2 = renderer.clone();
            let c2 = camera.clone();
            let p2 = picture.clone();
            size_sensor.connect_resize(move |_, new_w, new_h| {
                let new_w = new_w as u32;
                let new_h = new_h as u32;
                if new_w == 0 || new_h == 0 {
                    return;
                }
                let unchanged = {
                    let opt = r2.borrow();
                    match opt.as_ref() {
                        Some(r) => {
                            r.render_target_width() == new_w && r.render_target_height() == new_h
                        }
                        None => true,
                    }
                };
                if unchanged {
                    return;
                }
                let r3 = r2.clone();
                let c3 = c2.clone();
                let p3 = p2.clone();
                glib::idle_add_local_once(move || {
                    trigger_render(&r3, &c3, &p3);
                });
            });
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
            on_error,
            animation: Rc::new(RefCell::new(None)),
            anim_generation: Rc::new(Cell::new(0)),
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    pub fn load_image<F>(&self, path: PathBuf, on_dims: F)
    where
        F: FnOnce(u32, u32) + 'static,
    {
        *self.current_target.borrow_mut() = Some(path.clone());
        self.stop_animation();

        let has_vulkan = self.renderer.borrow().is_some();

        if has_vulkan && might_be_animated(&path) {
            self.load_animated_image(path, on_dims);
        } else if has_vulkan {
            self.load_image_vulkan(path, on_dims);
        } else {
            self.load_image_software(path, on_dims);
        }
    }

    pub fn prefetch(&self, path: PathBuf) {
        if might_be_animated(&path) || raw::is_raw(&path) {
            return;
        }

        let should_prefetch = {
            let opt = self.renderer.borrow();
            match opt.as_ref() {
                Some(r) => !r.is_cached(&path),
                None => false,
            }
        };
        if !should_prefetch {
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
            let mut opt = r2.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.cache_only(&path, rgba.as_raw(), w, h);
            }
        });
    }

    pub fn set_rotation(&self, degrees: f32) {
        self.camera.borrow_mut().set_rotation_degrees(degrees);
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn zoom_in(&self) {
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = (cam.zoom * 1.25).min(50.0);
        }
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn zoom_out(&self) {
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = (cam.zoom / 1.25).max(0.1);
        }
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn reset_view(&self) {
        {
            let mut cam = self.camera.borrow_mut();
            cam.zoom = 1.0;
            cam.position.x = 0.0;
            cam.position.y = 0.0;
        }
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn get_view_state(&self) -> (f32, f32, f32) {
        let cam = self.camera.borrow();
        (cam.zoom, cam.position.x, cam.position.y)
    }

    pub fn prepare_view(&self, zoom: f32, pos_x: f32, pos_y: f32) {
        let mut cam = self.camera.borrow_mut();
        cam.zoom = zoom;
        cam.position.x = pos_x;
        cam.position.y = pos_y;
    }

    pub fn toggle_enhance(&self) {
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.toggle_pass(ProcessingPass::Enhance);
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn toggle_sharpen(&self) {
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.toggle_pass(ProcessingPass::Sharpen);
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    pub fn toggle_denoise(&self) {
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.toggle_pass(ProcessingPass::Denoise);
                r.dirty = true;
            } else {
                return;
            }
        }
        trigger_render(&self.renderer, &self.camera, &self.picture);
    }

    // ── Private: stop animation ───────────────────────────────────────────────

    fn stop_animation(&self) {
        self.anim_generation
            .set(self.anim_generation.get().wrapping_add(1));
        *self.animation.borrow_mut() = None;
    }

    // ── Private: Vulkan load path (8-bit and 16-bit) ──────────────────────────

    fn load_image_vulkan<F>(&self, path: PathBuf, on_dims: F)
    where
        F: FnOnce(u32, u32) + 'static,
    {
        {
            let mut opt = self.renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                if let Some(dims) = r.activate_cached(&path) {
                    let (w, h) = (dims.0 as u32, dims.1 as u32);
                    r.dirty = true;
                    r.render(&self.camera.borrow());
                    drop(opt);
                    present_frame(&self.renderer, &self.picture);
                    on_dims(w, h);
                    return;
                }
            }
        }

        let is_raw_file = raw::is_raw(&path);

        let (tx, rx) = oneshot::channel::<Option<DecodedImage>>();
        let path_load = path.clone();
        rayon::spawn(move || {
            let result = if is_raw_file {
                decode_raw_image(&path_load)
            } else {
                decode_standard_image(&path_load)
            };
            let _ = tx.send(result);
        });

        let r2 = self.renderer.clone();
        let c2 = self.camera.clone();
        let p2 = self.picture.clone();
        let tracker = self.current_target.clone();

        glib::spawn_future_local(async move {
            let Some(decoded) = rx.await.ok().flatten() else {
                return;
            };

            let still_target = {
                let t = tracker.borrow();
                t.as_deref() == Some(path.as_path())
            };

            let (w, h) = decoded.dimensions();

            if !still_target {
                let mut opt = r2.borrow_mut();
                if let Some(ref mut r) = *opt {
                    match &decoded {
                        DecodedImage::Rgba8 {
                            rgba,
                            width,
                            height,
                            ..
                        } => {
                            r.cache_only(&path, rgba, *width, *height);
                        }
                        DecodedImage::Rgba16 {
                            data,
                            width,
                            height,
                            ..
                        } => {
                            r.cache_only_16bit(&path, data, *width, *height);
                        }
                    }
                }
                return;
            }

            {
                let mut opt = r2.borrow_mut();
                if let Some(ref mut r) = *opt {
                    match &decoded {
                        DecodedImage::Rgba8 {
                            rgba,
                            width,
                            height,
                            ..
                        } => {
                            r.upload_and_activate(&path, rgba, *width, *height);
                        }
                        DecodedImage::Rgba16 {
                            data,
                            width,
                            height,
                            ..
                        } => {
                            r.upload_and_activate_16bit(&path, data, *width, *height);
                        }
                    }
                    r.dirty = true;
                    r.render(&c2.borrow());
                }
            }

            present_frame(&r2, &p2);
            on_dims(w, h);
        });
    }

    // ── Private: animated image path ──────────────────────────────────────────

    fn load_animated_image<F>(&self, path: PathBuf, on_dims: F)
    where
        F: FnOnce(u32, u32) + 'static,
    {
        let (tx, rx) = oneshot::channel::<Option<AnimDecodeResult>>();
        let path_load = path.clone();
        rayon::spawn(move || {
            let _ = tx.send(decode_animated(&path_load));
        });

        let r2 = self.renderer.clone();
        let c2 = self.camera.clone();
        let p2 = self.picture.clone();
        let tracker = self.current_target.clone();
        let animation = self.animation.clone();
        let anim_gen = self.anim_generation.clone();

        glib::spawn_future_local(async move {
            let Some(result) = rx.await.ok().flatten() else {
                return;
            };

            let still_target = {
                let t = tracker.borrow();
                t.as_deref() == Some(path.as_path())
            };
            if !still_target {
                return;
            }

            match result {
                AnimDecodeResult::Single {
                    rgba,
                    width,
                    height,
                } => {
                    {
                        let mut opt = r2.borrow_mut();
                        if let Some(ref mut r) = *opt {
                            r.upload_and_activate(&path, &rgba, width, height);
                            r.dirty = true;
                            r.render(&c2.borrow());
                        }
                    }
                    present_frame(&r2, &p2);
                    on_dims(width, height);
                }
                AnimDecodeResult::Animated { frames } => {
                    if frames.is_empty() {
                        return;
                    }

                    let (w, h) = (frames[0].width, frames[0].height);
                    let mut frame_keys = Vec::with_capacity(frames.len());
                    let mut delays = Vec::with_capacity(frames.len());

                    {
                        let mut opt = r2.borrow_mut();
                        if let Some(ref mut r) = *opt {
                            for (i, frame) in frames.iter().enumerate() {
                                let key = PathBuf::from(format!("{}#frame{}", path.display(), i));
                                r.cache_only(&key, &frame.rgba, frame.width, frame.height);
                                frame_keys.push(key);
                                delays.push(frame.delay);
                            }

                            r.activate_cached(&frame_keys[0]);
                            r.dirty = true;
                            r.render(&c2.borrow());
                        }
                    }

                    present_frame(&r2, &p2);
                    on_dims(w, h);

                    let anim_id = anim_gen.get().wrapping_add(1);
                    anim_gen.set(anim_id);

                    *animation.borrow_mut() = Some(AnimationState {
                        frame_keys,
                        delays,
                        current_frame: 0,
                    });

                    schedule_animation_frame(r2, c2, p2, animation, anim_gen, anim_id);
                }
            }
        });
    }

    // ── Private: software fallback path ───────────────────────────────────────

    fn load_image_software<F>(&self, path: PathBuf, on_dims: F)
    where
        F: FnOnce(u32, u32) + 'static,
    {
        let is_raw_file = raw::is_raw(&path);

        let (tx, rx) = oneshot::channel::<Option<DecodedImage>>();
        let path_load = path.clone();
        rayon::spawn(move || {
            let result = if is_raw_file {
                decode_raw_image(&path_load)
            } else {
                decode_standard_image(&path_load)
            };
            let _ = tx.send(result);
        });

        let p2 = self.picture.clone();
        let tracker = self.current_target.clone();

        glib::spawn_future_local(async move {
            let Some(decoded) = rx.await.ok().flatten() else {
                return;
            };

            let still_target = {
                let t = tracker.borrow();
                t.as_deref() == Some(path.as_path())
            };
            if !still_target {
                return;
            }

            let (w, h) = decoded.dimensions();

            let rgba8 = match decoded {
                DecodedImage::Rgba8 { rgba, .. } => rgba,
                DecodedImage::Rgba16 {
                    data,
                    width,
                    height,
                    ..
                } => raw::linear_16_to_srgb_8(&data, width, height),
            };

            let stride = (w * 4) as usize;
            let bytes = glib::Bytes::from_owned(rgba8);
            let texture = gdk::MemoryTexture::new(
                w as i32,
                h as i32,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                stride,
            );
            p2.set_paintable(Some(&texture));
            on_dims(w, h);
        });
    }
}

// ── Animated image decode ─────────────────────────────────────────────────────

fn might_be_animated(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("gif")
    )
}

fn decode_animated(path: &Path) -> Option<AnimDecodeResult> {
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let decoder = match GifDecoder::new(reader) {
        Ok(d) => d,
        Err(_) => {
            let img = image::open(path).ok()?;
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            return Some(AnimDecodeResult::Single {
                rgba: rgba.into_raw(),
                width: w,
                height: h,
            });
        }
    };

    let frames: Vec<image::Frame> = match decoder.into_frames().collect_frames() {
        Ok(f) => f,
        Err(_) => {
            let img = image::open(path).ok()?;
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            return Some(AnimDecodeResult::Single {
                rgba: rgba.into_raw(),
                width: w,
                height: h,
            });
        }
    };

    if frames.len() <= 1 {
        let img = image::open(path).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        return Some(AnimDecodeResult::Single {
            rgba: rgba.into_raw(),
            width: w,
            height: h,
        });
    }

    let anim_frames: Vec<AnimFrame> = frames
        .into_iter()
        .map(|f| {
            let (numer, denom) = f.delay().numer_denom_ms();
            let delay_ms = (numer as u64) / (denom as u64).max(1);
            let delay = if delay_ms < 20 {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(delay_ms)
            };
            let buf = f.into_buffer();
            let (w, h) = buf.dimensions();
            AnimFrame {
                rgba: buf.into_raw(),
                width: w,
                height: h,
                delay,
            }
        })
        .collect();

    Some(AnimDecodeResult::Animated {
        frames: anim_frames,
    })
}

// ── Animation scheduler ───────────────────────────────────────────────────────

fn schedule_animation_frame(
    renderer: Rc<RefCell<Option<VkRenderer>>>,
    camera: Rc<RefCell<Camera>>,
    picture: Picture,
    animation: Rc<RefCell<Option<AnimationState>>>,
    generation: Rc<Cell<u64>>,
    anim_id: u64,
) {
    let delay = {
        let anim = animation.borrow();
        let Some(ref state) = *anim else { return };
        state.delays[state.current_frame]
    };

    glib::timeout_add_local_once(delay, move || {
        if generation.get() != anim_id {
            return;
        }

        let frame_key = {
            let mut anim = animation.borrow_mut();
            let Some(ref mut state) = *anim else { return };
            state.current_frame = (state.current_frame + 1) % state.frame_keys.len();
            state.frame_keys[state.current_frame].clone()
        };

        {
            let mut opt = renderer.borrow_mut();
            let Some(ref mut r) = *opt else { return };
            if r.activate_cached(&frame_key).is_none() {
                return;
            }
            r.dirty = true;
            r.render(&camera.borrow());
        }

        present_frame(&renderer, &picture);

        schedule_animation_frame(
            Rc::clone(&renderer),
            Rc::clone(&camera),
            picture.clone(),
            Rc::clone(&animation),
            Rc::clone(&generation),
            anim_id,
        );
    });
}

// ── Vulkan initialization ─────────────────────────────────────────────────────

fn try_init_vulkan(on_error: &Rc<dyn Fn(String)>) -> Option<VkRenderer> {
    let (vk_format, format_fourcc) = negotiate_dmabuf_format();

    let vk_context = match VkContext::new() {
        Ok(ctx) => ctx,
        Err(e) => {
            (on_error)(format!(
                "Vulkan unavailable: {}. Using software fallback.",
                e
            ));
            return None;
        }
    };

    match VkRenderer::new(vk_context, 1, 1, vk_format, format_fourcc) {
        Ok(r) => Some(r),
        Err(e) => {
            (on_error)(format!(
                "GPU renderer failed: {}. Using software fallback.",
                e
            ));
            None
        }
    }
}

// ── Module-level helpers ──────────────────────────────────────────────────────

fn sync_size(
    renderer: &Rc<RefCell<Option<VkRenderer>>>,
    camera: &Rc<RefCell<Camera>>,
    picture: &Picture,
) {
    let pw = picture.width() as u32;
    let ph = picture.height() as u32;
    if pw == 0 || ph == 0 {
        return;
    }

    let (current_w, current_h) = {
        let opt = renderer.borrow();
        let Some(ref r) = *opt else { return };
        (r.render_target_width(), r.render_target_height())
    };

    if pw != current_w || ph != current_h {
        {
            let mut opt = renderer.borrow_mut();
            if let Some(ref mut r) = *opt {
                r.resize(pw, ph);
            }
        }
        camera.borrow_mut().set_viewport_size(pw, ph);
    }
}

fn trigger_render(
    renderer: &Rc<RefCell<Option<VkRenderer>>>,
    camera: &Rc<RefCell<Camera>>,
    picture: &Picture,
) {
    sync_size(renderer, camera, picture);

    {
        let mut opt = renderer.borrow_mut();
        let Some(ref mut r) = *opt else { return };
        r.dirty = true;
        r.render(&camera.borrow());
    }

    present_frame(renderer, picture);
}

fn present_frame(renderer: &Rc<RefCell<Option<VkRenderer>>>, picture: &Picture) {
    let (fd, stride, fourcc, w, h) = {
        let opt = renderer.borrow();
        let Some(ref r) = *opt else { return };
        (
            r.export_fd_for_gtk(),
            r.render_target_stride(),
            r.render_target_fourcc(),
            r.render_target_width(),
            r.render_target_height(),
        )
    };

    if w == 0 || h == 0 {
        return;
    }

    let dmabuf_ok = if let Some(fd) = fd {
        let sync_fd = {
            let mut opt = renderer.borrow_mut();
            opt.as_mut().and_then(|r| r.take_sync_fd())
        };
        try_push_dmabuf(picture, w, h, fourcc, fd, stride, sync_fd)
    } else {
        false
    };

    if !dmabuf_ok {
        let pixels = {
            let opt = renderer.borrow();
            opt.as_ref().and_then(|r| r.read_pixels())
        };
        let stride_bytes = stride as usize;
        if let Some(pixels) = pixels {
            push_memory_texture(picture, w, h, stride_bytes, fourcc, &pixels);
        }
    }
}

fn try_push_dmabuf(
    picture: &Picture,
    width: u32,
    height: u32,
    fourcc: u32,
    fd: std::os::fd::RawFd,
    stride: u32,
    sync_fd: Option<std::os::fd::RawFd>,
) -> bool {
    let builder = gdk::DmabufTextureBuilder::new();
    builder.set_width(width);
    builder.set_height(height);
    builder.set_fourcc(fourcc);
    builder.set_modifier(0);
    builder.set_n_planes(1);
    builder.set_fd(0, fd);
    builder.set_stride(0, stride);
    builder.set_offset(0, 0);

    if let Some(sfd) = sync_fd {
        unsafe { libc::close(sfd) };
    }

    match unsafe { builder.build() } {
        Ok(texture) => {
            picture.set_paintable(Some(&texture));
            true
        }
        Err(e) => {
            eprintln!("[Iris] DmabufTexture build failed (X11 fallback active): {e}");
            false
        }
    }
}

fn push_memory_texture(
    picture: &Picture,
    width: u32,
    height: u32,
    stride: usize,
    fourcc: u32,
    pixels: &[u8],
) {
    let mem_format = fourcc_to_gdk_memory_format(fourcc);
    let bytes = glib::Bytes::from(pixels);
    let texture = gdk::MemoryTexture::new(width as i32, height as i32, mem_format, &bytes, stride);
    picture.set_paintable(Some(&texture));
}

// ── Format negotiation ────────────────────────────────────────────────────────

const FORMAT_PRIORITY: &[(u32, avk::Format)] = &[
    (0x34324241, avk::Format::R8G8B8A8_UNORM),
    (0x34325241, avk::Format::B8G8R8A8_UNORM),
    (0x34324258, avk::Format::R8G8B8A8_UNORM),
    (0x34325258, avk::Format::B8G8R8A8_UNORM),
];

fn negotiate_dmabuf_format() -> (avk::Format, u32) {
    let display = match gdk::Display::default() {
        Some(d) => d,
        None => {
            println!("[Iris] No GDK display; using fallback DMA-BUF format");
            return (avk::Format::R8G8B8A8_UNORM, 0x34324241);
        }
    };

    let formats = display.dmabuf_formats();
    let n = formats.n_formats();

    if n == 0 {
        println!("[Iris] Compositor reports no DMA-BUF formats; using fallback");
        return (avk::Format::R8G8B8A8_UNORM, 0x34324241);
    }

    use std::collections::HashSet;
    let compositor_fourccs: HashSet<u32> = (0..n)
        .filter_map(|i| {
            let (fourcc, modifier) = formats.format(i);
            if modifier == 0 { Some(fourcc) } else { None }
        })
        .collect();

    for &(fourcc, vk_fmt) in FORMAT_PRIORITY {
        if compositor_fourccs.contains(&fourcc) {
            println!("[Iris] Negotiated DMA-BUF format: fourcc=0x{fourcc:08x} vk={vk_fmt:?}");
            return (vk_fmt, fourcc);
        }
    }

    println!("[Iris] No preferred DMA-BUF format matched; using fallback");
    (avk::Format::R8G8B8A8_UNORM, 0x34324241)
}

fn fourcc_to_gdk_memory_format(fourcc: u32) -> gdk::MemoryFormat {
    match fourcc {
        0x34324241 | 0x34324258 => gdk::MemoryFormat::R8g8b8a8,
        0x34325241 | 0x34325258 => gdk::MemoryFormat::B8g8r8a8,
        _ => gdk::MemoryFormat::R8g8b8a8,
    }
}
