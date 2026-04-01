#![allow(unsafe_op_in_unsafe_fn)]

use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{FileDialog, Orientation, glib};
use image::GenericImageView;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

mod color;
mod config;
mod error;
mod raw;
mod thumbcache;
mod viewport;

use config::Config;

const APP_ID: &str = "dev.iris.viewer";

fn read_exif_rotation(path: &Path) -> i32 {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    let mut buf = std::io::BufReader::new(file);
    let exif = match exif::Reader::new().read_from_container(&mut buf) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    match exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
        Some(field) => match field.value.get_uint(0) {
            Some(1) => 0,
            Some(3) => 180,
            Some(6) => 90,
            Some(8) => 270,
            _ => 0,
        },
        None => 0,
    }
}

#[derive(Clone, Copy)]
struct ViewState {
    zoom: f32,
    position_x: f32,
    position_y: f32,
}

struct AppState {
    files: Vec<PathBuf>,
    current_index: usize,
    rotations: HashMap<PathBuf, i32>,
    view_states: HashMap<PathBuf, ViewState>,
    info_visible: bool,
    watched_directory: Option<PathBuf>,
    /// +1 when navigating forward, -1 backward, 0 neutral.
    /// Used to bias prefetch in the direction the user is scrubbing.
    last_nav_direction: i32,
}

impl AppState {
    fn new() -> Self {
        Self {
            files: vec![],
            current_index: 0,
            rotations: HashMap::new(),
            view_states: HashMap::new(),
            info_visible: false,
            watched_directory: None,
            last_nav_direction: 0,
        }
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.files.get(self.current_index).cloned()
    }

    fn current_rotation(&self) -> i32 {
        self.current_path()
            .and_then(|p| self.rotations.get(&p).copied())
            .unwrap_or(0)
    }

    fn rotate_cw(&mut self) {
        if let Some(path) = self.current_path() {
            let r = self.rotations.entry(path).or_insert(0);
            *r = (*r + 90) % 360;
        }
    }

    fn rotate_ccw(&mut self) {
        if let Some(path) = self.current_path() {
            let r = self.rotations.entry(path).or_insert(0);
            *r = (*r + 270) % 360;
        }
    }

    fn load_directory(&mut self, path: &PathBuf) {
        if let Some(parent) = path.parent() {
            let mut files = Self::scan_images(parent);
            files.sort();
            self.current_index = files.iter().position(|f| f == path).unwrap_or(0);
            self.files = files;
            self.watched_directory = Some(parent.to_path_buf());
            self.last_nav_direction = 0;
        }
    }

    fn load_from_directory(&mut self, dir: &Path) {
        let mut files = Self::scan_images(dir);
        files.sort();
        self.current_index = 0;
        self.files = files;
        self.watched_directory = Some(dir.to_path_buf());
        self.last_nav_direction = 0;
    }

    fn refresh_watched_directory(&mut self) -> Option<PathBuf> {
        let dir = self.watched_directory.clone()?;
        let old_current = self.current_path();
        let mut files = Self::scan_images(&dir);
        files.sort();

        if files.is_empty() {
            self.files.clear();
            self.current_index = 0;
            return None;
        }

        let new_current = if let Some(old) = old_current {
            if let Some(idx) = files.iter().position(|f| *f == old) {
                self.current_index = idx;
                old
            } else {
                self.current_index = self.current_index.min(files.len() - 1);
                files[self.current_index].clone()
            }
        } else {
            self.current_index = self.current_index.min(files.len() - 1);
            files[self.current_index].clone()
        };

        self.files = files;
        Some(new_current)
    }

