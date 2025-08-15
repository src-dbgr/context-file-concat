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

    // To avoid code duplication, we define the handlers first.
    let ipc_handler_state = state.clone();
    let ipc_handler_proxy = proxy.clone();
    let ipc_handler_dialog = dialog_service.clone();
    let ipc_handler = move |message: String| {
        app::handle_ipc_message(
            message,
            ipc_handler_dialog.clone(),
            ipc_handler_proxy.clone(),
            ipc_handler_state.clone(),
        );
    };

    let drop_handler_state = state.clone();
    let drop_handler_proxy = proxy.clone();
    let file_drop_handler = move |event| {
        use wry::FileDropEvent;
        match event {
            FileDropEvent::Hovered { .. } => {
                let _ =
                    drop_handler_proxy.send_event(app::events::UserEvent::DragStateChanged(true));
            }
            FileDropEvent::Dropped { paths, .. } => {
                let _ =
                    drop_handler_proxy.send_event(app::events::UserEvent::DragStateChanged(false));
                if let Some(path) = paths.first() {
                    app::tasks::start_scan_on_path(
                        path.clone(),
                        drop_handler_proxy.clone(),
                        drop_handler_state.clone(),
                        false,
                    );
                }
            }
            FileDropEvent::Cancelled => {
                let _ =
                    drop_handler_proxy.send_event(app::events::UserEvent::DragStateChanged(false));
            }
            _ => (),
        }
        true
    };

    // Now, build the WebView differently for debug and release builds.
    #[cfg(debug_assertions)]
    let webview_builder = {
        // In debug builds, load from the Vite dev server.
        tracing::info!("Running in DEBUG mode, loading from Vite dev server.");
        WebViewBuilder::new(&*window)
            .with_url("http://localhost:1420")
            .with_devtools(true)
    };

    #[cfg(not(debug_assertions))]
    let webview_builder = {
        // In release builds, load the static HTML file produced by `npm run build`.
        tracing::info!("Running in RELEASE mode, loading from bundled assets.");
        let html_content = include_str!("ui/dist/index.html");
        WebViewBuilder::new(&*window)
            .with_html(html_content)
            .with_devtools(false) // Devtools are disabled in release
    };

    let webview = webview_builder
        .with_ipc_handler(ipc_handler)
        .with_file_drop_handler(file_drop_handler)
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
