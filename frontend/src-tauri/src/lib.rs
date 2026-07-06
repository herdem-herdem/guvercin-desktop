use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{
  menu::{Menu, MenuItem, PredefinedMenuItem},
  tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
  Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_log::{Target, TargetKind};

/// Label of the primary window. Tauri assigns "main" to the first window
/// declared in tauri.conf.json.
const MAIN_WINDOW_LABEL: &str = "main";
const TRAY_ID: &str = "guvercin-tray";

/// Shared state holding the port the Rust backend is listening on.
/// Set once during app setup; read by the `get_backend_port` command.
#[derive(Default)]
struct BackendPort(Mutex<Option<u16>>);

#[cfg(any(
  target_os = "linux",
  target_os = "dragonfly",
  target_os = "freebsd",
  target_os = "netbsd",
  target_os = "openbsd"
))]
fn disable_native_webview_context_menus(window: &tauri::WebviewWindow) {
  use webkit2gtk::{ContextMenuExt, HitTestResultExt, WebViewExt};

  let _ = window.with_webview(|webview| {
    let wv = webview.inner();
    wv.connect_context_menu(|_, menu, _, hit_test| {
      // Allow native menu for editable fields (copy/paste), disable everywhere else.
      if hit_test.context_is_editable() {
        return false;
      }

      // Extra safety: clear any items that might have been appended by default handlers.
      menu.remove_all();
      true
    });
  });
}

/// Shared state that maps window labels → mail data JSON.
/// The new window calls `get_mail_window_data` to consume its entry.
#[derive(Default)]
struct MailWindowStore(Mutex<HashMap<String, String>>);

/// Shared state that maps window labels → compose data JSON.
#[derive(Default)]
struct ComposeWindowStore(Mutex<HashMap<String, String>>);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LinkClickBehavior {
  Ask,
  Open,
  Copy,
}

