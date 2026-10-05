mod app;
mod barcode;
mod fgl;
mod preview;
mod ticket;
mod transport;

use gpui_kit::*;

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys(app::key_bindings());

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1360.), px(880.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Ticketsmith".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            cx.spawn(async move |cx| {
                cx.open_window(options, |window, cx| {
                    let view = app::Ticketsmith::view(window, cx);
                    cx.new(|cx| component::Root::new(view, window, cx))
                })
                .expect("failed to open window");
            })
            .detach();
        });
}
