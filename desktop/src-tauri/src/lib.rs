mod codex;
mod config;
mod mcp_health;
mod sidecar;

use config::{config_path, parse_sidecar_profile, AppConfig, SidecarProfile};
use sidecar::SidecarOrchestrator;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{
    webview::{NewWindowFeatures, NewWindowResponse, PageLoadEvent, Url},
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

const MAIN_WINDOW_LABEL: &str = "main";
const SETTINGS_WINDOW_LABEL: &str = "settings";
static POPUP_WINDOW_COUNTER: AtomicUsize = AtomicUsize::new(1);

#[derive(serde::Serialize)]
struct ConfigResponse {
    penpot_public_uri: String,
    penpot_flags: String,
    sidecar_profile: String,
    mcp_stream_url: String,
    mcp_ws_url: String,
}

#[derive(serde::Serialize)]
struct SettingsResponse {
    penpot_public_uri: String,
    penpot_flags: String,
    sidecar_profile: String,
    auto_start_sidecar: bool,
    repo_root: Option<String>,
    effective_penpot_uri: String,
    mcp_stream_url: String,
    mcp_ws_url: String,
    config_path: Option<String>,
}

#[derive(serde::Deserialize)]
struct SaveSettingsPayload {
    penpot_public_uri: String,
    penpot_flags: String,
    sidecar_profile: String,
    auto_start_sidecar: bool,
    repo_root: Option<String>,
}

#[derive(serde::Serialize)]
struct CodexConfigResponse {
    stream_url: String,
    cli_override: String,
    toml_snippet: String,
}

fn settings_response(cfg: &AppConfig) -> SettingsResponse {
    let profile = cfg.sidecar_profile;
    SettingsResponse {
        penpot_public_uri: cfg.normalized_public_uri(),
        penpot_flags: cfg.penpot_flags.clone(),
        sidecar_profile: profile.as_str().to_string(),
        auto_start_sidecar: cfg.auto_start_sidecar,
        repo_root: cfg.repo_root.clone(),
        effective_penpot_uri: cfg.penpot_uri_for_profile(&profile),
        mcp_stream_url: cfg.mcp_stream_url(),
        mcp_ws_url: cfg.mcp_ws_url(),
        config_path: config_path().map(|p| p.display().to_string()),
    }
}

fn config_response(cfg: &AppConfig) -> ConfigResponse {
    ConfigResponse {
        penpot_public_uri: cfg.normalized_public_uri(),
        penpot_flags: cfg.penpot_flags.clone(),
        sidecar_profile: cfg.effective_sidecar_profile().as_str().to_string(),
        mcp_stream_url: cfg.mcp_stream_url(),
        mcp_ws_url: cfg.mcp_ws_url(),
    }
}

#[tauri::command]
fn get_config() -> ConfigResponse {
    config_response(&AppConfig::load())
}

#[tauri::command]
fn report_runtime_error(source: String, message: String) {
    eprintln!("[desktop][runtime][{source}] {message}");
}

#[tauri::command]
fn get_settings() -> SettingsResponse {
    settings_response(&AppConfig::load())
}

#[tauri::command]
fn save_settings(payload: SaveSettingsPayload) -> Result<SettingsResponse, String> {
    let profile = parse_sidecar_profile(&payload.sidecar_profile)?;
    let mut cfg = AppConfig::load();
    cfg.penpot_flags = payload.penpot_flags;
    cfg.sidecar_profile = profile;
    cfg.auto_start_sidecar = payload.auto_start_sidecar;
    cfg.repo_root = payload
        .repo_root
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    cfg.penpot_public_uri = match profile {
        SidecarProfile::Devenv | SidecarProfile::DockerLocal => {
            cfg.penpot_uri_for_profile(&profile)
        }
        SidecarProfile::Remote | SidecarProfile::ManagedMcp => {
            if payload.penpot_public_uri.trim().is_empty() {
                return Err("Penpot 服务器地址不能为空".to_string());
            }
            payload.penpot_public_uri
        }
    };

    cfg.save()?;
    let effective = restart_sidecar(&cfg)?;
    reload_main_window(&effective)?;
    Ok(settings_response(&effective))
}

#[tauri::command]
fn set_penpot_uri(uri: String) -> Result<ConfigResponse, String> {
    let mut cfg = AppConfig::load();
    cfg.penpot_public_uri = uri;
    cfg.sidecar_profile = SidecarProfile::Remote;
    cfg.save()?;
    reload_main_window(&cfg)?;
    Ok(config_response(&cfg))
}

#[tauri::command]
fn set_sidecar_profile(profile: String) -> Result<ConfigResponse, String> {
    let mapped = parse_sidecar_profile(&profile)?;

    let mut cfg = AppConfig::load();
    cfg.sidecar_profile = mapped;
    cfg.penpot_public_uri = cfg.penpot_uri_for_profile(&cfg.sidecar_profile);
    cfg.save()?;

    let effective = restart_sidecar(&cfg)?;
    reload_main_window(&effective)?;
    Ok(config_response(&effective))
}

#[tauri::command]
async fn start_sidecar() -> Result<sidecar::SidecarStatus, String> {
    let cfg = AppConfig::load();
    let orchestrator = sidecar_orchestrator()?;
    let status = orchestrator.ensure_started(&cfg).await;
    if !status.running {
        return Err(status
            .message
            .unwrap_or_else(|| "sidecar failed to start".to_string()));
    }
    Ok(status)
}

#[tauri::command]
fn stop_sidecar() -> sidecar::SidecarStatus {
    if let Some(orchestrator) = SIDECAR.get() {
        orchestrator.stop_managed();
    }
    sidecar_orchestrator()
        .map(|o| o.status())
        .unwrap_or_else(|_| sidecar::SidecarStatus {
            profile: "unknown".to_string(),
            penpot_public_uri: AppConfig::default().normalized_public_uri(),
            repo_root: None,
            running: false,
            services: vec![],
            message: Some("Sidecar not initialized".to_string()),
        })
}

#[tauri::command]
fn get_sidecar_status() -> sidecar::SidecarStatus {
    sidecar_orchestrator()
        .map(|o| o.status())
        .unwrap_or_else(|_| sidecar::SidecarStatus {
            profile: AppConfig::load()
                .effective_sidecar_profile()
                .as_str()
                .to_string(),
            penpot_public_uri: AppConfig::load().normalized_public_uri(),
            repo_root: sidecar::resolve_repo_root().map(|p| p.display().to_string()),
            running: false,
            services: vec![],
            message: Some("Sidecar not initialized".to_string()),
        })
}

#[tauri::command]
fn generate_codex_config(user_token: Option<String>) -> CodexConfigResponse {
    let cfg = AppConfig::load();
    let snippet = codex::build_codex_snippet(&cfg, user_token.as_deref());
    CodexConfigResponse {
        stream_url: snippet.stream_url,
        cli_override: snippet.cli_override,
        toml_snippet: snippet.toml_snippet,
    }
}

#[tauri::command]
async fn check_mcp_health() -> mcp_health::McpHealthReport {
    let cfg = AppConfig::load();
    mcp_health::check_mcp_health(&cfg).await
}

#[tauri::command]
fn reload_penpot() -> Result<(), String> {
    reload_main_window(&AppConfig::load())
}

#[tauri::command]
fn close_settings_window() -> Result<(), String> {
    let app = APP_HANDLE
        .get()
        .ok_or_else(|| "application not ready".to_string())?;
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn sidecar_orchestrator() -> Result<&'static SidecarOrchestrator, String> {
    SIDECAR
        .get()
        .ok_or_else(|| "sidecar orchestrator not initialized".to_string())
}

fn restart_sidecar(cfg: &AppConfig) -> Result<AppConfig, String> {
    let orchestrator = sidecar_orchestrator()?;
    let status = tauri::async_runtime::block_on(orchestrator.ensure_started(cfg));
    let mut effective = cfg.clone();
    effective.penpot_public_uri = status.penpot_public_uri;
    Ok(effective)
}

fn reload_main_window(cfg: &AppConfig) -> Result<(), String> {
    let app = APP_HANDLE
        .get()
        .ok_or_else(|| "application not ready".to_string())?;
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.close();
    }
    open_main_window(app, cfg)
}

