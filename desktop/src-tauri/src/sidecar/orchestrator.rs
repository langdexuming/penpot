use super::health::HealthClient;
use super::repo::resolve_repo_root;
use crate::config::{AppConfig, SidecarProfile};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceStatus {
    pub name: String,
    pub state: String,
    pub endpoint: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SidecarStatus {
    pub profile: String,
    pub penpot_public_uri: String,
    pub repo_root: Option<String>,
    pub running: bool,
    pub services: Vec<ServiceStatus>,
    pub message: Option<String>,
}

struct SidecarOutcome {
    status: SidecarStatus,
    spawned_devenv: bool,
    started_docker: bool,
    mcp_child: Option<Child>,
}

pub struct SidecarOrchestrator {
    inner: Mutex<SidecarState>,
}

struct SidecarState {
    profile: SidecarProfile,
    repo_root: Option<PathBuf>,
    started_docker: bool,
    spawned_devenv: bool,
    mcp_child: Option<Child>,
    last_status: SidecarStatus,
}

impl SidecarOrchestrator {
    pub fn from_config(cfg: &AppConfig) -> Self {
        let profile = cfg.effective_sidecar_profile();
        let repo_root = cfg.repo_root.clone().map(PathBuf::from).or_else(resolve_repo_root);
        let penpot_uri = cfg.penpot_uri_for_profile(&profile);

        Self {
            inner: Mutex::new(SidecarState {
                profile,
                repo_root: repo_root.clone(),
                started_docker: false,
                spawned_devenv: false,
                mcp_child: None,
                last_status: SidecarStatus {
                    profile: profile.as_str().to_string(),
                    penpot_public_uri: penpot_uri,
                    repo_root: repo_root.map(|p| p.display().to_string()),
                    running: false,
                    services: vec![],
                    message: Some("Sidecar not started".to_string()),
                },
            }),
        }
    }

    pub fn status(&self) -> SidecarStatus {
        self.inner
            .lock()
            .map(|state| state.last_status.clone())
            .unwrap_or_else(|_| SidecarStatus {
                profile: "unknown".to_string(),
                penpot_public_uri: AppConfig::default().normalized_public_uri(),
                repo_root: None,
                running: false,
                services: vec![],
                message: Some("Sidecar state lock poisoned".to_string()),
            })
    }

    pub async fn ensure_started(&self, cfg: &AppConfig) -> SidecarStatus {
        let profile = cfg.effective_sidecar_profile();
        let penpot_uri = cfg.penpot_uri_for_profile(&profile);
        let repo_root = cfg
            .repo_root
            .clone()
            .map(PathBuf::from)
            .or_else(resolve_repo_root);
        let outcome = match run_profile(profile, &penpot_uri, repo_root.clone(), cfg).await {
            Ok(outcome) => outcome,
            Err(status) => SidecarOutcome {
                status,
                spawned_devenv: false,
                started_docker: false,
                mcp_child: None,
            },
        };

        if let Ok(mut state) = self.inner.lock() {
            state.profile = profile;
            state.repo_root = repo_root;
            if outcome.spawned_devenv {
                state.spawned_devenv = true;
            }
            if outcome.started_docker {
                state.started_docker = true;
            }
            if let Some(child) = outcome.mcp_child {
                if let Some(mut old) = state.mcp_child.take() {
                    let _ = old.kill();
                }
                state.mcp_child = Some(child);
            }
            state.last_status = outcome.status;
        }

        self.status()
    }

    pub fn stop_managed(&self) {
        let mut state = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(mut child) = state.mcp_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        if state.started_docker {
            if let Some(repo) = state.repo_root.as_ref() {
                let _ = Command::new("docker")
                    .args([
                        "compose",
                        "-f",
                        "docker/images/docker-compose.yaml",
                        "down",
                    ])
                    .current_dir(repo)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            state.started_docker = false;
        }

        state.last_status.message = Some("Sidecar stopped".to_string());
        state.last_status.running = false;
    }
}

async fn run_profile(
    profile: SidecarProfile,
    penpot_uri: &str,
    repo_root: Option<PathBuf>,
    cfg: &AppConfig,
) -> Result<SidecarOutcome, SidecarStatus> {
    let health = HealthClient::new().map_err(|err| {
        failed_status(
            profile,
            penpot_uri,
            &repo_root,
            format!("HTTP client error: {err}"),
        )
    })?;

    let mut services = Vec::new();

    match profile {
        SidecarProfile::Remote => {
            let probe = health.probe_url(penpot_uri).await;
            services.push(service_from_probe("penpot", penpot_uri, &probe));
            Ok(SidecarOutcome {
                status: SidecarStatus {
                    profile: profile.as_str().to_string(),
                    penpot_public_uri: penpot_uri.to_string(),
                    repo_root: repo_root.as_ref().map(|p| p.display().to_string()),
                    running: probe.reachable,
                    services,
                    message: None,
                },
                spawned_devenv: false,
                started_docker: false,
                mcp_child: None,
            })
        }
        SidecarProfile::Devenv => {
            let mut spawned_devenv = false;
            if !health.probe_url(penpot_uri).await.reachable {
                if cfg.auto_start_sidecar {
                    spawned_devenv = spawn_devenv(&repo_root).map_err(|err| {
                        failed_status(profile, penpot_uri, &repo_root, err)
                    })?;
                    let ready = health
                        .wait_for_reachable(
                            penpot_uri,
                            std::time::Duration::from_secs(180),
                            std::time::Duration::from_secs(3),
                        )
                        .await;
                    if !ready {
                        return Err(failed_status(
                            profile,
                            penpot_uri,
                            &repo_root,
                            "Timed out waiting for devenv at https://localhost:3449".to_string(),
                        ));
                    }
                } else {
                    return Err(failed_status(
                        profile,
                        penpot_uri,
                        &repo_root,
                        "Devenv is not reachable. Start ./manage.sh run-devenv-agentic or enable auto_start_sidecar.".to_string(),
                    ));
                }
            }

            let probe = health.probe_url(penpot_uri).await;
            services.push(service_from_probe("penpot-devenv", penpot_uri, &probe));
            let mcp_url = cfg.mcp_stream_url();
            let mcp_probe = health.probe_url(&mcp_url).await;
            services.push(service_from_probe("mcp-stream", &mcp_url, &mcp_probe));

            Ok(SidecarOutcome {
                status: SidecarStatus {
                    profile: profile.as_str().to_string(),
                    penpot_public_uri: penpot_uri.to_string(),
                    repo_root: repo_root.as_ref().map(|p| p.display().to_string()),
                    running: probe.reachable,
                    services,
                    message: if spawned_devenv {
                        Some("Started devenv via manage.sh run-devenv-agentic".to_string())
                    } else {
                        None
                    },
                },
                spawned_devenv,
                started_docker: false,
                mcp_child: None,
            })
        }
        SidecarProfile::DockerLocal => {
            let mut started_docker = false;
            if !health.probe_url(penpot_uri).await.reachable {
                if cfg.auto_start_sidecar {
                    spawn_docker_stack(&repo_root).map_err(|err| {
                        failed_status(profile, penpot_uri, &repo_root, err)
                    })?;
                    started_docker = true;
                    let ready = health
                        .wait_for_reachable(
                            penpot_uri,
                            std::time::Duration::from_secs(300),
                            std::time::Duration::from_secs(5),
                        )
                        .await;
                    if !ready {
                        return Err(failed_status(
                            profile,
                            penpot_uri,
                            &repo_root,
                            "Timed out waiting for docker stack at http://localhost:9001".to_string(),
                        ));
                    }
                } else {
                    return Err(failed_status(
                        profile,
                        penpot_uri,
                        &repo_root,
                        "Docker Penpot stack is not reachable on http://localhost:9001".to_string(),
                    ));
                }
            }

            let probe = health.probe_url(penpot_uri).await;
            services.push(service_from_probe("penpot-docker", penpot_uri, &probe));

            Ok(SidecarOutcome {
                status: SidecarStatus {
                    profile: profile.as_str().to_string(),
                    penpot_public_uri: penpot_uri.to_string(),
                    repo_root: repo_root.as_ref().map(|p| p.display().to_string()),
                    running: probe.reachable,
                    services,
                    message: if started_docker {
                        Some("Started docker/images/docker-compose.yaml".to_string())
                    } else {
                        None
                    },
                },
                spawned_devenv: false,
                started_docker,
                mcp_child: None,
            })
        }
        SidecarProfile::ManagedMcp => {
            let penpot_probe = health.probe_url(penpot_uri).await;
            services.push(service_from_probe("penpot", penpot_uri, &penpot_probe));

            let mcp_url = cfg.mcp_stream_url();
            let mut mcp_child = None;
            if !health.probe_url(&mcp_url).await.reachable && cfg.auto_start_sidecar {
                mcp_child = Some(spawn_mcp_server(&repo_root).map_err(|err| {
                    failed_status(profile, penpot_uri, &repo_root, err)
                })?);
                let _ = health
                    .wait_for_reachable(
                        &mcp_url,
                        std::time::Duration::from_secs(60),
                        std::time::Duration::from_secs(2),
                    )
                    .await;
            }

            let mcp_probe = health.probe_url(&mcp_url).await;
            services.push(service_from_probe("mcp-stream", &mcp_url, &mcp_probe));

            Ok(SidecarOutcome {
                status: SidecarStatus {
                    profile: profile.as_str().to_string(),
                    penpot_public_uri: penpot_uri.to_string(),
                    repo_root: repo_root.as_ref().map(|p| p.display().to_string()),
                    running: penpot_probe.reachable && mcp_probe.reachable,
                    services,
                    message: if mcp_child.is_some() {
                        Some("Started local MCP server from monorepo".to_string())
                    } else {
                        None
                    },
                },
                spawned_devenv: false,
                started_docker: false,
                mcp_child,
            })
        }
    }
}

fn service_from_probe(name: &str, endpoint: &str, probe: &super::health::ProbeResult) -> ServiceStatus {
    ServiceStatus {
        name: name.to_string(),
        state: if probe.reachable {
            "running".to_string()
        } else {
            "unhealthy".to_string()
        },
        endpoint: Some(endpoint.to_string()),
        detail: probe
            .error
            .clone()
            .or_else(|| probe.status.map(|s| format!("HTTP {s}"))),
    }
}

fn failed_status(
    profile: SidecarProfile,
    penpot_uri: &str,
    repo_root: &Option<PathBuf>,
    message: String,
) -> SidecarStatus {
    SidecarStatus {
        profile: profile.as_str().to_string(),
        penpot_public_uri: penpot_uri.to_string(),
        repo_root: repo_root.as_ref().map(|p| p.display().to_string()),
        running: false,
        services: vec![],
        message: Some(message),
    }
}

fn spawn_devenv(repo_root: &Option<PathBuf>) -> Result<bool, String> {
    let repo = repo_root
        .as_ref()
        .ok_or_else(|| "Penpot repo not found. Set PENPOT_REPO_ROOT.".to_string())?;
    let script = repo.join("manage.sh");
    if !script.exists() {
        return Err(format!("manage.sh not found at {}", script.display()));
    }

    let output = Command::new("bash")
        .arg(script)
        .arg("run-devenv-agentic")
        .current_dir(repo)
        .output()
        .map_err(|e| format!("failed to run manage.sh: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    if output.status.success() {
        return Ok(true);
    }

    if combined.contains("already running") {
        return Ok(false);
    }

    Err(format!(
        "manage.sh run-devenv-agentic failed ({}): {combined}",
        output.status
    ))
}

fn spawn_docker_stack(repo_root: &Option<PathBuf>) -> Result<(), String> {
    let repo = repo_root
        .as_ref()
        .ok_or_else(|| "Penpot repo not found. Set PENPOT_REPO_ROOT.".to_string())?;

    let status = Command::new("docker")
        .args([
            "compose",
            "-f",
            "docker/images/docker-compose.yaml",
            "up",
            "-d",
        ])
        .current_dir(repo)
        .status()
        .map_err(|e| format!("failed to run docker compose: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("docker compose up failed with {status}"))
    }
}

fn spawn_mcp_server(repo_root: &Option<PathBuf>) -> Result<Child, String> {
    let repo = repo_root
        .as_ref()
        .ok_or_else(|| "Penpot repo not found. Set PENPOT_REPO_ROOT.".to_string())?;

    let entry = repo.join("mcp/packages/server/dist/index.js");
    if !entry.exists() {
        return Err(format!(
            "MCP server build not found at {}. Run: cd mcp && pnpm run build",
            entry.display()
        ));
    }

    Command::new("node")
        .arg(entry)
        .current_dir(repo.join("mcp/packages/server"))
        .env("PENPOT_MCP_SERVER_PORT", "4401")
        .env("PENPOT_MCP_WEBSOCKET_PORT", "4402")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start MCP server: {e}"))
}
