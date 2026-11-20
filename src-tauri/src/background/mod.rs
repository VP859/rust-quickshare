//! The minimal background process (Linux, [`Config::minimal_background`]).
//!
//! With the window closed, the whole app – GTK, WebKitGTK and its processes –
//! is replaced by `ferry-gui --background`: the node, local network
//! discovery, a tray icon ([`tray`]) and questions asked in notifications
//! ([`notify`], [`handler`]). It never initializes GTK or Tauri, so it needs a
//! fraction of the memory.
//!
//! Only one of the two runs the node at a time. The app hands over by
//! starting the background process and quitting ([`hand_over`]); the
//! background process waits until the app has exited. The other way, anything
//! that starts the app (the tray icon, the app menu, "Send with Ferry")
//! first asks a running background process to stop over its control socket
//! ([`stop_running`]) and waits until it has.

mod handler;
mod notify;
mod tray;

use ferry::config::Config;
use ferry::discovery::{Discovery, DiscoveryOptions};
use ferry::localsend::{LocalSend, LocalSendOptions};
use ferry::node::NodeEngine;
use ferry::transfer::service;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::node::AppState;

/// Argument that starts the background process instead of the app.
pub const BACKGROUND_ARG: &str = "--background";
/// `--after=PID`: the app that handed over; its node must be gone first.
const AFTER_ARG: &str = "--after=";
/// How long the background process waits for the app to exit, and the app
/// for the background process to stop.
const HANDOVER_TIMEOUT: Duration = Duration::from_secs(15);
/// Line asking the background process to stop (the only command).
const QUIT: &str = "quit";

/// The app, for handing over once it is idle ([`hand_over_when_idle`]).
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Longest socket path a Unix socket address takes (`sun_path` holds 108
/// bytes with the terminating zero).
const MAX_SOCKET_PATH: usize = 107;

/// Path of the control socket: in the user's runtime folder, which only the
/// user can open, or else in the configuration folder; `None` when neither
/// gives a path short enough for a socket.
fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| ferry::config::paths::config_dir().ok())
        .map(|dir| dir.join("ferry-background.sock"))
        .filter(|path| path.as_os_str().len() <= MAX_SOCKET_PATH)
}

/// Asks a running background process to stop and waits until it has (its node
/// is closed); does nothing when none runs. Called by the app before it starts.
pub fn stop_running() {
    let Some(path) = socket_path() else {
        return;
    };
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&path) else {
        return;
    };
    info!("Taking over from the background process");
    let _ = stream.set_read_timeout(Some(HANDOVER_TIMEOUT));
    if stream.write_all(format!("{QUIT}\n").as_bytes()).is_ok() {
        // The process closes the connection when it has stopped.
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest);
    }
}

/// Whether the background process can run here (see [`wanted`]).
pub fn available() -> bool {
    socket_path().is_some()
}

/// Remembers the app, so it can hand over once it is idle.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Whether the app should give way to the background process when its window
/// is closed: the user chose it, and there is a place for the control socket
/// (without it the app could not take the node back).
pub fn wanted(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .minimal_background
        .load(std::sync::atomic::Ordering::Relaxed)
        && available()
}

/// Starts the background process and quits the app; the background process
/// takes the node over once the app has exited.
///
/// # Errors
///
/// When the background process cannot be started (the app keeps running).
pub fn hand_over(app: &AppHandle) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg(BACKGROUND_ARG)
        .arg(format!("{AFTER_ARG}{}", std::process::id()))
        .stdin(std::process::Stdio::null())
        .spawn()?;
    info!("Handing over to the background process");
    app.exit(0);
    Ok(())
}

/// Hands over when the window is closed, the background process is wanted and
/// nothing needs the app any more (no transfer running, no question waiting).
/// Called when a transfer or a question ends.
pub fn hand_over_when_idle() {
    let Some(app) = APP.get() else {
        return;
    };
    let state = app.state::<AppState>();
    if !wanted(app) || app.get_webview_window("main").is_some() || state.is_busy() {
        return;
    }
    if let Err(e) = hand_over(app) {
        warn!("Could not start the background process: {e}");
    }
}

/// What ends the background process.
enum Stop {
    /// The app asks for the node; it learns that the node is closed when this
    /// connection closes.
    App(UnixStream),
    /// "Quit Ferry" in the tray menu.
    Quit,
}

/// Runs the background process until the app takes over or the user quits.
pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    // Two worker threads are plenty for waiting for files; the app uses one
    // per processor core.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Could not start the background process: {e}");
            return;
        }
    };
    if let Err(e) = runtime.block_on(serve()) {
        warn!("Background process: {e:#}");
    }
}