fn open_settings_window(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        window
            .set_focus()
            .map_err(|e| format!("failed to focus settings window: {e}"))?;
        return Ok(());
    }

    WebviewWindowBuilder::new(
        app,
        SETTINGS_WINDOW_LABEL,
        WebviewUrl::App("settings.html".into()),
    )
    .title("Penpot 设置")
    .inner_size(520.0, 720.0)
    .min_inner_size(420.0, 560.0)
    .resizable(true)
    .center()
    .build()
    .map_err(|e| e.to_string())?;

    Ok(())
}

fn is_same_penpot_origin(base: &Url, candidate: &Url) -> bool {
    base.scheme() == candidate.scheme()
        && base.host_str() == candidate.host_str()
        && base.port_or_known_default() == candidate.port_or_known_default()
}

fn build_penpot_window(
    app: &AppHandle,
    label: &str,
    cfg: &AppConfig,
    url: Url,
    features: Option<NewWindowFeatures>,
) -> Result<WebviewWindow, String> {
    let script = cfg.initialization_script();
    let base_uri: Url = cfg
        .normalized_public_uri()
        .parse()
        .map_err(|e| format!("invalid penpot uri: {e}"))?;
    let popup_cfg = cfg.clone();
    let popup_base = base_uri.clone();
    let popup_app = app.clone();

    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("Penpot")
        .inner_size(1440.0, 900.0)
        .min_inner_size(1024.0, 640.0)
        .initialization_script(script)
        .on_document_title_changed(|window, title| {
            let _ = window.set_title(&title);
        })
        .on_new_window(move |url, features| {
            if cfg!(debug_assertions) {
                println!("[desktop] new window request {}", url);
            }

            if is_same_penpot_origin(&popup_base, &url) {
                let label = format!(
                    "penpot-popup-{}",
                    POPUP_WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed)
                );

                match build_penpot_window(
                    &popup_app,
                    &label,
                    &popup_cfg,
                    url.clone(),
                    Some(features),
                ) {
                    Ok(window) => NewWindowResponse::Create { window },
                    Err(err) => {
                        eprintln!("[desktop] failed to create popup window: {err}");
                        let _ = popup_app.opener().open_url(url.as_str(), None::<&str>);
                        NewWindowResponse::Deny
                    }
                }
            } else {
                let _ = popup_app.opener().open_url(url.as_str(), None::<&str>);
                NewWindowResponse::Deny
            }
        })
        .on_navigation(|url| {
            println!("[desktop] navigating to {}", url);
            true
        })
        .on_page_load(|window, payload| {
            println!(
                "[desktop] page load {:?} {}",
                payload.event(),
                payload.url()
            );

            if cfg!(debug_assertions) && payload.event() == PageLoadEvent::Finished {
                window.open_devtools();
                let _ = window.eval(
                    r#"
if (!window.__penpotDesktopDebugHooksInstalled) {
  window.__penpotDesktopDebugHooksInstalled = true;
  const reportToHost = async (source, parts) => {
    try {
      const text = parts
        .map((part) => {
          if (part instanceof Error) return part.stack || part.message;
          if (typeof part === "string") return part;
          try {
            return JSON.stringify(part);
          } catch (_) {
            return String(part);
          }
        })
        .join(" | ");
      if (window.__TAURI__?.core?.invoke) {
        await window.__TAURI__.core.invoke("report_runtime_error", {
          source,
          message: text,
        });
      }
    } catch (_) {}
  };

  console.log("[desktop] debug hooks installed", location.href, navigator.userAgent);
  window.addEventListener("error", (event) => {
    void reportToHost("window.error", [
      event.message,
      event.filename,
      event.lineno,
      event.colno,
      event.error,
    ]);
    console.error(
      "[desktop][window.error]",
      event.message,
      event.filename,
      event.lineno,
      event.colno,
      event.error
    );
  });
  window.addEventListener("unhandledrejection", (event) => {
    void reportToHost("unhandledrejection", [event.reason]);
    console.error("[desktop][unhandledrejection]", event.reason);
  });
  const originalConsoleError = console.error.bind(console);
  console.error = (...args) => {
    void reportToHost("console.error", args);
    originalConsoleError(...args);
  };
  window.setTimeout(() => {
    const appRoot = document.getElementById("app");
    const snapshot = {
      href: location.href,
      title: document.title,
      readyState: document.readyState,
      bodyChildren: document.body?.children?.length ?? null,
      appChildren: appRoot?.children?.length ?? null,
      appText: (appRoot?.innerText || "").slice(0, 400),
      bodyText: (document.body?.innerText || "").slice(0, 400),
    };
    void reportToHost("dom.snapshot", [snapshot]);
  }, 3000);
}
"#,
                );

                let snapshot_window = window.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let login_window = snapshot_window.clone();
                    let _ = snapshot_window.eval_with_callback(
                        r#"
(() => {
  const appRoot = document.getElementById("app");
  const bodyStyle = document.body ? getComputedStyle(document.body) : null;
  const appStyle = appRoot ? getComputedStyle(appRoot) : null;
  return {
    href: location.href,
    title: document.title,
    readyState: document.readyState,
    bodyChildren: document.body?.children?.length ?? null,
    appChildren: appRoot?.children?.length ?? null,
    bodyText: (document.body?.innerText || "").slice(0, 400),
    appText: (appRoot?.innerText || "").slice(0, 400),
    bodyBg: bodyStyle?.backgroundColor ?? null,
    bodyColor: bodyStyle?.color ?? null,
    appDisplay: appStyle?.display ?? null,
    appVisibility: appStyle?.visibility ?? null,
    appOpacity: appStyle?.opacity ?? null,
  };
})()
"#,
                        move |value| {
                            println!("[desktop][dom.snapshot] {value}");

                            let parsed = serde_json::from_str::<serde_json::Value>(&value).ok();
                            let app_text = parsed
                                .as_ref()
                                .and_then(|json| json.get("appText"))
                                .and_then(|text| text.as_str())
                                .unwrap_or_default();

                            let auth_expired = app_text.contains("session expired")
                                || app_text.contains("not authenticated")
                                || app_text.contains("还没有登录")
                                || app_text.contains("会话已过期");

                            if auth_expired {
                                println!(
                                    "[desktop] detected unauthenticated state, redirecting to login route"
                                );
                                let _ = login_window.eval(
                                    r#"window.location.replace(`${window.location.origin}/#/auth/login`);"#,
                                );
                            }
                        },
                    );
                });
            }
        });

    let builder = if let Some(features) = features {
        builder.window_features(features)
    } else {
        builder
    };

    builder.build().map_err(|e| e.to_string())
}