    fn scan_images(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return vec![];
        };
        entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let is_standard = matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "tiff" | "bmp")
                );
                is_standard || crate::raw::is_raw(p)
            })
            .collect()
    }

    fn next(&mut self) -> Option<PathBuf> {
        if self.files.is_empty() {
            return None;
        }
        self.current_index = (self.current_index + 1) % self.files.len();
        self.current_path()
    }

    fn prev(&mut self) -> Option<PathBuf> {
        if self.files.is_empty() {
            return None;
        }
        self.current_index = (self.current_index + self.files.len() - 1) % self.files.len();
        self.current_path()
    }

    /// Returns adjacent paths biased by the current navigation direction.
    /// When scrubbing forward, prefetch more forward images.
    /// When scrubbing backward, prefetch more backward images.
    /// When neutral (e.g. thumbnail click), prefetch symmetrically.
    fn adjacent_paths(&self) -> Vec<PathBuf> {
        if self.files.is_empty() {
            return vec![];
        }
        let len = self.files.len();
        let max_neighbors = len.saturating_sub(1);

        let (forward_count, backward_count) = match self.last_nav_direction.signum() {
            1 => (10.min(max_neighbors), 3.min(max_neighbors)),
            -1 => (3.min(max_neighbors), 10.min(max_neighbors)),
            _ => (5.min(max_neighbors), 5.min(max_neighbors)),
        };

        let mut paths = Vec::with_capacity(forward_count + backward_count);
        for offset in 1..=forward_count {
            paths.push(self.files[(self.current_index + offset) % len].clone());
        }
        for offset in 1..=backward_count {
            paths.push(self.files[(self.current_index + len - offset) % len].clone());
        }
        paths
    }
}

fn start_directory_watcher(
    state: Rc<RefCell<AppState>>,
    populate_thumbnails: Rc<dyn Fn()>,
    load_image: Rc<dyn Fn(PathBuf)>,
) -> notify::RecommendedWatcher {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();

    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .expect("Failed to create directory watcher");

    if let Some(dir) = state.borrow().watched_directory.clone() {
        let _ = watcher.watch(&dir, RecursiveMode::NonRecursive);
    }

    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let mut changed = false;
        while let Ok(res) = rx.try_recv() {
            match res {
                Ok(_event) => changed = true,
                Err(err) => eprintln!("[Iris] Directory watch error: {err}"),
            }
        }

        if changed {
            let next = state.borrow_mut().refresh_watched_directory();
            populate_thumbnails();
            if let Some(path) = next {
                load_image(path);
            }
        }

        glib::ControlFlow::Continue
    });

    watcher
}

/// Load or generate a 128×128 RGBA8 thumbnail entirely off the GTK thread.
fn load_or_generate_thumb(path: &Path) -> Option<Vec<u8>> {
    let thumb_size = 128u32;

    let cache_dir = dirs::cache_dir()?.join("thumbnails").join("normal");
    let uri = format!(
        "file://{}",
        path.canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
    );
    let key = format!("{:x}", md5::compute(uri.as_bytes()));
    let thumb_path = cache_dir.join(format!("{key}.png"));

    // Try disk cache
    if let (Ok(src_meta), Ok(thumb_meta)) =
        (std::fs::metadata(path), std::fs::metadata(&thumb_path))
    {
        if let (Ok(src_mtime), Ok(thumb_mtime)) = (src_meta.modified(), thumb_meta.modified()) {
            if thumb_mtime >= src_mtime {
                if let Ok(img) = image::open(&thumb_path) {
                    let rgba = img.to_rgba8();
                    let resized = image::imageops::resize(
                        &rgba,
                        thumb_size,
                        thumb_size,
                        image::imageops::FilterType::Triangle,
                    );
                    return Some(resized.into_raw());
                }
            }
        }
    }

    // Cache miss — generate
    let thumb = if crate::raw::is_raw(path) {
        let raw_img = crate::raw::decode_raw(path)?;
        let rgba8 = crate::raw::linear_16_to_srgb_8(&raw_img.data, raw_img.width, raw_img.height);
        let img = image::RgbaImage::from_raw(raw_img.width, raw_img.height, rgba8)?;
        image::imageops::resize(
            &img,
            thumb_size,
            thumb_size,
            image::imageops::FilterType::Triangle,
        )
    } else {
        let img = image::open(path).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        let icc = crate::color::extract_icc_profile(path);
        let corrected = crate::color::rgba8_to_srgb_with_icc(img.as_raw(), w, h, icc.as_deref());
        let corrected_img = image::RgbaImage::from_raw(w, h, corrected)?;
        image::imageops::resize(
            &corrected_img,
            thumb_size,
            thumb_size,
            image::imageops::FilterType::Triangle,
        )
    };

    // Save to disk cache
    let _ = std::fs::create_dir_all(&cache_dir);
    let _ = thumb.save_with_format(&thumb_path, image::ImageFormat::Png);

    Some(thumb.into_raw())
}