/// Waits for the app to exit, takes the node over and serves until stopped.
async fn serve() -> anyhow::Result<()> {
    let path = socket_path().ok_or_else(|| anyhow::anyhow!("no folder for the control socket"))?;
    if UnixStream::connect(&path).await.is_ok() {
        info!("A background process already runs");
        return Ok(());
    }
    wait_for_app().await;
    // A socket left behind by a process that did not stop cleanly.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;

    let config = Config::load().unwrap_or_else(|e| {
        warn!("Could not load the configuration: {e:#}");
        Config::default()
    });
    ferry::i18n::init(None, config.language.as_deref());
    ferry::history::enable_from_config(&config);
    ferry::transfer::control::set_speed_limit(config.speed_limit);
    ferry::transfer::metadata::set_strip_metadata(config.strip_metadata);

    let engine = start_engine().await?;
    info!("Background process receiving as {}", engine.node_id());

    let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
    let notifier = Arc::new(notify::Notifier::new().await);
    let handler = Arc::new(handler::BackgroundHandler::new(notifier.clone()));

    let discovery = start_discovery(&engine, &config);
    let localsend = if config.localsend {
        LocalSend::start(
            &engine,
            LocalSendOptions::from_config(&config),
            handler.clone(),
        )
        .await
        .inspect_err(|e| warn!("{e:#}"))
        .ok()
    } else {
        None
    };
    // Kept for the lifetime of the process: the icon goes with the connection.
    let _tray = tray::start(stop_tx.clone())
        .await
        .inspect_err(|e| warn!("No tray icon: {e:#}"))
        .ok();
    tokio::spawn(accept(listener, stop_tx));

    let stop = tokio::select! {
        () = service::serve(engine.clone(), handler) => None,
        stop = stop_rx.recv() => stop,
    };
    if let Some(localsend) = localsend {
        localsend.stopped().await;
    }
    drop(discovery);
    engine.close().await;
    let _ = std::fs::remove_file(&path);
    info!("Background process stopped");
    // The answer (and closing the connection) tells the app the node is free.
    if let Some(Stop::App(mut stream)) = stop {
        let _ = stream.write_all(b"stopped\n").await;
    }
    Ok(())
}

/// With `--after=PID`, waits until the app that handed over has exited and
/// freed the node's sockets.
async fn wait_for_app() {
    let Some(pid) = std::env::args().find_map(|a| a.strip_prefix(AFTER_ARG).map(str::to_owned))
    else {
        return;
    };
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let deadline = tokio::time::Instant::now() + HANDOVER_TIMEOUT;
    while proc.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Starts the node; retried for a few seconds while the app that just
/// exited still holds its port.
async fn start_engine() -> anyhow::Result<NodeEngine> {
    let mut attempt = 0;
    loop {
        match NodeEngine::with_persistent_identity().await {
            Ok(engine) => return Ok(engine),
            Err(e) if attempt < 10 => {
                attempt += 1;
                warn!("Node not started yet ({e:#}), retrying");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Starts local network discovery; removed devices that appear are told about
/// their removal, as in the app.
fn start_discovery(engine: &NodeEngine, config: &Config) -> Option<Discovery> {
    let discovery = Discovery::start(
        engine.clone(),
        &config.device_name(),
        DiscoveryOptions::from_config(config),
    )
    .inspect_err(|e| warn!("Local network discovery unavailable: {e:#}"))
    .ok()?;
    let mut watch = discovery.subscribe();
    let engine = engine.clone();
    tokio::spawn(async move {
        while watch.changed().await.is_ok() {
            let devices = watch.borrow_and_update().clone();
            let pending = Config::load()
                .map(|c| c.pending_unpair_ids())
                .unwrap_or_default();
            if pending.is_empty() {
                continue;
            }
            let told = service::deliver_unpairs(&engine, &devices, &pending).await;
            if told.is_empty() {
                continue;
            }
            let saved = Config::load().and_then(|mut config| {
                for id in &told {
                    config.pending_unpair.remove(id);
                }
                config.save()
            });
            if let Err(e) = saved {
                warn!("Could not save the configuration: {e:#}");
            }
        }
    });
    Some(discovery)
}

/// Accepts connections on the control socket: a `quit` line stops the process.
async fn accept(listener: UnixListener, stop: mpsc::UnboundedSender<Stop>) {
    while let Ok((stream, _)) = listener.accept().await {
        let mut line = String::new();
        let mut reader = BufReader::new(stream);
        let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await;
        if matches!(read, Ok(Ok(_))) && line.trim() == QUIT {
            let _ = stop.send(Stop::App(reader.into_inner()));
        }
    }
}

/// Starts the app (the tray icon was clicked); it stops this process when it
/// starts.
fn open_app() {
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .stdin(std::process::Stdio::null())
            .spawn()
    });
    if let Err(e) = started {
        warn!("Could not start Ferry: {e}");
    }
}
