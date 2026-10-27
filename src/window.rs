//! The application window. It shows the interface from `ui/`, the same one
//! tuxhub shows, and connects it to the background service.

use crate::client;
use crate::integration::{APP_ID, DISPLAY_NAME, tool_root};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::ffi::{CString, c_char};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::unix::{EventLoopBuilderExtUnix, WindowExtUnix};
use tao::window::{Theme, WindowBuilder};
use wry::http::{Request, Response, StatusCode, header};
use wry::{WebViewBuilder, WebViewBuilderExtUnix};

const BRIDGE: &str = include_str!("bridge.js");
const STATUS_ID: &str = "window-status";
const SCHEME: &str = "tuxhub";

enum UserEvent {
    /// A JSON message for the page.
    Deliver(String),
    /// The page has loaded: the window may appear.
    Show,
}

/// How long the window stays hidden at most when the page does not finish
/// loading, so it never stays invisible.
const SHOW_FALLBACK: Duration = Duration::from_secs(3);

type Socket = Arc<Mutex<Option<UnixStream>>>;

pub fn run() -> io::Result<()> {
    let ui = ui_dir().ok_or_else(|| io::Error::other("the interface files (ui/) are missing"))?;
    let socket: Socket = Arc::new(Mutex::new(None));
    connect(&socket)?;

    set_program_name(APP_ID);
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event()
        .with_app_id(APP_ID)
        .build();
    let proxy = event_loop.create_proxy();
    {
        let (socket, proxy) = (socket.clone(), proxy.clone());
        std::thread::spawn(move || forward(&socket, &proxy));
    }

    let dark = desktop_prefers_dark();
    let window = WindowBuilder::new()
        .with_title(DISPLAY_NAME)
        .with_inner_size(LogicalSize::new(880.0, 760.0))
        .with_min_inner_size(LogicalSize::new(480.0, 420.0))
        .with_theme(dark.map(|d| if d { Theme::Dark } else { Theme::Light }))
        // Hidden until the page has loaded, so it never flashes white.
        .with_visible(false)
        .build(&event_loop)
        .map_err(io::Error::other)?;

    let initial = json!({ "dark": dark, "locale": desktop_locale() });
    let ipc_socket = socket.clone();
    let ipc_proxy = proxy.clone();
    let load_proxy = proxy.clone();
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SHOW_FALLBACK);
            let _ = proxy.send_event(UserEvent::Show);
        });
    }
    let vbox = window
        .default_vbox()
        .ok_or_else(|| io::Error::other("no window container"))?;
    let webview = WebViewBuilder::new()
        .with_initialization_script(format!("window.__tuxhubInitial = {initial};"))
        .with_custom_protocol(SCHEME.into(), move |_, req| serve(&ui, &req))
        .with_url(format!("{SCHEME}://localhost/index.html"))
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                let _ = load_proxy.send_event(UserEvent::Show);
            }
        })
        .with_ipc_handler(move |req: Request<String>| ipc(&ipc_socket, &ipc_proxy, req.body()))
        .with_navigation_handler(|url| url.starts_with(&format!("{SCHEME}://")))
        .with_new_window_req_handler(|url, _| {
            // Links open in the browser, never inside the window.
            if url.starts_with("https://") {
                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
            }
            wry::NewWindowResponse::Deny
        })
        .with_devtools(cfg!(debug_assertions))
        .build_gtk(vbox)
        .map_err(io::Error::other)?;

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Deliver(json)) => {
                let _ = webview.evaluate_script(&format!("window.__tuxhubReceive({json})"));
            }
            Event::UserEvent(UserEvent::Show) => {
                if !window.is_visible() {
                    window.set_visible(true);
                    window.set_focus();
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Sets the GLib program name. GTK 3 sends it to the compositor as the Wayland
/// app id, not the id of the event loop's application, and the launcher and
/// icon are found by that id. GTK keeps a name that is set before it starts.
fn set_program_name(name: &str) {
    unsafe extern "C" {
        // From GLib, which GTK links.
        fn g_set_prgname(prgname: *const c_char);
    }
    let name = CString::new(name).expect("the app id has no NUL byte");
    // GLib copies the string.
    unsafe { g_set_prgname(name.as_ptr()) };
}

fn ui_dir() -> Option<PathBuf> {
    let installed = tool_root().map(|r| r.join("ui"));
    let dev = cfg!(debug_assertions).then(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("ui"));
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|e| Some(e.parent()?.join("ui")));
    [installed, dev, beside_exe]
        .into_iter()
        .flatten()
        .find(|d| d.join("index.html").is_file())
}

fn connect(socket: &Socket) -> io::Result<()> {
    let stream = client::connect_or_start()?;
    let mut w = &stream;
    writeln!(w, "{}", json!({ "id": STATUS_ID, "method": "status" }))?;
    *socket.lock().unwrap() = Some(stream);
    Ok(())
}

/// Forwards messages from the service to the page and reconnects when the
/// service restarts.
fn forward(socket: &Socket, proxy: &EventLoopProxy<UserEvent>) {
    let mut failures = 0;
    loop {
        let reader = socket
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.try_clone().ok());
        if let Some(reader) = reader {
            failures = 0;
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let msg = if msg.get("id") == Some(&json!(STATUS_ID)) {
                    json!({ "method": "status", "params": msg.get("result") })
                } else {
                    msg
                };
                if proxy
                    .send_event(UserEvent::Deliver(msg.to_string()))
                    .is_err()
                {
                    return;
                }
            }
        }
        *socket.lock().unwrap() = None;
        let _ = proxy.send_event(UserEvent::Deliver(
            json!({ "method": "status", "params": { "state": "offline", "text": "Reconnecting", "running": false } })
                .to_string(),
        ));
        std::thread::sleep(Duration::from_millis(300 * (failures + 1).min(10)));
        if connect(socket).is_err() {
            failures += 1;
        }
    }
}

