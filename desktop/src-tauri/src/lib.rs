mod codex;
mod config;
mod mcp_health;
mod sidecar;

use config::{config_path, parse_sidecar_profile, AppConfig, SidecarProfile};
use sidecar::SidecarOrchestrator;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

const MAIN_WINDOW_LABEL: &str = "main";
const SETTINGS_WINDOW_LABEL: &str = "settings";

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
        return Err(status.message.unwrap_or_else(|| "sidecar failed to start".to_string()));
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
            profile: AppConfig::load().effective_sidecar_profile().as_str().to_string(),
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

fn open_main_window(app: &AppHandle, cfg: &AppConfig) -> Result<(), String> {
    let url = cfg
        .normalized_public_uri()
        .parse()
        .map_err(|e| format!("invalid penpot uri: {e}"))?;
    let script = cfg.initialization_script();

    WebviewWindowBuilder::new(app, MAIN_WINDOW_LABEL, WebviewUrl::External(url))
        .title("Penpot")
        .inner_size(1440.0, 900.0)
        .min_inner_size(1024.0, 640.0)
        .initialization_script(script)
        .build()
        .map_err(|e| e.to_string())?;

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
    let sidecar_status = MenuItem::with_id(
        app,
        "sidecar_status",
        "Sidecar Status",
        true,
        None::<&str>,
    )?;
    let sidecar_start = MenuItem::with_id(
        app,
        "sidecar_start",
        "Start Sidecar",
        true,
        None::<&str>,
    )?;
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
        status.repo_root.clone().unwrap_or_else(|| "(none)".to_string()),
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
