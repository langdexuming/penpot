use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_PENPOT_URI: &str = "https://design.penpot.app";
pub const DEFAULT_DEV_URI: &str = "https://localhost:3449";

pub const DEFAULT_FLAGS: &str = "enable-feature-render-wasm plugins/runtime";
pub const DEFAULT_DOCKER_URI: &str = "http://localhost:9001";

pub const LOCAL_MCP_STREAM_URL: &str = "http://127.0.0.1:4401/mcp";
pub const LOCAL_MCP_WS_URL: &str = "ws://127.0.0.1:4402/";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidecarProfile {
    #[default]
    Remote,
    Devenv,
    DockerLocal,
    ManagedMcp,
}

impl SidecarProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Devenv => "devenv",
            Self::DockerLocal => "docker_local",
            Self::ManagedMcp => "managed_mcp",
        }
    }

    pub fn from_env() -> Option<Self> {
        let value = std::env::var("PENPOT_DESKTOP_SIDECAR").ok()?;
        match value.as_str() {
            "remote" => Some(Self::Remote),
            "devenv" => Some(Self::Devenv),
            "docker" | "docker_local" => Some(Self::DockerLocal),
            "mcp" | "managed_mcp" => Some(Self::ManagedMcp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub penpot_public_uri: String,
    pub penpot_flags: String,
    #[serde(default)]
    pub sidecar_profile: SidecarProfile,
    #[serde(default = "default_auto_start_sidecar")]
    pub auto_start_sidecar: bool,
    #[serde(default)]
    pub repo_root: Option<String>,
}

fn default_auto_start_sidecar() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            penpot_public_uri: default_penpot_uri(),
            penpot_flags: DEFAULT_FLAGS.to_string(),
            sidecar_profile: SidecarProfile::Remote,
            auto_start_sidecar: true,
            repo_root: None,
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        if let Ok(uri) = std::env::var("PENPOT_DESKTOP_URI") {
            return Self {
                penpot_public_uri: normalize_uri(&uri),
                ..Default::default()
            };
        }

        config_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or_else(|| "config directory unavailable".to_string())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let raw = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, raw).map_err(|e| e.to_string())
    }

    pub fn normalized_public_uri(&self) -> String {
        normalize_uri(&self.penpot_public_uri)
    }

    pub fn initialization_script(&self) -> String {
        let uri = self.normalized_public_uri();
        let flags = &self.penpot_flags;
        let mcp_ws = self.mcp_plugin_ws_uri();
        let mcp_ws_line = mcp_ws
            .as_ref()
            .map(|ws| format!("window.penpotMcpServerURI = {ws:?};\n"))
            .unwrap_or_default();

        format!(
            r#"
window.penpotPublicURI = {uri:?};
window.penpotFlags = {flags:?};
{mcp_ws_line}window.externalContextInfo = function() {{
  return {{ platform: "desktop", shell: "tauri", penpotUri: {uri:?} }};
}};
"#
        )
    }

    pub fn mcp_stream_url(&self) -> String {
        if self.effective_sidecar_profile() == SidecarProfile::ManagedMcp {
            LOCAL_MCP_STREAM_URL.to_string()
        } else {
            join_uri_path(&self.normalized_public_uri(), "mcp/stream")
        }
    }

    /// Direct WebSocket URL for the Penpot MCP plugin (bypasses reverse proxy).
    pub fn mcp_plugin_ws_uri(&self) -> Option<String> {
        if self.effective_sidecar_profile() == SidecarProfile::ManagedMcp {
            Some(LOCAL_MCP_WS_URL.to_string())
        } else {
            None
        }
    }

    pub fn effective_sidecar_profile(&self) -> SidecarProfile {
        SidecarProfile::from_env().unwrap_or_else(|| {
            if std::env::var("PENPOT_DESKTOP_DEV").is_ok() {
                SidecarProfile::Devenv
            } else {
                self.sidecar_profile
            }
        })
    }

    pub fn penpot_uri_for_profile(&self, profile: &SidecarProfile) -> String {
        match profile {
            SidecarProfile::Devenv => normalize_uri(DEFAULT_DEV_URI),
            SidecarProfile::DockerLocal => normalize_uri(DEFAULT_DOCKER_URI),
            SidecarProfile::Remote | SidecarProfile::ManagedMcp => self.normalized_public_uri(),
        }
    }

    pub fn mcp_ws_url(&self) -> String {
        if self.effective_sidecar_profile() == SidecarProfile::ManagedMcp {
            LOCAL_MCP_WS_URL.to_string()
        } else {
            let base = self.normalized_public_uri();
            let ws_base = if base.starts_with("https://") {
                base.replacen("https://", "wss://", 1)
            } else {
                base.replacen("http://", "ws://", 1)
            };
            join_uri_path(&ws_base, "mcp/ws")
        }
    }
}

fn join_uri_path(base: &str, path: &str) -> String {
    format!("{base_trim}/{path}", base_trim = base.trim_end_matches('/'))
}

fn default_penpot_uri() -> String {
    if std::env::var("PENPOT_DESKTOP_DEV").is_ok() {
        DEFAULT_DEV_URI.to_string()
    } else {
        DEFAULT_PENPOT_URI.to_string()
    }
}

fn normalize_uri(uri: &str) -> String {
    let trimmed = uri.trim();
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    if with_scheme.ends_with('/') {
        with_scheme
    } else {
        format!("{with_scheme}/")
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("penpot-desktop").join("config.json"))
}

pub fn parse_sidecar_profile(value: &str) -> Result<SidecarProfile, String> {
    match value {
        "remote" => Ok(SidecarProfile::Remote),
        "devenv" => Ok(SidecarProfile::Devenv),
        "docker_local" | "docker" => Ok(SidecarProfile::DockerLocal),
        "managed_mcp" | "mcp" => Ok(SidecarProfile::ManagedMcp),
        _ => Err(format!("unknown sidecar profile: {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_uri_trailing_slash() {
        assert_eq!(
            normalize_uri("https://design.penpot.app"),
            "https://design.penpot.app/"
        );
    }

    #[test]
    fn builds_mcp_urls() {
        let cfg = AppConfig {
            penpot_public_uri: "https://penpot.example.com/".to_string(),
            penpot_flags: DEFAULT_FLAGS.to_string(),
            sidecar_profile: SidecarProfile::Remote,
            auto_start_sidecar: true,
            repo_root: None,
        };
        assert_eq!(
            cfg.mcp_stream_url(),
            "https://penpot.example.com/mcp/stream"
        );
        assert_eq!(cfg.mcp_ws_url(), "wss://penpot.example.com/mcp/ws");
        assert!(cfg.mcp_plugin_ws_uri().is_none());
    }

    #[test]
    fn managed_mcp_uses_local_endpoints() {
        let cfg = AppConfig {
            penpot_public_uri: "https://penpot.example.com/".to_string(),
            penpot_flags: DEFAULT_FLAGS.to_string(),
            sidecar_profile: SidecarProfile::ManagedMcp,
            auto_start_sidecar: true,
            repo_root: None,
        };
        assert_eq!(cfg.mcp_stream_url(), LOCAL_MCP_STREAM_URL);
        assert_eq!(cfg.mcp_ws_url(), LOCAL_MCP_WS_URL);
        assert_eq!(cfg.mcp_plugin_ws_uri().as_deref(), Some(LOCAL_MCP_WS_URL));
        assert!(cfg.initialization_script().contains("penpotMcpServerURI"));
    }
}
