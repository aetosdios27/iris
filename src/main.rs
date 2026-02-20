use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{
    EventControllerScroll, EventControllerScrollFlags, FileDialog, Picture, ScrolledWindow, glib,
};
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

mod renderer;

const APP_ID: &str = "dev.iris.viewer";

struct AppState {
    files: Vec<PathBuf>,
    current_index: usize,
    scale: f64,
    rotations: HashMap<PathBuf, i32>,
}

impl AppState {
    fn new() -> Self {
        Self {
            files: vec![],
            current_index: 0,
            scale: 1.0,
            rotations: HashMap::new(),
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

fn main() {
    let renderer = Arc::new(pollster::block_on(renderer::Renderer::new(1200, 800)));
    println!("wgpu Vulkan renderer ready");

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| build_ui(app, renderer.clone()));
    app.run();
}

fn load_pixbuf_rotated(path: &PathBuf, rotation: i32) -> Option<gtk4::gdk_pixbuf::Pixbuf> {
    use gtk4::gdk_pixbuf::{Pixbuf, PixbufRotation};
    let pixbuf = Pixbuf::from_file(path).ok()?;
    let rot = match rotation {
        90 => PixbufRotation::Clockwise,
        180 => PixbufRotation::Upsidedown,
        270 => PixbufRotation::Counterclockwise,
        _ => PixbufRotation::None,
    };
    pixbuf.rotate_simple(rot)
}

fn build_ui(app: &adw::Application, _renderer: Arc<renderer::Renderer>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Iris")
        .default_width(1200)
        .default_height(800)
        .build();

    let state = Rc::new(RefCell::new(AppState::new()));
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

    header.pack_start(&open_btn);
    header.pack_end(&rotate_cw_btn);
    header.pack_end(&rotate_ccw_btn);

    let counter_label = Rc::new(gtk4::Label::new(Some("Iris")));
    header.set_title_widget(Some(&*counter_label));
    toolbar_view.add_top_bar(&header);

    let picture = Rc::new(
        Picture::builder()
            .vexpand(true)
            .hexpand(true)
            .can_shrink(true)
            .content_fit(gtk4::ContentFit::Contain)
            .build(),
    );

    let zoom_box = Rc::new(
        gtk4::Box::builder()
            .vexpand(true)
            .hexpand(true)
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .build(),
    );
    zoom_box.append(&*picture);

    let scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&*zoom_box)
        .build();

    toolbar_view.set_content(Some(&scrolled));
    window.set_content(Some(&toolbar_view));

    // Load image helper
    let load_image = {
        let picture = picture.clone();
        let counter_label = counter_label.clone();
        let state = state.clone();
        let zoom_box = zoom_box.clone();
        move |path: PathBuf| {
            let rotation = state.borrow().current_rotation();
            let scale = state.borrow().scale;
            if let Some(pixbuf) = load_pixbuf_rotated(&path, rotation) {
                let w = (pixbuf.width() as f64 * scale) as i32;
                let h = (pixbuf.height() as f64 * scale) as i32;
                zoom_box.set_size_request(w, h);
                picture.set_pixbuf(Some(&pixbuf));
            }
            let s = state.borrow();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            counter_label.set_label(&format!(
                "{} — {}/{}",
                name,
                s.current_index + 1,
                s.files.len()
            ));
        }
    };

    // Scroll to zoom
    let scroll_ctrl = EventControllerScroll::new(
        EventControllerScrollFlags::VERTICAL | EventControllerScrollFlags::DISCRETE,
    );
    let state_scroll = state.clone();
    let zoom_box_scroll = zoom_box.clone();
    let picture_scroll = picture.clone();
    scroll_ctrl.connect_scroll(move |_, _, dy| {
        let mut s = state_scroll.borrow_mut();
        s.scale = (s.scale * if dy < 0.0 { 1.1 } else { 0.9 }).clamp(0.1, 10.0);
        if let Some(paintable) = picture_scroll.paintable() {
            let w = (paintable.intrinsic_width() as f64 * s.scale) as i32;
            let h = (paintable.intrinsic_height() as f64 * s.scale) as i32;
            zoom_box_scroll.set_size_request(w, h);
        }
        glib::Propagation::Stop
    });
    scrolled.add_controller(scroll_ctrl);

    // Open dialog
    let window_ref = window.clone();
    let state_open = state.clone();
    let load_open = load_image.clone();
    open_btn.connect_clicked(move |_| {
        let dialog = FileDialog::builder()
            .title("Open Image")
            .modal(true)
            .build();
        let state_clone = state_open.clone();
        let load = load_open.clone();
        dialog.open(
            Some(&window_ref),
            gtk4::gio::Cancellable::NONE,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        state_clone.borrow_mut().load_directory(&path);
                        load(path);
                    }
                }
            },
        );
    });

    // Rotate CW button
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

    // Rotate CCW button
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

    // Keyboard
    let key_ctrl = gtk4::EventControllerKey::new();
    let window_key = window.clone();
    let state_key = state.clone();
    let load_key = load_image.clone();
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
            let path = state_key.borrow_mut().next();
            if let Some(p) = path {
                load_key(p);
            }
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::Left => {
            let path = state_key.borrow_mut().prev();
            if let Some(p) = path {
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
        _ => glib::Propagation::Proceed,
    });
    window.add_controller(key_ctrl);

    window.present();
}