fn main() {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gtk4::gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_activate(|app| {
        build_ui(app, None);
    });

    app.connect_open(|app, files, _hint| {
        let path = files.first().and_then(|f| f.path());
        build_ui(app, path);
    });

    app.run();
}

fn build_ui(app: &adw::Application, initial_path: Option<PathBuf>) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let cfg = Config::load();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Iris")
        .default_width(cfg.window_width)
        .default_height(cfg.window_height)
        .build();

    if cfg.window_maximized {
        window.maximize();
    }

    let css = gtk4::CssProvider::new();
    css.load_from_string(
        "
        .thumb-btn { padding: 3px; border-radius: 8px; transition: all 180ms ease; opacity: 0.6; }
        .thumb-btn:hover { opacity: 1.0; background: alpha(@accent_color, 0.15); }
        .thumb-active { opacity: 1.0; outline: 2px solid @accent_color; border-radius: 8px; background: alpha(@accent_color, 0.12); }
        .thumb-strip { background: alpha(@window_bg_color, 0.95); }
        .info-panel { padding: 16px; border-left: 1px solid alpha(@borders, 0.5); }
        .info-field-label { font-size: 11px; opacity: 0.5; margin-top: 10px; text-transform: uppercase; letter-spacing: 0.5px; }
        .info-field-value { font-weight: 600; }
    ",
    );
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let state = Rc::new(RefCell::new(AppState::new()));
    state.borrow_mut().info_visible = cfg.info_panel_visible;

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let open_btn = gtk4::Button::builder().label("Open").build();
    let rotate_cw_btn = gtk4::Button::builder()
        .icon_name("object-rotate-right-symbolic")
        .tooltip_text("Rotate CW (R)")
        .build();
    let rotate_ccw_btn = gtk4::Button::builder()
        .icon_name("object-rotate-left-symbolic")
        .tooltip_text("Rotate CCW (Shift+R)")
        .build();
    let info_btn = gtk4::Button::builder()
        .icon_name("dialog-information-symbolic")
        .tooltip_text("Image info (I)")
        .build();

    let enhance_btn = gtk4::ToggleButton::builder()
        .icon_name("display-brightness-symbolic")
        .tooltip_text("Auto Enhance (E)")
        .build();
    let sharpen_btn = gtk4::ToggleButton::builder()
        .icon_name("find-location-symbolic")
        .tooltip_text("Sharpen (S)")
        .build();
    let denoise_btn = gtk4::ToggleButton::builder()
        .icon_name("weather-fog-symbolic")
        .tooltip_text("Denoise (D)")
        .build();

    header.pack_start(&open_btn);
    header.pack_end(&info_btn);
    header.pack_end(&rotate_cw_btn);
    header.pack_end(&rotate_ccw_btn);
    header.pack_end(&gtk4::Separator::new(Orientation::Vertical));
    header.pack_end(&denoise_btn);
    header.pack_end(&sharpen_btn);
    header.pack_end(&enhance_btn);

    let counter_label = Rc::new(gtk4::Label::new(Some("Iris")));
    header.set_title_widget(Some(&*counter_label));
    toolbar_view.add_top_bar(&header);

    let root_box = gtk4::Box::new(Orientation::Vertical, 0);
    let content_box = gtk4::Box::new(Orientation::Horizontal, 0);
    content_box.set_vexpand(true);

    let toast_overlay = adw::ToastOverlay::new();

    let viewport_stack = Rc::new(gtk4::Stack::new());
    viewport_stack.set_vexpand(true);
    viewport_stack.set_hexpand(true);
    viewport_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    viewport_stack.set_transition_duration(150);

    let viewport = Rc::new(viewport::Viewport::new({
        let toast_overlay = toast_overlay.clone();
        move |msg| {
            let toast = adw::Toast::new(&msg);
            toast.set_timeout(5);
            toast_overlay.add_toast(toast);
        }
    }));
    viewport_stack.add_named(&viewport.widget, Some("image"));

    let welcome_box = gtk4::Box::new(Orientation::Vertical, 12);
    welcome_box.set_halign(gtk4::Align::Center);
    welcome_box.set_valign(gtk4::Align::Center);
    let welcome_icon = gtk4::Image::from_icon_name("image-x-generic-symbolic");
    welcome_icon.set_pixel_size(64);
    welcome_icon.set_opacity(0.3);
    let welcome_lbl = gtk4::Label::builder()
        .label("Open an image or drag one here")
        .css_classes(["title-4"])
        .opacity(0.4)
        .build();
    welcome_box.append(&welcome_icon);
    welcome_box.append(&welcome_lbl);

    viewport_stack.add_named(&welcome_box, Some("welcome"));
    viewport_stack.set_visible_child_name("welcome");

    content_box.append(&*viewport_stack);

    let info_sep = Rc::new(gtk4::Separator::new(Orientation::Vertical));
    info_sep.set_visible(cfg.info_panel_visible);
    content_box.append(&*info_sep);

    let info_panel = Rc::new(gtk4::Box::new(Orientation::Vertical, 4));
    info_panel.set_width_request(260);
    info_panel.set_visible(cfg.info_panel_visible);
    info_panel.add_css_class("info-panel");
    content_box.append(&*info_panel);

    let info_title = gtk4::Label::builder()
        .label("Image Info")
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();
    info_panel.append(&info_title);
    info_panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    let make_field = |label_text: &str| -> (gtk4::Box, Rc<gtk4::Label>) {
        let row = gtk4::Box::new(Orientation::Vertical, 2);
        let lbl = gtk4::Label::builder()
            .label(label_text)
            .xalign(0.0)
            .css_classes(["info-field-label"])
            .build();
        let val = Rc::new(
            gtk4::Label::builder()
                .label("—")
                .xalign(0.0)
                .wrap(true)
                .selectable(true)
                .css_classes(["info-field-value"])
                .build(),
        );
        row.append(&lbl);
        row.append(&*val);
        (row, val)
    };

    let (row_name, info_name) = make_field("Filename");
    let (row_dims, info_dims) = make_field("Dimensions");
    let (row_size, info_size) = make_field("File size");
    let (row_path, info_path_lbl) = make_field("Path");

    info_panel.append(&row_name);
    info_panel.append(&row_dims);
    info_panel.append(&row_size);
    info_panel.append(&row_path);

    let thumb_scroll = Rc::new(
        gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .height_request(108)
            .focusable(false)
            .can_focus(false)
            .build(),
    );

    let thumb_strip = Rc::new(gtk4::Box::new(Orientation::Horizontal, 6));
    thumb_strip.set_margin_start(8);
    thumb_strip.set_margin_end(8);
    thumb_strip.set_margin_top(6);
    thumb_strip.set_margin_bottom(6);
    thumb_scroll.set_child(Some(&*thumb_strip));

    root_box.append(&content_box);
    root_box.append(&gtk4::Separator::new(Orientation::Horizontal));
    root_box.append(&*thumb_scroll);

    toolbar_view.set_content(Some(&root_box));
    toast_overlay.set_child(Some(&toolbar_view));
    window.set_content(Some(&toast_overlay));

    // ── Navigation coalescing state ───────────────────────────────────────
    let nav_pending: Rc<Cell<i32>> = Rc::new(Cell::new(0));
    let nav_scheduled: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // ── O(1) thumbnail tracking ───────────────────────────────────────────
    let prev_active_thumb: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    let thumb_buttons: Rc<RefCell<Vec<gtk4::Button>>> = Rc::new(RefCell::new(vec![]));
    let load_image_fn: Rc<RefCell<Option<Rc<dyn Fn(PathBuf)>>>> = Rc::new(RefCell::new(None));

    let scroll_to_active_thumb = {
        let thumb_buttons = thumb_buttons.clone();
        let thumb_scroll = thumb_scroll.clone();
        let state = state.clone();
        Rc::new(move || {
            let idx = state.borrow().current_index;
            let btns = thumb_buttons.borrow();
            if let Some(btn) = btns.get(idx) {
                let hadj = thumb_scroll.hadjustment();
                if let Some(point) =
                    btn.compute_point(&*thumb_scroll, &gtk4::graphene::Point::new(0.0, 0.0))
                {
                    let x = point.x() as f64;
                    let btn_width = btn.width() as f64;
                    let scroll_width = thumb_scroll.width() as f64;
                    let current = hadj.value();
                    if x < 0.0 || x + btn_width > scroll_width {
                        let target = current + x - (scroll_width / 2.0) + (btn_width / 2.0);
                        hadj.set_value(target.max(0.0));
                    }
                }
            }
        })
    };

    let populate_thumbnails: Rc<dyn Fn()> = Rc::new({
        let thumb_strip = thumb_strip.clone();
        let thumb_buttons = thumb_buttons.clone();
        let state = state.clone();
        let load_fn_ref = load_image_fn.clone();
        let prev_active = prev_active_thumb.clone();

        move || {
            while let Some(child) = thumb_strip.first_child() {
                thumb_strip.remove(&child);
            }
            thumb_buttons.borrow_mut().clear();

            let files = state.borrow().files.clone();
            let current_index = state.borrow().current_index;

            for (i, path) in files.iter().enumerate() {
                let thumb_spinner = gtk4::Spinner::new();
                thumb_spinner.set_size_request(90, 90);
                thumb_spinner.start();

                let thumb_stack = gtk4::Stack::new();
                thumb_stack.set_size_request(90, 90);
                thumb_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
                thumb_stack.set_transition_duration(200);
                thumb_stack.add_named(&thumb_spinner, Some("loading"));

                let thumb_pic = gtk4::Picture::builder()
                    .can_shrink(true)
                    .content_fit(gtk4::ContentFit::Cover)
                    .width_request(90)
                    .height_request(90)
                    .build();
                thumb_stack.add_named(&thumb_pic, Some("image"));
                thumb_stack.set_visible_child_name("loading");

                let btn = gtk4::Button::builder()
                    .child(&thumb_stack)
                    .css_classes(["flat", "thumb-btn"])
                    .focusable(false)
                    .can_focus(false)
                    .build();

                if i == current_index {
                    btn.add_css_class("thumb-active");
                }

                let state_click = state.clone();
                let load_fn_click = load_fn_ref.clone();
                let path_click = path.clone();
                btn.connect_clicked(move |_| {
                    {
                        let mut s = state_click.borrow_mut();
                        s.current_index = i;
                        s.last_nav_direction = 0;
                    }
                    if let Some(f) = load_fn_click.borrow().as_ref() {
                        f(path_click.clone());
                    }
                });

                thumb_strip.append(&btn);
                thumb_buttons.borrow_mut().push(btn);

                let path_async = path.clone();
                let thumb_pic_async = thumb_pic.clone();
                let thumb_stack_async = thumb_stack.clone();

                glib::spawn_future_local(async move {
                    let (tx, rx) = futures::channel::oneshot::channel();
                    rayon::spawn({
                        let path = path_async.clone();
                        move || {
                            let result = load_or_generate_thumb(&path);
                            let _ = tx.send(result);
                        }
                    });

                    if let Ok(Some(bytes)) = rx.await {
                        let glib_bytes = glib::Bytes::from_owned(bytes);
                        let texture = gtk4::gdk::MemoryTexture::new(
                            128,
                            128,
                            gtk4::gdk::MemoryFormat::R8g8b8a8,
                            &glib_bytes,
                            (128 * 4) as usize,
                        );
                        thumb_pic_async.set_paintable(Some(&texture));
                    }
                    thumb_stack_async.set_visible_child_name("image");
                });
            }

            // Sync the O(1) tracker with the freshly created buttons
            prev_active.set(Some(current_index));
        }
    });

    // ── Core load_image closure ───────────────────────────────────────────
    let load_image: Rc<dyn Fn(PathBuf)> = Rc::new({
        let counter_label = counter_label.clone();
        let state = state.clone();
        let info_name = info_name.clone();
        let info_dims = info_dims.clone();
        let info_size = info_size.clone();
        let info_path_lbl = info_path_lbl.clone();
        let thumb_buttons = thumb_buttons.clone();
        let viewport_stack = viewport_stack.clone();
        let viewport_engine = viewport.clone();
        let scroll_fn = scroll_to_active_thumb.clone();
        let prev_active = prev_active_thumb.clone();

        move |path: PathBuf| {
            // ── 1. Save view state of the image we're leaving ─────────────
            {
                let current = state.borrow().current_path();
                if let Some(ref current_path) = current {
                    let (zoom, px, py) = viewport_engine.get_view_state();
                    if zoom != 1.0 || px != 0.0 || py != 0.0 {
                        state.borrow_mut().view_states.insert(
                            current_path.clone(),
                            ViewState {
                                zoom,
                                position_x: px,
                                position_y: py,
                            },
                        );
                    }
                }
            }

            // ── 2. Get rotation from cache (zero I/O) ────────────────────
            let cached_rotation = state.borrow().rotations.get(&path).copied();
            let rotation = cached_rotation.unwrap_or(0);

            // ── 3. Gather navigation state ────────────────────────────────
            let (idx, total, adjacent) = {
                let s = state.borrow();
                (s.current_index, s.files.len(), s.adjacent_paths())
            };

            // ── 4. Restore or reset camera for the target image ───────────
            {
                let s = state.borrow();
                if let Some(vs) = s.view_states.get(&path) {
                    viewport_engine.prepare_view(vs.zoom, vs.position_x, vs.position_y);
                } else {
                    viewport_engine.prepare_view(1.0, 0.0, 0.0);
                }
            }

            // ── 5. Update header / info labels (cheap string ops) ─────────
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            counter_label.set_label(&format!("{} — {}/{}", name, idx + 1, total));
            info_name.set_label(&name);
            info_path_lbl.set_label(path.to_str().unwrap_or(""));

            // ── 6. O(1) thumbnail active-state update ─────────────────────
            {
                let btns = thumb_buttons.borrow();
                if let Some(prev) = prev_active.get() {
                    if let Some(btn) = btns.get(prev) {
                        btn.remove_css_class("thumb-active");
                    }
                }
                if let Some(btn) = btns.get(idx) {
                    btn.add_css_class("thumb-active");
                }
                prev_active.set(Some(idx));
            }

            // ── 7. Scroll thumbnail strip ─────────────────────────────────
            scroll_fn();

            // ── 8. Apply rotation and show viewport ───────────────────────
            viewport_engine.set_rotation(rotation as f32);
            viewport_stack.set_visible_child_name("image");

            // ── 9. Trigger image load (async internally) ──────────────────
            let info_dims_cb = info_dims.clone();
            viewport_engine.load_image(path.clone(), move |w, h| {
                info_dims_cb.set_label(&format!("{}×{} px", w, h));
            });

            // ── 10. Directional prefetch ──────────────────────────────────
            for adj_path in adjacent {
                viewport_engine.prefetch(adj_path);
            }

            // ── 11. Async EXIF rotation (only if not already cached) ──────
            if cached_rotation.is_none() {
                let path_exif = path.clone();
                let state_exif = state.clone();
                let viewport_exif = viewport_engine.clone();
                let (tx, rx) = futures::channel::oneshot::channel();
                rayon::spawn(move || {
                    let rot = read_exif_rotation(&path_exif);
                    let _ = tx.send((path_exif, rot));
                });
                glib::spawn_future_local({
                    let state_exif = state_exif.clone();
                    async move {
                        if let Ok((p, rot)) = rx.await {
                            let is_current = {
                                let mut s = state_exif.borrow_mut();
                                s.rotations.insert(p.clone(), rot);
                                s.current_path().as_deref() == Some(p.as_path())
                            };
                            if is_current && rot != 0 {
                                viewport_exif.set_rotation(rot as f32);
                            }
                        }
                    }
                });
            }

            // ── 12. Async file-size metadata ──────────────────────────────
            {
                info_size.set_label("…");
                let path_meta = path.clone();
                let info_size_cb = info_size.clone();
                let (tx, rx) = futures::channel::oneshot::channel();
                rayon::spawn(move || {
                    let size = std::fs::metadata(&path_meta).ok().map(|m| m.len());
                    let _ = tx.send(size);
                });
                glib::spawn_future_local(async move {
                    if let Ok(Some(bytes)) = rx.await {
                        let size_str = if bytes > 1_048_576 {
                            format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                        } else {
                            format!("{:.0} KB", bytes as f64 / 1024.0)
                        };
                        info_size_cb.set_label(&size_str);
                    }
                });
            }
        }
    });

    *load_image_fn.borrow_mut() = Some(load_image.clone());

    // ── Navigation coalescing scheduler ───────────────────────────────────
    // Accumulates rapid key-repeat events and processes them as a single
    // jump once the GTK main loop drains its event queue.
    let schedule_nav: Rc<dyn Fn()> = Rc::new({
        let nav_pending = nav_pending.clone();
        let nav_scheduled = nav_scheduled.clone();
        let state = state.clone();
        let load_image = load_image.clone();

        move || {
            if nav_scheduled.get() {
                return;
            }
            nav_scheduled.set(true);

            let np = nav_pending.clone();
            let ns = nav_scheduled.clone();
            let st = state.clone();
            let lk = load_image.clone();

            glib::idle_add_local_once(move || {
                ns.set(false);
                let delta = np.replace(0);
                if delta == 0 {
                    return;
                }
                let path = {
                    let mut s = st.borrow_mut();
                    let len = s.files.len();
                    if len == 0 {
                        return;
                    }
                    let new_idx =
                        (s.current_index as i64 + delta as i64).rem_euclid(len as i64) as usize;
                    s.current_index = new_idx;
                    s.last_nav_direction = if delta > 0 { 1 } else { -1 };
                    s.current_path()
                };
                if let Some(p) = path {
                    lk(p);
                }
            });
        }
    });

    let _watcher = start_directory_watcher(
        state.clone(),
        populate_thumbnails.clone(),
        load_image.clone(),
    );

    let window_ref = window.clone();
    let state_open = state.clone();
    let load_open = load_image.clone();
    let populate_open = populate_thumbnails.clone();
    open_btn.connect_clicked(move |_| {
        let dialog = FileDialog::builder()
            .title("Open Image")
            .modal(true)
            .build();
        let state_clone = state_open.clone();
        let load = load_open.clone();
        let populate = populate_open.clone();
        dialog.open(
            Some(&window_ref),
            gtk4::gio::Cancellable::NONE,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        state_clone.borrow_mut().load_directory(&path);
                        populate();
                        load(path);
                    }
                }
            },
        );
    });

    let info_panel_btn = info_panel.clone();
    let info_sep_btn = info_sep.clone();
    let state_info = state.clone();
    info_btn.connect_clicked(move |_| {
        let mut s = state_info.borrow_mut();
        s.info_visible = !s.info_visible;
        info_panel_btn.set_visible(s.info_visible);
        info_sep_btn.set_visible(s.info_visible);
    });

    let state_rcw = state.clone();
    let viewport_rcw = viewport.clone();
    rotate_cw_btn.connect_clicked(move |_| {
        let rotation = {
            let mut s = state_rcw.borrow_mut();
            s.rotate_cw();
            s.current_rotation()
        };
        viewport_rcw.set_rotation(rotation as f32);
    });

    let state_rccw = state.clone();
    let viewport_rccw = viewport.clone();
    rotate_ccw_btn.connect_clicked(move |_| {
        let rotation = {
            let mut s = state_rccw.borrow_mut();
            s.rotate_ccw();
            s.current_rotation()
        };
        viewport_rccw.set_rotation(rotation as f32);
    });

    let viewport_enh = viewport.clone();
    enhance_btn.connect_toggled(move |_| {
        viewport_enh.toggle_enhance();
    });

    let viewport_shp = viewport.clone();
    sharpen_btn.connect_toggled(move |_| {
        viewport_shp.toggle_sharpen();
    });

    let viewport_dns = viewport.clone();
    denoise_btn.connect_toggled(move |_| {
        viewport_dns.toggle_denoise();
    });

    let drop_target = gtk4::DropTarget::new(
        gtk4::gdk::FileList::static_type(),
        gtk4::gdk::DragAction::COPY,
    );
    let state_drop = state.clone();
    let load_drop = load_image.clone();
    let populate_drop = populate_thumbnails.clone();
    drop_target.connect_drop(move |_, value, _, _| {
        let Ok(file_list) = value.get::<gtk4::gdk::FileList>() else {
            return false;
        };
        let files = file_list.files();
        let Some(file) = files.first() else {
            return false;
        };
        let Some(path) = file.path() else {
            return false;
        };
        if path.is_file() {
            state_drop.borrow_mut().load_directory(&path);
            populate_drop();
            load_drop(path);
            true
        } else if path.is_dir() {
            state_drop.borrow_mut().load_from_directory(&path);
            populate_drop();
            if let Some(first) = state_drop.borrow().current_path() {
                load_drop(first);
            }
            true
        } else {
            false
        }
    });
    window.add_controller(drop_target);

    // ── Keyboard handler with navigation coalescing ───────────────────────
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let window_key = window.clone();
    let state_key = state.clone();
    let info_panel_key = info_panel.clone();
    let info_sep_key = info_sep.clone();
    let viewport_key = viewport.clone();
    let nav_pending_key = nav_pending.clone();
    let schedule_nav_key = schedule_nav.clone();

    key_ctrl.connect_key_pressed(move |_, key, _, modifier| match key {
        gtk4::gdk::Key::f | gtk4::gdk::Key::F => {
            window_key.fullscreen();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::Escape => {
            window_key.unfullscreen();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::Right | gtk4::gdk::Key::space => {
            nav_pending_key.set(nav_pending_key.get() + 1);
            schedule_nav_key();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::Left => {
            nav_pending_key.set(nav_pending_key.get() - 1);
            schedule_nav_key();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::r | gtk4::gdk::Key::R => {
            let rotation = {
                let mut s = state_key.borrow_mut();
                if modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                    s.rotate_ccw();
                } else {
                    s.rotate_cw();
                }
                s.current_rotation()
            };
            viewport_key.set_rotation(rotation as f32);
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::plus | gtk4::gdk::Key::equal => {
            viewport_key.zoom_in();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::minus => {
            viewport_key.zoom_out();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::_0 | gtk4::gdk::Key::Home => {
            viewport_key.reset_view();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::i | gtk4::gdk::Key::I => {
            let mut s = state_key.borrow_mut();
            s.info_visible = !s.info_visible;
            info_panel_key.set_visible(s.info_visible);
            info_sep_key.set_visible(s.info_visible);
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::e | gtk4::gdk::Key::E => {
            viewport_key.toggle_enhance();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::s | gtk4::gdk::Key::S => {
            viewport_key.toggle_sharpen();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::d | gtk4::gdk::Key::D => {
            viewport_key.toggle_denoise();
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    window.add_controller(key_ctrl);

    let state_close = state.clone();
    window.connect_close_request(move |win| {
        let s = state_close.borrow();
        let config = Config {
            window_width: win.width(),
            window_height: win.height(),
            window_maximized: win.is_maximized(),
            info_panel_visible: s.info_visible,
            last_directory: s
                .current_path()
                .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned())),
        };
        config.save();
        glib::Propagation::Proceed
    });

    window.present();

    if let Some(path) = initial_path {
        if path.is_file() {
            state.borrow_mut().load_directory(&path);
            populate_thumbnails();
            load_image(path);
        } else if path.is_dir() {
            state.borrow_mut().load_from_directory(&path);
            populate_thumbnails();
            if let Some(first) = state.borrow().current_path() {
                load_image(first);
            }
        }
    }
}