/// Handles a request of the bridge in the page.
fn ipc(socket: &Socket, proxy: &EventLoopProxy<UserEvent>, body: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(body) else {
        return;
    };
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let reply = |result: Result<Value, String>| {
        let m = match result {
            Ok(r) => json!({ "id": id, "result": r }),
            Err(e) => json!({ "id": id, "error": { "message": e } }),
        };
        let _ = proxy.send_event(UserEvent::Deliver(m.to_string()));
    };
    let request = match msg.get("op").and_then(Value::as_str) {
        Some("call") => {
            json!({ "id": id, "method": msg.get("method"), "params": msg.get("params") })
        }
        Some("action") => {
            json!({ "id": id, "method": "action", "params": { "id": msg.get("action") } })
        }
        // Settings live in the service; there are no tuxhub settings here.
        Some("getSettings") => return reply(Ok(json!({}))),
        Some("saveSettings") => return reply(Ok(json!(true))),
        _ => return reply(Err("unsupported request".into())),
    };
    let sent = socket
        .lock()
        .unwrap()
        .as_ref()
        .map(|mut s| writeln!(s, "{request}"));
    if !matches!(sent, Some(Ok(()))) {
        reply(Err(format!("the {DISPLAY_NAME} service is not available")));
    }
}

/// Serves files of the interface and the bridge script.
fn serve(ui: &Path, req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = req.uri().path().trim_start_matches('/');
    let (body, mime): (Option<Vec<u8>>, &str) = if path == "__tuxhub/bridge.js" {
        (Some(BRIDGE.as_bytes().to_vec()), "text/javascript")
    } else {
        let rel = Path::new(if path.is_empty() { "index.html" } else { path });
        let safe = rel.components().all(|c| matches!(c, Component::Normal(_)));
        let body = safe.then(|| std::fs::read(ui.join(rel)).ok()).flatten();
        (body, mime_type(rel))
    };
    match body {
        Some(b) => Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(
                header::CONTENT_SECURITY_POLICY,
                // Web Awesome loads its built-in icons from data: URLs with fetch().
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' data:",
            )
            .body(Cow::Owned(b))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(&b""[..]))
            .unwrap(),
    }
}

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

/// Language of the desktop session, "de" or "en".
fn desktop_locale() -> &'static str {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default();
    if lang.starts_with("de") { "de" } else { "en" }
}

/// Dark or light preference of the desktop, from the settings portal.
fn desktop_prefers_dark() -> Option<bool> {
    let out = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--timeout",
            "1",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.ReadOne",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // The reply looks like "(<uint32 1>,)": 1 is dark, 2 is light, 0 is no preference.
    match text.chars().rfind(char::is_ascii_digit)? {
        '1' => Some(true),
        '2' => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(ui: &Path, path: &str) -> Response<Cow<'static, [u8]>> {
        let req = Request::builder()
            .uri(format!("{SCHEME}://localhost{path}"))
            .body(Vec::new())
            .unwrap();
        serve(ui, &req)
    }

    #[test]
    fn serves_the_interface_and_the_bridge_only() {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let page = get(&ui, "/index.html");
        assert_eq!(page.status(), StatusCode::OK);
        let csp = page.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("connect-src 'self' data:"));
        assert_eq!(get(&ui, "/").status(), StatusCode::OK);
        assert_eq!(get(&ui, "/__tuxhub/bridge.js").status(), StatusCode::OK);
        assert_eq!(get(&ui, "/vendor/webawesome.js").status(), StatusCode::OK);
        for outside in ["/../Cargo.toml", "/%2e%2e/Cargo.toml", "/missing.js"] {
            assert_eq!(
                get(&ui, outside).status(),
                StatusCode::NOT_FOUND,
                "{outside}"
            );
        }
    }
}
