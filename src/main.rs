use context_file_concat::app;
use context_file_concat::app::file_dialog::NativeDialogService;
use context_file_concat::app::proxy::EventProxy;
use context_file_concat::config;
use std::sync::{Arc, Mutex};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

// Keep platform quirks isolated.
mod platform;

// Embedded UI-Assets – only compiled in Release
#[cfg(not(debug_assertions))]
mod web_assets;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Create the event loop and window
    // The loop itself only carries payload-free wake tokens; the actual UI events wait
    // in the transport's outbox (see `app::transport`).
    let event_loop = EventLoopBuilder::<app::transport::Wake>::with_user_event().build();

    // Load the persisted config exactly once; the window geometry is read from it.
    let initial_state = app::state::AppState::default();
    let (width, height) = initial_state.config.window_size;
    let (pos_x, pos_y) = initial_state.config.window_position;

    let window = WindowBuilder::new()
        .with_title("CFC - Context File Concatenator")
        .with_inner_size(tao::dpi::LogicalSize::new(width, height))
        .with_position(tao::dpi::LogicalPosition::new(pos_x, pos_y))
        .with_min_inner_size(tao::dpi::LogicalSize::new(900, 600))
        .build(&event_loop)
        .expect("Failed to build Window");

    let window = Arc::new(window);

    // macOS: ensure a (minimal) NSMenu exists before building the WebView.
    #[cfg(target_os = "macos")]
    platform::macos::menu::install_standard_menus("CFC");

    // Create the shared application state and the coalescing transport to the UI thread
    let (proxy, ui_events) = app::transport::channel(event_loop.create_proxy());
    let state = Arc::new(Mutex::new(initial_state));
    let dialog_service = Arc::new(NativeDialogService {});

    // To avoid code duplication, we define the handlers first.
    let ipc_handler_state = state.clone();
    let ipc_handler_proxy = proxy.clone();
    let ipc_handler_dialog = dialog_service.clone();
    let ipc_handler = move |request: wry::http::Request<String>| {
        app::handle_ipc_message(
            request.into_body(),
            ipc_handler_dialog.clone(),
            ipc_handler_proxy.clone(),
            ipc_handler_state.clone(),
        );
    };

    let drop_handler_state = state.clone();
    let drop_handler_proxy = proxy.clone();
    let drag_drop_handler = move |event| {
        use wry::DragDropEvent;
        match event {
            DragDropEvent::Enter { .. } => {
                drop_handler_proxy.send_event(app::events::UserEvent::DragStateChanged(true));
            }
            DragDropEvent::Drop { paths, .. } => {
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
            DragDropEvent::Leave => {
                drop_handler_proxy.send_event(app::events::UserEvent::DragStateChanged(false));
            }
            // `Over` fires continuously while hovering; the highlight is already on.
            _ => (),
        }
        true
    };

    // Build the WebView for DEBUG: Vite Dev-Server
    #[cfg(debug_assertions)]
    let webview_builder = {
        tracing::info!("Running in DEBUG mode, loading from Vite dev server.");
        WebViewBuilder::new()
            .with_url("http://localhost:1420")
            .with_devtools(true)
    };

    // RELEASE: Custom-Protocol (no Port, no file://-issues)
    #[cfg(not(debug_assertions))]
    let webview_builder = {
        use std::borrow::Cow;
        use wry::http::Response;

        tracing::info!(
            "Running in RELEASE mode, using embedded assets via custom scheme 'cfc://'."
        );

        let enable_devtools = std::env::var_os("CFC_DEVTOOLS").is_some();

        let scheme = "cfc".to_string();
        let handler = move |_webview_id: wry::WebViewId,
                            request: wry::http::Request<Vec<u8>>|
              -> Response<Cow<'static, [u8]>> {
            let path = request.uri().path().to_string();
            match web_assets::load(&path) {
                Some((bytes, content_type)) => Response::builder()
                    .status(200)
                    .header("Content-Type", content_type)
                    .body(bytes)
                    .expect("response build ok"),
                None => {
                    let not_found: &'static [u8] = b"Not Found";
                    Response::builder()
                        .status(404)
                        .header("Content-Type", "text/plain; charset=utf-8")
                        .body(Cow::Borrowed(not_found))
                        .expect("response build ok")
                }
            }
        };

        WebViewBuilder::new()
            .with_custom_protocol(scheme, handler)
            .with_url("cfc://index.html")
            .with_devtools(enable_devtools)
            // Navigation policy (RELEASE branch; append identically in DEBUG branch)
            .with_navigation_handler(|uri: String| {
                // Determine the scheme manually
                let scheme = uri.split(':').next().unwrap_or("");

                match scheme {
                    // Allow internally
                    "cfc" | "about" | "blob" | "data" => true,

                    // Open external links in the system browser
                    "http" | "https" => {
                        let _ = open::that(&uri);
                        false
                    }

                    // Block everything else (e.g., file://)
                    _ => false,
                }
            })
    };

    let webview = webview_builder
        .with_ipc_handler(ipc_handler)
        .with_drag_drop_handler(drag_drop_handler)
        // wry >= 0.53 allows downloads by default. The app never offers any, so keep
        // the previous behaviour and deny them.
        .with_download_started_handler(|_url, _destination| false)
        .build(&*window)
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
                    let scale = window_for_events.scale_factor();
                    let size = window_for_events.inner_size().to_logical::<f64>(scale);
                    let position = window_for_events
                        .outer_position()
                        .unwrap_or_default()
                        .to_logical::<f64>(scale);
                    state_guard.config.window_size = (size.width, size.height);
                    state_guard.config.window_position = (position.x, position.y);

                    if let Err(e) = config::settings::save_config(&state_guard.config, None) {
                        tracing::error!("Failed to save config on exit: {}", e);
                    }
                    *control_flow = ControlFlow::Exit;
                }
                // tao reports physical pixels; the config stores logical units because the
                // window is restored with Logical{Size,Position}. Mixing both makes the
                // window grow on every restart on HiDPI displays.
                WindowEvent::Resized(size) => {
                    let size = size.to_logical::<f64>(window_for_events.scale_factor());
                    let mut state_guard = state_for_events.lock().unwrap();
                    state_guard.config.window_size = (size.width, size.height);
                }
                WindowEvent::Moved(position) => {
                    let position = position.to_logical::<f64>(window_for_events.scale_factor());
                    let mut state_guard = state_for_events.lock().unwrap();
                    state_guard.config.window_position = (position.x, position.y);
                }
                _ => (),
            },
            Event::UserEvent(app::transport::Wake) => {
                // One wake announces exactly one queued event.
                if let Some(user_event) = ui_events.next_event() {
                    app::handle_user_event(user_event, &webview);
                }
            }
            _ => (),
        }
    });
}
