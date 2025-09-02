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

// Keep platform quirks isolated.
mod platform;

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

    // macOS: ensure a (minimal) NSMenu exists before building the WebView.
    #[cfg(target_os = "macos")]
    platform::macos::menu::install_standard_menus("CFC");

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

    // Build the WebView for DEBUG: Vite Dev-Server
    #[cfg(debug_assertions)]
    let webview_builder = {
        tracing::info!("Running in DEBUG mode, loading from Vite dev server.");
        WebViewBuilder::new(&*window)
            .with_url("http://localhost:1420")
            .with_devtools(true)
    };

    // Build the WebView for RELEASE: kleiner eingebauter Static-HTTP-Server
    #[cfg(not(debug_assertions))]
    let webview_builder = {
        use std::{
            io::Read,
            net::TcpListener,
            path::{Path, PathBuf},
        };

        tracing::info!(
            "Running in RELEASE mode, serving UI over http://127.0.0.1:<port> from bundled assets."
        );

        // Pfad zum dist-Ordner im Bundle ermitteln
        fn dist_dir() -> PathBuf {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| std::env::current_dir().unwrap());

            #[cfg(target_os = "macos")]
            let p = exe_dir.join("../Resources/src/ui/dist");
            #[cfg(not(target_os = "macos"))]
            let p = exe_dir.join("resources").join("src/ui/dist");
            p
        }

        let ui_root = dist_dir();
        assert!(
            ui_root.join("index.html").exists(),
            "UI dist not found at {:?}",
            ui_root
        );

        // Listener auf Port 0 (OS sucht freien Port)
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("Failed to bind 127.0.0.1:0 for UI server");
        let port = listener.local_addr().expect("No local addr").port();

        // HTTP-Server im Hintergrund-Thread starten
        let ui_root_clone = ui_root.clone();
        std::thread::spawn(move || {
            let server =
                tiny_http::Server::from_listener(listener, None).expect("http server start failed");
            for req in server.incoming_requests() {
                let mut path = req.url().trim_start_matches('/').to_string();
                if path.is_empty() {
                    path = "index.html".into();
                }

                // Verzeichnisse auf index.html mappen
                let mut fs_path = ui_root_clone.join(Path::new(&path));
                if fs_path.is_dir() {
                    fs_path = fs_path.join("index.html");
                }

                match std::fs::File::open(&fs_path) {
                    Ok(mut f) => {
                        let mut buf = Vec::new();
                        if f.read_to_end(&mut buf).is_ok() {
                            let mime = mime_guess::from_path(&fs_path).first_or_octet_stream();
                            let mut resp = tiny_http::Response::from_data(buf);
                            let _ = resp.add_header(
                                tiny_http::Header::from_bytes(
                                    &b"Content-Type"[..],
                                    mime.essence_str(),
                                )
                                .unwrap(),
                            );
                            // Einfaches Caching (optional)
                            let _ = resp.add_header(
                                tiny_http::Header::from_bytes(
                                    &b"Cache-Control"[..],
                                    "public, max-age=31536000",
                                )
                                .unwrap(),
                            );
                            let _ = req.respond(resp);
                        } else {
                            let _ = req.respond(tiny_http::Response::new_empty(
                                tiny_http::StatusCode(500),
                            ));
                        }
                    }
                    Err(_) => {
                        // SPA-Fallback auf index.html
                        let index = ui_root_clone.join("index.html");
                        if let Ok(mut f) = std::fs::File::open(&index) {
                            let mut buf = Vec::new();
                            let _ = f.read_to_end(&mut buf);
                            let mut resp = tiny_http::Response::from_data(buf);
                            let _ = resp.add_header(
                                tiny_http::Header::from_bytes(
                                    &b"Content-Type"[..],
                                    "text/html; charset=utf-8",
                                )
                                .unwrap(),
                            );
                            let _ = req.respond(resp);
                        } else {
                            let _ = req.respond(tiny_http::Response::new_empty(
                                tiny_http::StatusCode(404),
                            ));
                        }
                    }
                }
            }
        });

        let enable_devtools = std::env::var_os("CFC_DEVTOOLS").is_some();
        let url = format!("http://127.0.0.1:{}/index.html", port);
        tracing::info!("Serving UI at {}", url);

        WebViewBuilder::new(&*window)
            .with_url(&url)
            .with_devtools(enable_devtools)
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
