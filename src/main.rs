use context_file_concat::app;
use context_file_concat::app::file_dialog::NativeDialogService;
use context_file_concat::app::proxy::EventProxy;
use context_file_concat::app::window_geometry::{self, Rect, WindowGeometry, MIN_WINDOW_SIZE};
use std::sync::{Arc, Mutex};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget},
    monitor::MonitorHandle,
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

    // Load the persisted config exactly once; the window geometry is read from it and
    // pulled back onto a visible monitor if necessary.
    let initial_state = app::state::AppState::default();
    let mut geometry = WindowGeometry::from_config(&initial_state.config)
        .clamped_to_visible(&logical_monitors(&event_loop));
    let (min_width, min_height) = MIN_WINDOW_SIZE;

    let window = WindowBuilder::new()
        .with_title("CFC - Context File Concatenator")
        .with_inner_size(LogicalSize::new(geometry.size.0, geometry.size.1))
        .with_position(LogicalPosition::new(
            geometry.position.0,
            geometry.position.1,
        ))
        .with_min_inner_size(LogicalSize::new(min_width, min_height))
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
    // Executes IPC commands in arrival order; must be created inside the Tokio runtime.
    let ipc_dispatcher = app::dispatch::CommandDispatcher::new();
    let ipc_handler = move |request: wry::http::Request<String>| {
        app::dispatch_ipc_message(
            &ipc_dispatcher,
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
        use platform::navigation::{AppOrigin, NavigationDecision, OriginForm};
        use std::borrow::Cow;
        use wry::http::Response;

        tracing::info!(
            "Running in RELEASE mode, using embedded assets via custom scheme 'cfc://'."
        );

        let enable_devtools = std::env::var_os("CFC_DEVTOOLS").is_some();

        let origin = AppOrigin::new("cfc", "index.html", OriginForm::for_current_platform());
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
            .with_custom_protocol(origin.scheme().to_string(), handler)
            .with_url(origin.start_url())
            .with_devtools(enable_devtools)
            // Navigation policy (RELEASE branch only; see `platform::navigation`)
            .with_navigation_handler(move |uri: String| match origin.classify(&uri) {
                NavigationDecision::Allow => true,
                NavigationDecision::OpenExternally => {
                    let _ = open::that(&uri);
                    false
                }
                NavigationDecision::Block => false,
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
    let mut geometry_saved = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                tracing::info!("Application initialized.");
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    tracing::info!("Close requested.");
                    *control_flow = ControlFlow::Exit;
                }
                // The geometry is tracked outside the application state: a worker
                // holding the state lock must not make resizing/moving stutter.
                WindowEvent::Resized(size) => {
                    let scale = window_for_events.scale_factor();
                    geometry.record_size((size.width.into(), size.height.into()), scale);
                    geometry.sync_if_uncontended(&state_for_events);
                }
                WindowEvent::Moved(position) => {
                    let scale = window_for_events.scale_factor();
                    geometry.record_position((position.x.into(), position.y.into()), scale);
                    geometry.sync_if_uncontended(&state_for_events);
                }
                _ => (),
            },
            // The one event every way of quitting ends in: closing the window
            // (`ControlFlow::Exit`) as well as Cmd+Q / the Quit menu item on macOS,
            // which terminate the application without a `CloseRequested`.
            Event::LoopDestroyed if !geometry_saved => {
                geometry_saved = true;
                tracing::info!("Exiting. Saving final window state...");
                if let Err(e) = window_geometry::persist(geometry, &state_for_events, None) {
                    tracing::error!("Failed to save config on exit: {}", e);
                }
            }
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

/// The available monitors in logical units, the primary one first.
fn logical_monitors<T>(event_loop: &EventLoopWindowTarget<T>) -> Vec<Rect> {
    let to_rect = |monitor: MonitorHandle| {
        let (position, size) = (monitor.position(), monitor.size());
        Rect::from_physical(
            position.x.into(),
            position.y.into(),
            size.width.into(),
            size.height.into(),
            monitor.scale_factor(),
        )
    };
    // The primary monitor is listed again by `available_monitors`; the duplicate is
    // harmless and keeps it in front as the fallback target.
    event_loop
        .primary_monitor()
        .into_iter()
        .chain(event_loop.available_monitors())
        .filter_map(to_rect)
        .collect()
}
