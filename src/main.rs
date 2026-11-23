use context_file_concat::app;
use context_file_concat::app::file_dialog::NativeDialogService;
use context_file_concat::app::proxy::EventProxy;
use context_file_concat::app::shared_state::SharedState;
use context_file_concat::app::state::AppState;
use context_file_concat::app::window_geometry::{
    GeometryStore, Rect, SaveDebounce, GEOMETRY_SAVE_DELAY, MIN_WINDOW_SIZE,
};
use context_file_concat::config::{settings, AppConfig, ConfigStore};
use std::sync::Arc;
use std::time::Instant;
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
use platform::origin::AppOrigin;

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

    // This is the only place that decides which files the application keeps its own
    // data in: the configuration, and beside it the geometry of the window.
    let config_path = settings::default_config_path();
    if config_path.is_none() {
        tracing::warn!("No config directory available. Settings will not be persisted.");
    }

    // The window geometry belongs to this function, not to the application state: it
    // is read here (taking over what an older version kept in the config file, before
    // anything can rewrite that file), follows the window in the event loop below and
    // is saved a moment after the window has come to rest and when the loop ends. For the start it is pulled back onto a visible
    // monitor if necessary.
    let (geometry_store, stored_geometry) = match &config_path {
        Some(path) => GeometryStore::open(path, primary_scale_factor(&event_loop)),
        None => (GeometryStore::detached(), Default::default()),
    };
    let mut geometry = stored_geometry.clamped_to_visible(&logical_monitors(&event_loop));

    // Load the persisted config exactly once; from here on the store is the way the
    // config gets saved.
    let (config_store, config) = match config_path {
        Some(path) => ConfigStore::open(path),
        None => (ConfigStore::detached(), AppConfig::default()),
    };
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
    let state = SharedState::new(AppState::new(config, config_store.clone()));
    // The dialogs belong to the window (the handle has to be taken here, on the main
    // thread), so they cannot end up behind it.
    let dialog_service = Arc::new(NativeDialogService::parented_to(&*window));

    // To avoid code duplication, we define the handlers first.
    let ipc_handler_state = state.clone();
    let ipc_handler_proxy = proxy.clone();
    let ipc_handler_dialog = dialog_service.clone();
    // Executes IPC commands in arrival order; must be created inside the Tokio runtime.
    let ipc_dispatcher = app::dispatch::CommandDispatcher::new();
    // Drops go through the same dispatcher, so they are ordered with the IPC commands.
    let drop_dispatcher = ipc_dispatcher.clone();
    // The origin the UI is loaded from, which is also the only one commands are taken
    // from: the Vite dev server in a debug build, the embedded assets in a release one.
    #[cfg(debug_assertions)]
    let origin = AppOrigin::dev_server();
    #[cfg(not(debug_assertions))]
    let origin = AppOrigin::embedded();

    let ipc_handler = move |request: wry::http::Request<String>| {
        // Only the UI itself may send commands (see `platform::ipc`).
        let Some(message) = platform::ipc::accepted_message(&origin, request) else {
            return;
        };
        app::dispatch_ipc_message(
            &ipc_dispatcher,
            message,
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
                    app::dispatch_dropped_path(
                        &drop_dispatcher,
                        path.clone(),
                        drop_handler_proxy.clone(),
                        drop_handler_state.clone(),
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
            .with_url(origin.start_url())
            .with_devtools(true)
    };

    // RELEASE: Custom-Protocol (no Port, no file://-issues)
    #[cfg(not(debug_assertions))]
    let webview_builder = {
        use platform::asset_protocol;
        use platform::navigation::NavigationDecision;

        tracing::info!(
            "Running in RELEASE mode, using embedded assets via custom scheme 'cfc://'."
        );

        let enable_devtools = std::env::var_os("CFC_DEVTOOLS").is_some();

        // Sent with every response: the policy is decided here, not in the markup.
        let csp = origin.content_security_policy();
        let handler = move |_webview_id: wry::WebViewId, request: wry::http::Request<Vec<u8>>| {
            asset_protocol::respond(web_assets::load(request.uri().path()), &csp)
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

    let window_for_events = window.clone();
    let mut session_saved = false;
    // The geometry is also saved while the application runs, so that a crash does not
    // cost it: once the window has rested after a resize/move, on the blocking pool.
    let mut save_debounce = SaveDebounce::new(GEOMETRY_SAVE_DELAY);
    let runtime = tokio::runtime::Handle::current();

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
                // The geometry is tracked here, outside the application state: a
                // worker holding the state lock cannot make resizing/moving stutter.
                WindowEvent::Resized(size) => {
                    let scale = window_for_events.scale_factor();
                    if geometry.record_size((size.width.into(), size.height.into()), scale) {
                        save_debounce.changed(Instant::now());
                    }
                }
                WindowEvent::Moved(position) => {
                    let scale = window_for_events.scale_factor();
                    if geometry.record_position((position.x.into(), position.y.into()), scale) {
                        save_debounce.changed(Instant::now());
                    }
                }
                _ => (),
            },
            // The one event every way of quitting ends in: closing the window
            // (`ControlFlow::Exit`) as well as Cmd+Q / the Quit menu item on macOS,
            // which terminate the application without a `CloseRequested`.
            //
            // Both saves happen here and now instead of in the background (the process
            // ends when this returns), and neither waits for the application state.
            Event::LoopDestroyed if !session_saved => {
                session_saved = true;
                save_debounce.cancel();
                tracing::info!("Exiting. Saving final window state...");
                if let Err(e) = geometry_store.save(geometry) {
                    tracing::error!("Failed to save the window geometry on exit: {e}");
                }
                // A change of the config whose background save has not run yet is
                // written now; whatever is still queued is skipped afterwards.
                if let Err(e) = config_store.close() {
                    tracing::error!("Failed to save config on exit: {e}");
                }
            }
            Event::UserEvent(app::transport::Wake) => {
                // One wake announces exactly one queued event.
                if let Some(user_event) = ui_events.next_event() {
                    app::handle_user_event(user_event, &webview);
                }
            }
            // Among others the wake-up the pending save has asked for (see below).
            _ => (),
        }

        // The window has come to rest: its geometry is written in the background. The
        // store serializes its writes and always writes the geometry it was handed
        // last, so this cannot get in the way of a later save or of the one at exit.
        if save_debounce.take_due(Instant::now()) {
            let store = geometry_store.clone();
            runtime.spawn_blocking(move || {
                if let Err(e) = store.save(geometry) {
                    tracing::warn!("Failed to save the window geometry: {e}");
                }
            });
        }
        if let (Some(due_at), ControlFlow::Wait) = (save_debounce.due_at(), *control_flow) {
            *control_flow = ControlFlow::WaitUntil(due_at);
        }
    });
}

/// The scale factor of the primary monitor (of any monitor if none is primary); `1.0`
/// if the system reports no monitor at all.
fn primary_scale_factor<T>(event_loop: &EventLoopWindowTarget<T>) -> f64 {
    event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())
        .map_or(1.0, |monitor| monitor.scale_factor())
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