impl Default for LinkClickBehavior {
  fn default() -> Self {
    Self::Ask
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Preferences {
  #[serde(default)]
  link_click_behavior: LinkClickBehavior,
  #[serde(default)]
  domain_behaviors: HashMap<String, LinkClickBehavior>,
}

impl Default for Preferences {
  fn default() -> Self {
    Self {
      link_click_behavior: LinkClickBehavior::Ask,
      domain_behaviors: HashMap::new(),
    }
  }
}

struct PreferencesStore {
  path: PathBuf,
  prefs: Mutex<Preferences>,
}

impl PreferencesStore {
  fn load(path: PathBuf) -> Self {
    let prefs = match fs::read_to_string(&path) {
      Ok(raw) => serde_json::from_str::<Preferences>(&raw).unwrap_or_default(),
      Err(_) => Preferences::default(),
    };
    Self {
      path,
      prefs: Mutex::new(prefs),
    }
  }

  fn set_behavior(&self, behavior: LinkClickBehavior) -> Result<(), String> {
    {
      let mut guard = self.prefs.lock().unwrap();
      guard.link_click_behavior = behavior;
    }
    self.persist()
  }

  fn get_behavior(&self) -> LinkClickBehavior {
    self.prefs.lock().unwrap().link_click_behavior
  }

  fn set_domain_behavior(&self, domain: String, behavior: LinkClickBehavior) -> Result<(), String> {
    {
      let mut guard = self.prefs.lock().unwrap();
      guard.domain_behaviors.insert(domain, behavior);
    }
    self.persist()
  }

  fn get_domain_behavior(&self, domain: &str) -> Option<LinkClickBehavior> {
    self.prefs.lock().unwrap().domain_behaviors.get(domain).copied()
  }

  fn remove_domain_behavior(&self, domain: &str) -> Result<(), String> {
    {
      let mut guard = self.prefs.lock().unwrap();
      guard.domain_behaviors.remove(domain);
    }
    self.persist()
  }

  fn get_all_domain_behaviors(&self) -> HashMap<String, LinkClickBehavior> {
    self.prefs.lock().unwrap().domain_behaviors.clone()
  }

  fn persist(&self) -> Result<(), String> {
    if let Some(parent) = self.path.parent() {
      fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = {
      let guard = self.prefs.lock().unwrap();
      serde_json::to_string_pretty(&*guard).map_err(|e| e.to_string())?
    };
    fs::write(&self.path, raw).map_err(|e| e.to_string())?;
    Ok(())
  }
}

fn parse_behavior(input: &str) -> Option<LinkClickBehavior> {
  match input.trim().to_lowercase().as_str() {
    "ask" => Some(LinkClickBehavior::Ask),
    "open" => Some(LinkClickBehavior::Open),
    "copy" => Some(LinkClickBehavior::Copy),
    _ => None,
  }
}

fn is_allowed_external_url(url: &str) -> bool {
  let u = url.trim();
  u.starts_with("http://")
    || u.starts_with("https://")
    || u.starts_with("mailto:")
    || u.starts_with("tel:")
}

#[tauri::command]
async fn open_mail_window(
  handle: tauri::AppHandle,
  label: String,
  mail_data_json: String,
) -> Result<(), String> {
  let label = if label.trim().is_empty() {
    "mail".to_string()
  } else {
    label
  };

  if let Some(window) = handle.get_webview_window(&label) {
    window.show().map_err(|e| e.to_string())?;
    let _ = window.set_focus();
    return Ok(());
  }

  // Store the payload in shared app state so the new window can retrieve it
  // via `get_mail_window_data`. We cannot use localStorage (isolated per webview)
  // or URL query parameters (PathBuf strips them on App protocol).
  {
    let store = handle.state::<MailWindowStore>();
    let mut map = store.0.lock().unwrap();
    map.insert(label.clone(), mail_data_json);
  }

  let init_script = format!(
    "window.__GUV_DETACHED__ = {{ kind: 'mail', label: {} }};",
    serde_json::to_string(&label).unwrap_or_else(|_| "\"\"".to_string())
  );

  WebviewWindowBuilder::new(
    &handle,
    &label,
    WebviewUrl::App(PathBuf::from("index.html")),
  )
  .title("Guvercin - Mail")
  .initialization_script(init_script)
  .visible(true)
  // Use a sensible default size for detached mail windows and a comfortable
  // minimum so users don't resize to an unusably small viewport.
  .inner_size(1200.0, 800.0)
  .min_inner_size(900.0, 640.0)
  .build()
  .map_err(|e| e.to_string())
  .map(|window| {
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    disable_native_webview_context_menus(&window);
    #[cfg(not(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    )))]
    let _ = window;
  })?;

  Ok(())
}

/// Called by the new window on startup to fetch its mail data.
#[tauri::command]
fn get_mail_window_data(
  label: String,
  store: State<'_, MailWindowStore>,
) -> Option<String> {
  let map = store.0.lock().unwrap();
  map.get(&label).cloned()
}

#[tauri::command]
fn close_mail_window(handle: tauri::AppHandle, label: String) -> Result<(), String> {
  let label = if label.trim().is_empty() {
    "mail".to_string()
  } else {
    label
  };

  {
    let store = handle.state::<MailWindowStore>();
    let mut map = store.0.lock().unwrap();
    map.remove(&label);
  }

  if let Some(window) = handle.get_webview_window(&label) {
    let _ = window.close();
  }
  Ok(())
}

#[tauri::command]
async fn open_compose_window(
  handle: tauri::AppHandle,
  label: String,
  compose_data_json: String,
) -> Result<(), String> {
  let label = if label.trim().is_empty() {
    "compose".to_string()
  } else {
    label
  };

  if let Some(window) = handle.get_webview_window(&label) {
    window.show().map_err(|e| e.to_string())?;
    let _ = window.set_focus();
    return Ok(());
  }

  {
    let store = handle.state::<ComposeWindowStore>();
    let mut map = store.0.lock().unwrap();
    map.insert(label.clone(), compose_data_json);
  }

  let init_script = format!(
    "window.__GUV_DETACHED__ = {{ kind: 'compose', label: {} }};",
    serde_json::to_string(&label).unwrap_or_else(|_| "\"\"".to_string())
  );

  WebviewWindowBuilder::new(
    &handle,
    &label,
    WebviewUrl::App(PathBuf::from("index.html")),
  )
  .title("Guvercin - Compose")
  .initialization_script(init_script)
  .visible(true)
  .inner_size(800.0, 650.0)
  .build()
  .map_err(|e| e.to_string())
  .map(|window| {
    #[cfg(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    ))]
    disable_native_webview_context_menus(&window);
    #[cfg(not(any(
      target_os = "linux",
      target_os = "dragonfly",
      target_os = "freebsd",
      target_os = "netbsd",
      target_os = "openbsd"
    )))]
    let _ = window;
  })?;

  Ok(())
}