fn open_main_window(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    let url = cfg
        .normalized_public_uri()
        .parse()
        .map_err(|e| format!("invalid penpot uri: {e}"))?;

    build_penpot_window(app, MAIN_WINDOW_LABEL, cfg, url, None)?;

    Ok(())
}

fn build_menu(app: &AppHandle) -> Result<Menu<tauri::Wry>, tauri::Error> {
    let reload = MenuItem::with_id(app, "reload", "Reload", true, None::<&str>)?;
    let open_browser =
        MenuItem::with_id(app, "open_browser", "Open in Browser", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let copy_codex = MenuItem::with_id(
        app,
        "copy_codex",
        "Copy Codex MCP Config",
        true,
        None::<&str>,
    )?;
    let check_mcp = MenuItem::with_id(app, "check_mcp", "Check MCP Health", true, None::<&str>)?;
    let sidecar_status =
        MenuItem::with_id(app, "sidecar_status", "Sidecar Status", true, None::<&str>)?;
    let sidecar_start =
        MenuItem::with_id(app, "sidecar_start", "Start Sidecar", true, None::<&str>)?;
    let sidecar_stop = MenuItem::with_id(app, "sidecar_stop", "Stop Sidecar", true, None::<&str>)?;

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &reload,
            &PredefinedMenuItem::separator(app)?,
            &open_browser,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let ai_menu = Submenu::with_items(app, "AI", true, &[&copy_codex, &check_mcp])?;
    let sidecar_menu = Submenu::with_items(
        app,
        "Sidecar",
        true,
        &[&sidecar_status, &sidecar_start, &sidecar_stop],
    )?;

    Menu::with_items(
        app,
        &[
            &file_menu,
            &sidecar_menu,
            &ai_menu,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )
}

fn show_sidecar_status_dialog(app: &AppHandle) {
    let status = get_sidecar_status();
    let services = status
        .services
        .iter()
        .map(|svc| {
            format!(
                "- {} [{}] {}",
                svc.name,
                svc.state,
                svc.endpoint.clone().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let msg = format!(
        "profile: {}\nuri: {}\nrunning: {}\nrepo: {}\n\n{}\n\n{}",
        status.profile,
        status.penpot_public_uri,
        status.running,
        status
            .repo_root
            .clone()
            .unwrap_or_else(|| "(none)".to_string()),
        status.message.clone().unwrap_or_default(),
        services
    );

    app.dialog()
        .message(msg)
        .title("Sidecar Status")
        .show(|_| {});
}

static APP_HANDLE: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
static SIDECAR: std::sync::OnceLock<SidecarOrchestrator> = std::sync::OnceLock::new();

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let _ = APP_HANDLE.set(app.handle().clone());

            let cfg = AppConfig::load();
            let orchestrator = SidecarOrchestrator::from_config(&cfg);
            let sidecar_status = tauri::async_runtime::block_on(orchestrator.ensure_started(&cfg));
            let _ = SIDECAR.set(orchestrator);

            let mut effective_cfg = cfg;
            effective_cfg.penpot_public_uri = sidecar_status.penpot_public_uri;

            app.set_menu(build_menu(app.handle())?)?;
            open_main_window(app.handle(), &effective_cfg)?;

            if let Some(message) = sidecar_status.message {
                if !sidecar_status.running {
                    let handle = app.handle().clone();
                    handle
                        .dialog()
                        .message(message)
                        .title("Sidecar")
                        .show(|_| {});
                }
            }

            Ok(())
        })
        .on_menu_event(|app, event| {
            let cfg = AppConfig::load();
            match event.id().as_ref() {
                "reload" => {
                    let _ = reload_main_window(&cfg);
                }
                "open_browser" => {
                    let _ = app
                        .opener()
                        .open_url(cfg.normalized_public_uri(), None::<&str>);
                }
                "settings" => {
                    let _ = open_settings_window(app);
                }
                "copy_codex" => {
                    let snippet = codex::build_codex_snippet(&cfg, None);
                    let _ = app.clipboard().write_text(snippet.toml_snippet);
                }
                "check_mcp" => {
                    let app_handle = app.clone();
                    let cfg = AppConfig::load();
                    tauri::async_runtime::spawn(async move {
                        let report = mcp_health::check_mcp_health(&cfg).await;
                        let msg = if report.stream_reachable {
                            format!(
                                "MCP stream reachable (HTTP {})\n{}",
                                report.stream_status.unwrap_or(0),
                                report.mcp_stream_url
                            )
                        } else {
                            format!(
                                "MCP stream not reachable\n{}\n{}",
                                report.mcp_stream_url,
                                report.error.unwrap_or_default()
                            )
                        };
                        app_handle
                            .dialog()
                            .message(msg)
                            .title("MCP Health")
                            .show(|_| {});
                    });
                }
                "sidecar_status" => show_sidecar_status_dialog(app),
                "sidecar_start" => {
                    let app_handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        match start_sidecar().await {
                            Ok(status) => {
                                let mut effective = AppConfig::load();
                                effective.penpot_public_uri = status.penpot_public_uri;
                                let _ = reload_main_window(&effective);
                                show_sidecar_status_dialog(&app_handle);
                            }
                            Err(err) => {
                                app_handle
                                    .dialog()
                                    .message(err)
                                    .title("Sidecar Start Failed")
                                    .show(|_| {});
                            }
                        }
                    });
                }
                "sidecar_stop" => {
                    let _ = stop_sidecar();
                    show_sidecar_status_dialog(app);
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_settings,
            save_settings,
            set_penpot_uri,
            set_sidecar_profile,
            get_sidecar_status,
            start_sidecar,
            stop_sidecar,
            generate_codex_config,
            check_mcp_health,
            reload_penpot,
            report_runtime_error,
            close_settings_window
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let RunEvent::Exit = event {
                if let Some(orchestrator) = SIDECAR.get() {
                    orchestrator.stop_managed();
                }
            }
        });
}
