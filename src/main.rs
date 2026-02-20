use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{EventControllerScroll, EventControllerScrollFlags, FileDialog, Orientation, glib};
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

mod viewport;

const APP_ID: &str = "dev.iris.viewer";

struct AppState {
    files: Vec<PathBuf>,
    current_index: usize,
    scale: f64,
    rotations: HashMap<PathBuf, i32>,
    info_visible: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            files: vec![],
            current_index: 0,
            scale: 1.0,
            rotations: HashMap::new(),
            info_visible: false,
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
            let mut files: Vec<PathBuf> = std::fs::read_dir(parent)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "tiff" | "bmp")
                    )
                })
                .collect();
            files.sort();
            self.current_index = files.iter().position(|f| f == path).unwrap_or(0);
            self.files = files;
        }
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
}

// Helper for thumbnails (CPU side)
async fn load_bytes_async(path: PathBuf) -> Option<Vec<u8>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    rayon::spawn(move || {
        let _ = tx.send(std::fs::read(&path).ok());
    });
    rx.await.ok().flatten()
}

// Helper for thumbnails (CPU side)
fn pixbuf_from_bytes(bytes: &[u8], rotation: i32) -> Option<gtk4::gdk_pixbuf::Pixbuf> {
    use gtk4::gdk_pixbuf::{PixbufLoader, PixbufRotation};
    let loader = PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let pixbuf = loader.pixbuf()?;
    let rot = match rotation {
        90 => PixbufRotation::Clockwise,
        180 => PixbufRotation::Upsidedown,
        270 => PixbufRotation::Counterclockwise,
        _ => PixbufRotation::None,
    };
    pixbuf.rotate_simple(rot)
}