#[tauri::command]
fn get_compose_window_data(
  label: String,
  store: State<'_, ComposeWindowStore>,
) -> Option<String> {
  let mut map = store.0.lock().unwrap();
  map.remove(&label)
}

#[tauri::command]
fn close_compose_window(handle: tauri::AppHandle, label: String) -> Result<(), String> {
  let label = if label.trim().is_empty() {
    "compose".to_string()
  } else {
    label
  };
  if let Some(window) = handle.get_webview_window(&label) {
    let _ = window.close();
  }
  Ok(())
}

#[tauri::command]
fn save_export_file_to_path(path: String, bytes: Vec<u8>) -> Result<(), String> {
  let path = PathBuf::from(path);
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
  }

  fs::write(&path, bytes).map_err(|e| e.to_string())?;
  Ok(())
}

#[tauri::command]
fn get_link_click_behavior(store: State<'_, PreferencesStore>) -> String {
  match store.get_behavior() {
    LinkClickBehavior::Ask => "ask".to_string(),
    LinkClickBehavior::Open => "open".to_string(),
    LinkClickBehavior::Copy => "copy".to_string(),
  }
}

#[tauri::command]
fn set_link_click_behavior(
  behavior: String,
  store: State<'_, PreferencesStore>,
) -> Result<(), String> {
  let parsed = parse_behavior(&behavior).ok_or_else(|| "Invalid behavior".to_string())?;
  store.set_behavior(parsed)
}

#[tauri::command]
fn get_domain_link_behavior(domain: String, store: State<'_, PreferencesStore>) -> Option<String> {
  store.get_domain_behavior(&domain).map(|b| match b {
    LinkClickBehavior::Ask => "ask".to_string(),
    LinkClickBehavior::Open => "open".to_string(),
    LinkClickBehavior::Copy => "copy".to_string(),
  })
}

#[tauri::command]
fn set_domain_link_behavior(
  domain: String,
  behavior: String,
  store: State<'_, PreferencesStore>,
) -> Result<(), String> {
  let parsed = parse_behavior(&behavior).ok_or_else(|| "Invalid behavior".to_string())?;
  store.set_domain_behavior(domain, parsed)
}

#[tauri::command]
fn remove_domain_link_behavior(
  domain: String,
  store: State<'_, PreferencesStore>,
) -> Result<(), String> {
  store.remove_domain_behavior(&domain)
}

#[tauri::command]
fn get_all_domain_link_behaviors(
  store: State<'_, PreferencesStore>,
) -> HashMap<String, String> {
  store
    .get_all_domain_behaviors()
    .into_iter()
    .map(|(k, v)| {
      let v_str = match v {
        LinkClickBehavior::Ask => "ask".to_string(),
        LinkClickBehavior::Open => "open".to_string(),
        LinkClickBehavior::Copy => "copy".to_string(),
      };
      (k, v_str)
    })
    .collect()
}

/// Reads a `.eml`/`.msg` file the OS opened us with (via file association) and
/// returns its contents base64-encoded. Accepts either a plain filesystem path
/// or a `file://` URL (macOS delivers file associations as `file://` deep links).
/// Only message file extensions are allowed so this can't be used to read
/// arbitrary files off disk.
#[tauri::command]
fn read_eml_file(path: String) -> Result<String, String> {
  use base64::Engine as _;

  let raw = path.trim();
  // Normalize a `file://` URL into a filesystem path.
  let path_str = if let Some(rest) = raw.strip_prefix("file://") {
    // Drop an optional host component (file://host/path -> /path).
    let rest = match rest.find('/') {
      Some(idx) => &rest[idx..],
      None => rest,
    };
    percent_decode(rest)
  } else {
    raw.to_string()
  };

  let path = PathBuf::from(&path_str);
  let ext = path
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| e.to_ascii_lowercase())
    .unwrap_or_default();
  if ext != "eml" && ext != "msg" {
    return Err("Only .eml or .msg files can be opened".to_string());
  }

  let bytes = fs::read(&path).map_err(|e| e.to_string())?;
  Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Minimal percent-decoder for `file://` URL paths (handles %20 etc.).
