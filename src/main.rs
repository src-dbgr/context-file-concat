use context_file_concat::app;
use std::sync::{Arc, Mutex};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Create the event loop and window
    let event_loop = EventLoopBuilder::<app::events::UserEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("CFC - Context File Concatenator")
        .with_inner_size(tao::dpi::LogicalSize::new(1400, 900))
        .with_min_inner_size(tao::dpi::LogicalSize::new(900, 600))
        .build(&event_loop)
        .expect("Failed to build Window");

    // Create the shared application state and the event loop proxy
    let proxy = event_loop.create_proxy();
    let state = Arc::new(Mutex::new(app::state::AppState::default()));

    // Load and prepare the HTML content for the WebView by injecting CSS and JS
    let html_content = include_str!("ui/index.html")
        .replace("/*INJECT_CSS*/", include_str!("ui/style.css"))
        .replace("/*INJECT_JS*/", include_str!("ui/dist/bundle.js"));

    // Clone resources needed for the file drop handler
    let proxy_for_drop = proxy.clone();
    let state_for_drop = state.clone();

    // Create the WebView
    let webview = WebViewBuilder::new(&window)
        .with_html(html_content)
        .with_ipc_handler(move |message: String| {
            app::handle_ipc_message(message, proxy.clone(), state.clone());
        })
        .with_file_drop_handler(move |event| {
            use wry::FileDropEvent;
            match event {
                FileDropEvent::Hovered { .. } => {
                    let _ =
                        proxy_for_drop.send_event(app::events::UserEvent::DragStateChanged(true));
                }
                FileDropEvent::Dropped { paths, .. } => {
                    let _ =
                        proxy_for_drop.send_event(app::events::UserEvent::DragStateChanged(false));
                    if let Some(path) = paths.first() {
                        app::tasks::start_scan_on_path(
                            path.clone(),
                            proxy_for_drop.clone(),
                            state_for_drop.clone(),
                        );
                    }
                }
                FileDropEvent::Cancelled => {
                    let _ =
                        proxy_for_drop.send_event(app::events::UserEvent::DragStateChanged(false));
                }
                _ => (),
            }
            true // Indicates that the event has been handled
        })
        .with_devtools(true)
        .build()
        .expect("Failed to build WebView");

    // Start the event loop
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(user_event) => {
                app::handle_user_event(user_event, &webview);
            }
            _ => (),
        }
    });
}