fn main() {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Iris")
        .default_width(1200)
        .default_height(800)
        .build();

    // ── CSS ─────────────────────────────────────────────
    let css = gtk4::CssProvider::new();
    css.load_from_string("
        .thumb-btn { padding: 3px; border-radius: 8px; transition: all 180ms ease; opacity: 0.6; }
        .thumb-btn:hover { opacity: 1.0; background: alpha(@accent_color, 0.15); }
        .thumb-active { opacity: 1.0; outline: 2px solid @accent_color; border-radius: 8px; background: alpha(@accent_color, 0.12); }
        .thumb-strip { background: alpha(@window_bg_color, 0.95); }
        .info-panel { padding: 16px; border-left: 1px solid alpha(@borders, 0.5); }
        .info-field-label { font-size: 11px; opacity: 0.5; margin-top: 10px; text-transform: uppercase; letter-spacing: 0.5px; }
        .info-field-value { font-weight: 600; }
    ");
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let state = Rc::new(RefCell::new(AppState::new()));

    // ── Header ──────────────────────────────────────────
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

    header.pack_start(&open_btn);
    header.pack_end(&info_btn);
    header.pack_end(&rotate_cw_btn);
    header.pack_end(&rotate_ccw_btn);

    let counter_label = Rc::new(gtk4::Label::new(Some("Iris")));
    header.set_title_widget(Some(&*counter_label));
    toolbar_view.add_top_bar(&header);

    // ── Layout ──────────────────────────────────────────
    let root_box = gtk4::Box::new(Orientation::Vertical, 0);
    let content_box = gtk4::Box::new(Orientation::Horizontal, 0);
    content_box.set_vexpand(true);

    // ── Viewport stack ───────────────────────────────────
    let viewport_stack = Rc::new(gtk4::Stack::new());
    viewport_stack.set_vexpand(true);
    viewport_stack.set_hexpand(true);
    viewport_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
    viewport_stack.set_transition_duration(150);

    // ── NEW: THE ENGINE ─────────────────────────────────
    // Wrap in Rc so we can clone it into closures
    let viewport = Rc::new(viewport::Viewport::new());
    viewport_stack.add_named(&viewport.widget, Some("image"));
    // ────────────────────────────────────────────────────

    // Spinner page
    let spinner_box = gtk4::Box::new(Orientation::Vertical, 0);
    spinner_box.set_vexpand(true);
    spinner_box.set_hexpand(true);
    spinner_box.set_halign(gtk4::Align::Center);
    spinner_box.set_valign(gtk4::Align::Center);
    let spinner = Rc::new(gtk4::Spinner::new());
    spinner.set_size_request(32, 32);
    spinner_box.append(&*spinner);

    // Welcome page
    let welcome_box = gtk4::Box::new(Orientation::Vertical, 12);
    welcome_box.set_halign(gtk4::Align::Center);
    welcome_box.set_valign(gtk4::Align::Center);
    let welcome_icon = gtk4::Image::from_icon_name("image-x-generic-symbolic");
    welcome_icon.set_pixel_size(64);
    welcome_icon.set_opacity(0.3);
    let welcome_lbl = gtk4::Label::builder()
        .label("Open an image to begin")
        .css_classes(["title-4"])
        .opacity(0.4)
        .build();
    welcome_box.append(&welcome_icon);
    welcome_box.append(&welcome_lbl);

    viewport_stack.add_named(&spinner_box, Some("spinner"));
    viewport_stack.add_named(&welcome_box, Some("welcome"));
    viewport_stack.set_visible_child_name("welcome");

    content_box.append(&*viewport_stack);

    // ── Info Panel ──────────────────────────────────────
    let info_sep = Rc::new(gtk4::Separator::new(Orientation::Vertical));
    info_sep.set_visible(false);
    content_box.append(&*info_sep);

    let info_panel = Rc::new(gtk4::Box::new(Orientation::Vertical, 4));
    info_panel.set_width_request(260);
    info_panel.set_visible(false);
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

    // ── Thumbnail Strip ──────────────────────────────────
    let thumb_scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .height_request(108)
        .build();

    let thumb_strip = Rc::new(gtk4::Box::new(Orientation::Horizontal, 6));
    thumb_strip.set_margin_start(8);
    thumb_strip.set_margin_end(8);
    thumb_strip.set_margin_top(6);
    thumb_strip.set_margin_bottom(6);
    thumb_scroll.set_child(Some(&*thumb_strip));

    root_box.append(&content_box);
    root_box.append(&gtk4::Separator::new(Orientation::Horizontal));
    root_box.append(&thumb_scroll);

    toolbar_view.set_content(Some(&root_box));
    window.set_content(Some(&toolbar_view));

    let thumb_buttons: Rc<RefCell<Vec<gtk4::Button>>> = Rc::new(RefCell::new(vec![]));
    let load_image_fn: Rc<RefCell<Option<Rc<dyn Fn(PathBuf)>>>> = Rc::new(RefCell::new(None));

    // ── populate_thumbnails ──────────────────────────────
    let populate_thumbnails: Rc<dyn Fn()> = Rc::new({
        let thumb_strip = thumb_strip.clone();
        let thumb_buttons = thumb_buttons.clone();
        let state = state.clone();
        let load_fn_ref = load_image_fn.clone();
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
                    .build();

                if i == current_index {
                    btn.add_css_class("thumb-active");
                }

                let state_click = state.clone();
                let load_fn_click = load_fn_ref.clone();
                let path_click = path.clone();
                btn.connect_clicked(move |_| {
                    state_click.borrow_mut().current_index = i;
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
                    let bytes = load_bytes_async(path_async).await;
                    if let Some(b) = bytes {
                        if let Some(pb) = pixbuf_from_bytes(&b, 0) {
                            if let Some(s) =
                                pb.scale_simple(90, 90, gtk4::gdk_pixbuf::InterpType::Bilinear)
                            {
                                thumb_pic_async.set_pixbuf(Some(&s));
                            }
                        }
                        thumb_stack_async.set_visible_child_name("image");
                    }
                });
            }
        }
    });

    // ── load_image ───────────────────────────────────────
    let load_image: Rc<dyn Fn(PathBuf)> = Rc::new({
        let counter_label = counter_label.clone();
        let state = state.clone();
        let info_name = info_name.clone();
        let info_dims = info_dims.clone();
        let info_size = info_size.clone();
        let info_path_lbl = info_path_lbl.clone();
        let thumb_buttons = thumb_buttons.clone();
        let viewport_stack = viewport_stack.clone();
        let spinner = spinner.clone();
        let viewport_engine = viewport.clone(); // The Engine

        move |path: PathBuf| {
            // let rotation = state.borrow().current_rotation();
            let idx = state.borrow().current_index;
            let total = state.borrow().files.len();

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            counter_label.set_label(&format!("{} — {}/{}", name, idx + 1, total));
            info_name.set_label(&name);
            info_path_lbl.set_label(path.to_str().unwrap_or(""));

            if let Ok(meta) = std::fs::metadata(&path) {
                let bytes = meta.len();
                let size_str = if bytes > 1_048_576 {
                    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
                } else {
                    format!("{:.0} KB", bytes as f64 / 1024.0)
                };
                info_size.set_label(&size_str);
            }

            // Highlight thumbnail
            {
                let btns = thumb_buttons.borrow();
                for (i, btn) in btns.iter().enumerate() {
                    if i == idx {
                        btn.add_css_class("thumb-active");
                    } else {
                        btn.remove_css_class("thumb-active");
                    }
                }
            }

            // Show Spinner
            spinner.start();
            viewport_stack.set_visible_child_name("spinner");

            let info_dims_async = info_dims.clone();
            let viewport_stack_async = viewport_stack.clone();
            let spinner_async = spinner.clone();
            let viewport_engine_async = viewport_engine.clone();
            let path_async = path.clone();

            glib::spawn_future_local(async move {
                // Load metadata for Info Panel
                let bytes = load_bytes_async(path_async.clone()).await;
                spinner_async.stop();

                if let Some(b) = bytes {
                    if let Some(pb) = pixbuf_from_bytes(&b, 0) {
                        info_dims_async.set_label(&format!("{}×{} px", pb.width(), pb.height()));
                    }
                }

                // Trigger Engine Load
                viewport_engine_async.load_image(path_async);

                // Switch to the WGPU Engine
                viewport_stack_async.set_visible_child_name("image");
            });
        }
    });

    *load_image_fn.borrow_mut() = Some(load_image.clone());

    // ── Open dialog ──────────────────────────────────────
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

    // ── Info toggle ──────────────────────────────────────
    let info_panel_btn = info_panel.clone();
    let info_sep_btn = info_sep.clone();
    let state_info = state.clone();
    info_btn.connect_clicked(move |_| {
        let mut s = state_info.borrow_mut();
        s.info_visible = !s.info_visible;
        info_panel_btn.set_visible(s.info_visible);
        info_sep_btn.set_visible(s.info_visible);
    });

    // ── Rotate (Stubbed for now) ────────────────────────
    let state_rcw = state.clone();
    let load_rcw = load_image.clone();
    rotate_cw_btn.connect_clicked(move |_| {
        let path = {
            let mut s = state_rcw.borrow_mut();
            s.rotate_cw();
            if s.files.is_empty() {
                return;
            }
            s.files[s.current_index].clone()
        };
        load_rcw(path);
    });

    let state_rccw = state.clone();
    let load_rccw = load_image.clone();
    rotate_ccw_btn.connect_clicked(move |_| {
        let path = {
            let mut s = state_rccw.borrow_mut();
            s.rotate_ccw();
            if s.files.is_empty() {
                return;
            }
            s.files[s.current_index].clone()
        };
        load_rccw(path);
    });

    // ── Keyboard ─────────────────────────────────────────
    let key_ctrl = gtk4::EventControllerKey::new();
    let window_key = window.clone();
    let state_key = state.clone();
    let load_key = load_image.clone();
    let info_panel_key = info_panel.clone();
    let info_sep_key = info_sep.clone();
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
            if let Some(p) = state_key.borrow_mut().next() {
                load_key(p);
            }
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::Left => {
            if let Some(p) = state_key.borrow_mut().prev() {
                load_key(p);
            }
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::r | gtk4::gdk::Key::R => {
            let path = {
                let mut s = state_key.borrow_mut();
                if modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
                    s.rotate_ccw();
                } else {
                    s.rotate_cw();
                }
                if s.files.is_empty() {
                    return glib::Propagation::Stop;
                }
                s.files[s.current_index].clone()
            };
            load_key(path);
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::i | gtk4::gdk::Key::I => {
            let mut s = state_key.borrow_mut();
            s.info_visible = !s.info_visible;
            info_panel_key.set_visible(s.info_visible);
            info_sep_key.set_visible(s.info_visible);
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    window.add_controller(key_ctrl);

    window.present();
}