fn percent_decode(input: &str) -> String {
  let bytes = input.as_bytes();
  let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%' && i + 2 < bytes.len() {
      let hi = (bytes[i + 1] as char).to_digit(16);
      let lo = (bytes[i + 2] as char).to_digit(16);
      if let (Some(hi), Some(lo)) = (hi, lo) {
        out.push((hi * 16 + lo) as u8);
        i += 3;
        continue;
      }
    }
    out.push(bytes[i]);
    i += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}

#[tauri::command]
fn get_backend_port(state: State<'_, BackendPort>) -> Option<u16> {
  *state.0.lock().unwrap()
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
  if !is_allowed_external_url(&url) {
    return Err("URL scheme not allowed".to_string());
  }
  open::that(url).map_err(|e| e.to_string())?;
  Ok(())
}

/// Registers this app as the OS default handler for the `mailto:` scheme.
/// On macOS this calls LaunchServices directly (same mechanism the "Default
/// email reader" preference and other mail apps use). No-op error on other
/// platforms where the default is managed elsewhere (installer/desktop file).
#[cfg(target_os = "macos")]
fn set_default_mail_client_macos(bundle_id: &str) -> Result<(), String> {
  use core_foundation::base::TCFType;
  use core_foundation::string::{CFString, CFStringRef};

  extern "C" {
    fn LSSetDefaultHandlerForURLScheme(
      in_url_scheme: CFStringRef,
      in_handler_bundle_id: CFStringRef,
    ) -> i32;
    fn LSSetDefaultRoleHandlerForContentType(
      in_content_type: CFStringRef,
      in_role: u32,
      in_handler_bundle_id: CFStringRef,
    ) -> i32;
  }

  let bundle = CFString::new(bundle_id);

  let scheme = CFString::new("mailto");
  let status = unsafe {
    LSSetDefaultHandlerForURLScheme(scheme.as_concrete_TypeRef(), bundle.as_concrete_TypeRef())
  };
  if status != 0 {
    return Err(format!("LSSetDefaultHandlerForURLScheme failed with status {status}"));
  }

  // Also claim `.eml` files. macOS maps them to the `com.apple.mail.email`
  // content type; kLSRolesAll = 0xFFFFFFFF. Requires the bundle to declare this
  // UTI via LSItemContentTypes (see src-tauri/Info.plist).
  let eml_uti = CFString::new("com.apple.mail.email");
  let eml_status = unsafe {
    LSSetDefaultRoleHandlerForContentType(
      eml_uti.as_concrete_TypeRef(),
      0xFFFF_FFFF,
      bundle.as_concrete_TypeRef(),
    )
  };
  if eml_status != 0 {
    return Err(format!(
      "LSSetDefaultRoleHandlerForContentType failed with status {eml_status}"
    ));
  }

  Ok(())
}

#[cfg(target_os = "macos")]
fn is_default_mail_client_macos(bundle_id: &str) -> bool {
  use core_foundation::base::TCFType;
  use core_foundation::string::{CFString, CFStringRef};

  extern "C" {
    fn LSCopyDefaultHandlerForURLScheme(in_url_scheme: CFStringRef) -> CFStringRef;
  }

  let scheme = CFString::new("mailto");
  let handler_ref = unsafe { LSCopyDefaultHandlerForURLScheme(scheme.as_concrete_TypeRef()) };
  if handler_ref.is_null() {
    return false;
  }
  let handler = unsafe { CFString::wrap_under_create_rule(handler_ref) };
  handler.to_string().eq_ignore_ascii_case(bundle_id)
}

#[tauri::command]
fn set_as_default_mail_client(app: tauri::AppHandle) -> Result<(), String> {
  #[cfg(target_os = "macos")]
  {
    let id = app.config().identifier.clone();
    set_default_mail_client_macos(&id)
  }
  #[cfg(not(target_os = "macos"))]
  {
    let _ = app;
    Err("Setting the default mail client is only supported on macOS".to_string())
  }
}

