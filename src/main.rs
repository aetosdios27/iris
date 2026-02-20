use adw::prelude::*;
use gtk4::prelude::*;
use gtk4::{
    EventControllerScroll, EventControllerScrollFlags, FileDialog, Picture, ScrolledWindow, glib,
};
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

mod renderer;

const APP_ID: &str = "dev.iris.viewer";

fn main() {
    // wgpu on Vulkan — completely separate from GTK's GL context, zero conflict
    let renderer = Arc::new(pollster::block_on(renderer::Renderer::new(1200, 800)));
    println!("wgpu Vulkan renderer ready");

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| build_ui(app, renderer.clone()));
    app.run();
}

fn build_ui(app: &adw::Application, renderer: Arc<renderer::Renderer>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Iris")
        .default_width(1200)
        .default_height(800)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let open_btn = gtk4::Button::builder().label("Open").build();
    header.pack_start(&open_btn);
    toolbar_view.add_top_bar(&header);

    let picture = Rc::new(
        Picture::builder()
            .vexpand(true)
            .hexpand(true)
            .can_shrink(true)
            .content_fit(gtk4::ContentFit::Contain)
            .build(),
    );

    let scrolled = ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&*picture)
        .build();

    // Scroll to zoom
    let scale = Rc::new(RefCell::new(1.0_f64));
    let scroll_ctrl = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
    let picture_scroll = picture.clone();
    let scale_scroll = scale.clone();
    scroll_ctrl.connect_scroll(move |_, _, dy| {
        let mut s = scale_scroll.borrow_mut();
        *s = (*s * if dy < 0.0 { 1.1 } else { 0.9 }).clamp(0.1, 10.0);
        picture_scroll.set_size_request((1200.0 * *s) as i32, (800.0 * *s) as i32);
        glib::Propagation::Stop
    });
    scrolled.add_controller(scroll_ctrl);

    toolbar_view.set_content(Some(&scrolled));
    window.set_content(Some(&toolbar_view));

    // Open file — GTK loads image, wgpu will process it next
    let picture_open = picture.clone();
    let window_ref = window.clone();
    let renderer_open = renderer.clone();
    open_btn.connect_clicked(move |_| {
        let dialog = FileDialog::builder()
            .title("Open Image")
            .modal(true)
            .build();

        let picture_clone = picture_open.clone();
        let _renderer = renderer_open.clone(); // wgpu processing hook — next step
        dialog.open(
            Some(&window_ref),
            gtk4::gio::Cancellable::NONE,
            move |result| {
                if let Ok(file) = result {
                    println!("Image opened: {:?}", file.path());
                    // GTK displays it natively for now
                    // Next step: pipe through wgpu for shader processing
                    picture_clone.set_file(Some(&file));
                }
            },
        );
    });

    // Keyboard shortcuts
    let key_ctrl = gtk4::EventControllerKey::new();
    let window_key = window.clone();
    key_ctrl.connect_key_pressed(move |_, key, _, _| match key {
        gtk4::gdk::Key::f | gtk4::gdk::Key::F => {
            window_key.fullscreen();
            glib::Propagation::Stop
        }
        gtk4::gdk::Key::Escape => {
            window_key.unfullscreen();
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    window.add_controller(key_ctrl);

    window.present();
}
