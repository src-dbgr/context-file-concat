use context_file_concat::app;
use context_file_concat::app::file_dialog::NativeDialogService;
use context_file_concat::config;
use std::sync::{Arc, Mutex};
use tao::{
    event::{Event, StartCause, WindowEvent},
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

    let initial_config = app::state::AppState::default().config;
    let (width, height) = initial_config.window_size;
    let (pos_x, pos_y) = initial_config.window_position;

    let window = WindowBuilder::new()
        .with_title("CFC - Context File Concatenator")
        .with_inner_size(tao::dpi::LogicalSize::new(width, height))
        .with_position(tao::dpi::LogicalPosition::new(pos_x, pos_y))
        .with_min_inner_size(tao::dpi::LogicalSize::new(900, 600))
        .build(&event_loop)
        .expect("Failed to build Window");

    let window = Arc::new(window);

    // Create the shared application state and the event loop proxy
    let proxy = event_loop.create_proxy();
    let state = Arc::new(Mutex::new(app::state::AppState::default()));
    let dialog_service = Arc::new(NativeDialogService {});

    let html_content = include_str!("ui/index.html")
        .replace("/*INJECT_CSS*/", include_str!("ui/style.css"))
        .replace("/*INJECT_JS*/", include_str!("ui/dist/bundle.js"));

    let proxy_for_ipc = proxy.clone();
    let state_for_ipc = state.clone();
    let dialog_for_ipc = dialog_service.clone();
    let proxy_for_drop = proxy.clone();
    let state_for_drop = state.clone();

    let webview = WebViewBuilder::new(&*window)
        .with_html(html_content)
        .with_ipc_handler(move |message: String| {
            app::handle_ipc_message(
                message,
                dialog_for_ipc.clone(),
                proxy_for_ipc.clone(),
                state_for_ipc.clone(),
            );
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
                        // Dropping a new path should always reset the state.
                        app::tasks::start_scan_on_path(
                            path.clone(),
                            proxy_for_drop.clone(),
                            state_for_drop.clone(),
                            false,
                        );
                    }
                }
                FileDropEvent::Cancelled => {
                    let _ =
                        proxy_for_drop.send_event(app::events::UserEvent::DragStateChanged(false));
                }
                _ => (),
            }
            true
        })
        .with_devtools(true)
        .build()
        .expect("Failed to build WebView");

    let state_for_events = state.clone();
    let window_for_events = window.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                tracing::info!("Application initialized.");
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    tracing::info!("Close requested. Saving final window state...");
                    let mut state_guard = state_for_events.lock().unwrap();
                    let size = window_for_events.inner_size();
                    let position = window_for_events.outer_position().unwrap_or_default();
                    state_guard.config.window_size = (size.width.into(), size.height.into());
                    state_guard.config.window_position = (position.x.into(), position.y.into());

                    if let Err(e) = config::settings::save_config(&state_guard.config, None) {
                        tracing::error!("Failed to save config on exit: {}", e);
                    }
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Resized(size) => {
                    let mut state_guard = state_for_events.lock().unwrap();
                    state_guard.config.window_size = (size.width.into(), size.height.into());
                }
                WindowEvent::Moved(position) => {
                    let mut state_guard = state_for_events.lock().unwrap();
                    state_guard.config.window_position = (position.x.into(), position.y.into());
                }
                _ => (),
            },
            Event::UserEvent(user_event) => {
                app::handle_user_event(user_event, &webview);
            }
            _ => (),
        }
    });
}