#[tauri::command]
fn is_default_mail_client(app: tauri::AppHandle) -> bool {
  #[cfg(target_os = "macos")]
  {
    let id = app.config().identifier.clone();
    is_default_mail_client_macos(&id)
  }
  #[cfg(not(target_os = "macos"))]
  {
    let _ = app;
    false
  }
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
  let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
  clipboard.set_text(text).map_err(|e| e.to_string())?;
  Ok(())
}

/// Brings the main window back from the tray: unhides the app (macOS), then
/// unminimizes, shows and focuses the window.
fn show_main_window(app: &tauri::AppHandle) {
  // On macOS the window is hidden by hiding the whole application (see the
  // close-to-tray handler), so it must be unhidden before the window can show.
  #[cfg(target_os = "macos")]
  let _ = app.show();
  if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
  }
}

/// Updates the unread-mail indicator: the OS badge (macOS dock / Linux launcher)
/// and the tray tooltip. A count of 0 clears both.
#[tauri::command]
fn set_unread_badge(app: tauri::AppHandle, count: u32) -> Result<(), String> {
  if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
    let value = if count == 0 { None } else { Some(count as i64) };
    let _ = window.set_badge_count(value);
  }

  if let Some(tray) = app.tray_by_id(TRAY_ID) {
    let tooltip = if count == 0 {
      "Guvercin".to_string()
    } else {
      format!("Guvercin — {count} unread")
    };
    let _ = tray.set_tooltip(Some(tooltip));
  }

  Ok(())
}

fn sanitize_theme_name(input: &str) -> String {
  let mut out = String::new();
  for ch in input.trim().to_lowercase().chars() {
    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
      out.push(ch);
    } else if ch.is_whitespace() {
      out.push('-');
    }
  }
  while out.contains("--") {
    out = out.replace("--", "-");
  }
  out.trim_matches('-').to_string()
}

fn user_theme_dir(handle: &tauri::AppHandle) -> Result<PathBuf, String> {
  let base = handle
    .path()
    .app_data_dir()
    .map_err(|e| e.to_string())?;
  let dir = base.join("themes").join("user");
  fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
  Ok(dir)
}

fn validate_theme_json(raw: &str) -> Result<Value, String> {
  let mut value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
  let obj = value.as_object_mut().ok_or_else(|| "Theme JSON must be an object".to_string())?;

  let name = obj
    .get("name")
    .and_then(|v| v.as_str())
    .unwrap_or("")
    .trim();
  if name.is_empty() {
    return Err("Theme JSON missing name".to_string());
  }

  let vars = obj
    .get("vars")
    .and_then(|v| v.as_object())
    .ok_or_else(|| "Theme JSON missing vars".to_string())?;
  if vars.is_empty() {
    return Err("Theme JSON vars is empty".to_string());
  }

  for (k, v) in vars.iter() {
    if !k.starts_with("--") {
      return Err("Theme vars keys must start with --".to_string());
    }
    if !v.is_string() {
      return Err("Theme vars values must be strings".to_string());
    }
  }

  Ok(value)
}

#[tauri::command]
fn list_user_themes(handle: tauri::AppHandle) -> Result<Vec<String>, String> {
  let dir = user_theme_dir(&handle)?;
  let mut out: Vec<String> = vec![];
  for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
    let entry = entry.map_err(|e| e.to_string())?;
    let path = entry.path();
    if path.extension().and_then(|e| e.to_str()).unwrap_or("") != "json" {
      continue;
    }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
      if !stem.trim().is_empty() {
        out.push(stem.to_string());
      }
    }
  }
  out.sort();
  out.dedup();
  Ok(out)
}

#[tauri::command]
fn read_user_theme(handle: tauri::AppHandle, name: String) -> Result<String, String> {
  let safe = sanitize_theme_name(&name);
  if safe.is_empty() {
    return Err("Invalid theme name".to_string());
  }
  let dir = user_theme_dir(&handle)?;
  let path = dir.join(format!("{safe}.json"));
  fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn write_user_theme(handle: tauri::AppHandle, name: String, json: String) -> Result<(), String> {
  let safe = sanitize_theme_name(&name);
  if safe.is_empty() {
    return Err("Invalid theme name".to_string());
  }
  let mut value = validate_theme_json(&json)?;

  if let Some(obj) = value.as_object_mut() {
    obj.insert("name".to_string(), Value::String(safe.clone()));
  }

  let dir = user_theme_dir(&handle)?;
  let path = dir.join(format!("{safe}.json"));
  fs::write(path, serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?)
    .map_err(|e| e.to_string())?;
  Ok(())
}

#[tauri::command]
fn uninstall_app(handle: tauri::AppHandle) -> Result<(), String> {
  std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Delete app from Applications
    let _ = fs::remove_dir_all("/Applications/guvercin.app");

    // Delete all user data
    if let Ok(home) = std::env::var("HOME") {
      let _ = fs::remove_dir_all(format!("{}/Library/Application Support/Guvercin", home));
      let _ = fs::remove_dir_all(format!("{}/.config/guvercin", home));
      let _ = fs::remove_dir_all(format!("{}/.guvercin", home));
    }

    // Exit the app
    handle.exit(0);
  });

  Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  #[allow(unused_mut)]
  let mut builder = tauri::Builder::default();

  // Single-instance must be registered first so a second launch (e.g. the OS
  // spawning us to handle a `mailto:` link) is forwarded to the already-running
  // instance instead of opening a duplicate. Its `deep-link` feature hands the
  // URL to the deep-link plugin, which the frontend receives via `onOpenUrl`.
  // macOS delivers deep links to the running instance natively, so this is only
  // needed on Windows and Linux.
  #[cfg(any(target_os = "windows", target_os = "linux"))]
  {
    builder = builder.plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}));
  }

  builder
    .plugin(tauri_plugin_deep_link::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(
      tauri_plugin_log::Builder::new()
        .level(log::LevelFilter::Info)
        .targets([
          Target::new(TargetKind::Stdout),
          Target::new(TargetKind::LogDir { file_name: None }),
          Target::new(TargetKind::Webview),
        ])
        .build(),
    )
    .plugin(tauri_plugin_dialog::init())
    .setup(|app| {
      let _app_handle = app.handle().clone();

      // System tray: keeps the app reachable while its window is hidden in the
      // background (see the close-to-tray handler below). Left-clicking the icon
      // restores the window; the context menu offers quick actions.
      {
        let show_i = MenuItem::with_id(app, "show", "Show Guvercin", true, None::<&str>)?;
        let compose_i = MenuItem::with_id(app, "compose", "New Mail", true, None::<&str>)?;
        let settings_i = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
        let sep = PredefinedMenuItem::separator(app)?;
        let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show_i, &compose_i, &settings_i, &sep, &quit_i])?;

        let mut tray_builder = TrayIconBuilder::with_id(TRAY_ID)
          .tooltip("Guvercin")
          .menu(&menu)
          .show_menu_on_left_click(false)
          .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "compose" => {
              show_main_window(app);
              let _ = app.emit("tray://new-mail", ());
            }
            "settings" => {
              show_main_window(app);
              let _ = app.emit("tray://settings", ());
            }
            "quit" => app.exit(0),
            _ => {}
          })
          .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
              button: MouseButton::Left,
              button_state: MouseButtonState::Up,
              ..
            } = event
            {
              show_main_window(tray.app_handle());
            }
          });

        if let Some(icon) = app.default_window_icon().cloned() {
          tray_builder = tray_builder.icon(icon);
        }
        tray_builder.build(app)?;
      }

      // Close-to-tray: hitting the window's close button hides it instead of
      // quitting, so background mail sync and notifications keep running. A real
      // quit is available via the tray menu's "Quit" item.
      if let Some(main_window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let handle_for_close = app.handle().clone();
        main_window.on_window_event(move |event| {
          if let WindowEvent::CloseRequested { api, .. } = event {
            // On macOS, hide the whole application (like Cmd+H) rather than just
            // the window. This way clicking a notification or the dock icon
            // re-activates the app and macOS restores the window automatically,
            // which in turn fires the focus event that opens the notified mail.
            #[cfg(target_os = "macos")]
            {
              let _ = handle_for_close.hide();
            }
            #[cfg(not(target_os = "macos"))]
            {
              if let Some(win) = handle_for_close.get_webview_window(MAIN_WINDOW_LABEL) {
                let _ = win.hide();
              }
            }
            api.prevent_close();
          }
        });
      }

      // Register the configured URI schemes (mailto) with the OS at runtime.
      // Required for development and for Linux/Windows where the scheme isn't
      // otherwise installed; harmless on macOS.
      #[cfg(desktop)]
      {
        use tauri_plugin_deep_link::DeepLinkExt;
        if let Err(e) = app.deep_link().register_all() {
          log::warn!("Failed to register deep-link schemes: {}", e);
        }
      }

      let prefs_path = app
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("preferences.json"))
        .unwrap_or_else(|| PathBuf::from("preferences.json"));
      app.manage(PreferencesStore::load(prefs_path));
      
      // Get app data directory for database
      let db_dir = app.path().app_data_dir().ok().map(|path| {
        let db_path = path.join("databases");
        let _ = std::fs::create_dir_all(&db_path);
        db_path
      });

      // Spawn backend in a separate thread
      std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
          use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
          
          loop {
            match rust_backend::run(db_dir.clone()).await {
              Ok(port) => {
                // Store the port so the frontend can retrieve it.
                let state = _app_handle.state::<BackendPort>();
                *state.0.lock().unwrap() = Some(port);
                // Keep the runtime alive so the spawned axum server keeps running.
                std::future::pending::<()>().await;
                break;
              }
              Err(rust_backend::error::AppError::KeyringDenied(_)) => {
                log::warn!("Keyring access denied; prompting user to retry or quit");
                let confirmed = _app_handle.dialog()
                  .message("Access to the secure storage was denied. Guvercin needs this access to protect your account data.")
                  .title("Keyring Access Required")
                  .kind(MessageDialogKind::Warning)
                  .buttons(MessageDialogButtons::OkCancelCustom("Retry".to_string(), "Quit".to_string()))
                  .blocking_show();
                
                if confirmed {
                    // Retry selected (OkCustom)
                    continue;
                } else {
                    // Quit selected (CancelCustom)
                    _app_handle.exit(0);
                    break;
                }
              }
              Err(e) => {
                log::error!("Backend error: {}", e);
                _app_handle.dialog()
                  .message(format!("The backend failed to start: {}", e))
                  .title("Initialization Error")
                  .kind(MessageDialogKind::Error)
                  .blocking_show();
                _app_handle.exit(1);
                break;
              }
            }
          }
        });
      });

      #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
      ))]
      {
        for (_, window) in app.webview_windows() {
          disable_native_webview_context_menus(&window);
        }
      }

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      open_mail_window,
      get_mail_window_data,
      close_mail_window,
      open_compose_window,
      get_compose_window_data,
      close_compose_window,
      save_export_file_to_path,
      get_link_click_behavior,
      set_link_click_behavior,
      get_domain_link_behavior,
      set_domain_link_behavior,
      remove_domain_link_behavior,
      get_all_domain_link_behaviors,
      open_external_url,
      set_as_default_mail_client,
      is_default_mail_client,
      copy_to_clipboard,
      set_unread_badge,
      list_user_themes,
      read_user_theme,
      write_user_theme,
      get_backend_port,
      read_eml_file,
      uninstall_app
    ])
    .manage(MailWindowStore::default())
    .manage(ComposeWindowStore::default())
    .manage(BackendPort::default())
    .build(tauri::generate_context!())
    .expect("error while building tauri application")
    .run(|app_handle, event| {
      // macOS delivers a Reopen event when the user clicks the dock icon while
      // the app is already running. Because closing the window only hides it
      // (close-to-tray), the window won't come back on its own — so bring it
      // back explicitly here.
      #[cfg(target_os = "macos")]
      if let tauri::RunEvent::Reopen { .. } = event {
        show_main_window(app_handle);
      }
      #[cfg(not(target_os = "macos"))]
      {
        let _ = (app_handle, event);
      }
    });
}
