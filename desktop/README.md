# Penpot Desktop

Tauri-based desktop client for **macOS** and **Windows**. It loads the same Penpot instance as the web browser (shared server, shared files, shared collaboration).

## Architecture

- **Fat client shell**: native window + injected `penpotPublicURI` / `penpotFlags`
- **Shared instance**: default `https://design.penpot.app`, or your team URL
- **Sidecar orchestration**: optional local Penpot / MCP stacks (see below)
- **MCP / Codex**: menu **AI → Copy Codex MCP Config**; point Codex at the same instance `/mcp/stream`
- **Optional devenv**: `PENPOT_DESKTOP_DEV=1` → `https://localhost:3449`

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- Node.js 22+
- macOS 12+ or Windows 10+ (WebView2)

For local devenv: Penpot devenv running on `https://localhost:3449`.

## Quick start

```bash
cd desktop
npm install
npm run dev                  # remote (design.penpot.app)
npm run dev:local            # devenv (https://localhost:3449)
npm run dev:docker           # docker compose stack (http://localhost:9001)
npm run dev:mcp              # remote Penpot + local MCP server (4401/4402)
```

## Settings UI

Menu **File → Settings…** opens the built-in settings window:

- **Penpot 服务器地址** — private / cloud instance URL
- **运行模式** — remote, devenv, docker, managed MCP
- **自动启动 Sidecar** — start local stacks when needed
- **Monorepo 路径** — optional path for `manage.sh` / docker / MCP

Changes are saved to `config.json` and the main Penpot window reloads automatically.

## Configuration

| Source | Priority |
|--------|----------|
| `PENPOT_DESKTOP_URI` env | highest (forces remote profile) |
| `PENPOT_DESKTOP_SIDECAR` env | overrides saved sidecar profile |
| `~/Library/Application Support/penpot-desktop/config.json` (macOS) or equivalent | saved settings |
| `PENPOT_DESKTOP_DEV=1` | sidecar profile → `devenv` |
| default | `remote` → `https://design.penpot.app/` |

Example config file:

```json
{
  "penpot_public_uri": "https://penpot.mycompany.com/",
  "penpot_flags": "enable-feature-render-wasm plugins/runtime",
  "sidecar_profile": "remote",
  "auto_start_sidecar": true,
  "repo_root": "/path/to/penpot"
}
```

Override at runtime:

```bash
PENPOT_DESKTOP_URI=https://penpot.example.com/ npm run dev
PENPOT_DESKTOP_SIDECAR=devenv npm run dev
PENPOT_REPO_ROOT=/path/to/penpot npm run dev:local
```

## Sidecar profiles

The desktop app can orchestrate optional local services before loading Penpot. Use menu **Sidecar** for status / start / stop.

| Profile | Penpot URI | Auto-start (`auto_start_sidecar: true`) |
|---------|------------|----------------------------------------|
| `remote` | configured / cloud default | health check only |
| `devenv` | `https://localhost:3449/` | `./manage.sh run-devenv-agentic` if unreachable |
| `docker_local` | `http://localhost:9001/` | `docker compose -f docker/images/docker-compose.yaml up -d` |
| `managed_mcp` | configured / cloud default | `node mcp/packages/server/dist/index.js` on ports 4401/4402 |

**Prerequisites**

- **devenv**: Penpot monorepo checkout; `manage.sh` on PATH via `PENPOT_REPO_ROOT` or cwd discovery
- **docker_local**: Docker; same repo root for compose file
- **managed_mcp**: `cd mcp && pnpm run build` once; injects `penpotMcpServerURI=ws://127.0.0.1:4402/` for the MCP plugin

On quit, the app stops managed resources it started (docker down, MCP child process). Devenv tmux sessions are left running.

## Codex CLI

1. Open Penpot in the desktop app and enable MCP in settings (create MCP token).
2. Menu **AI → Copy Codex MCP Config** (without token), or use Tauri command `generate_codex_config` with your token.
3. Run Codex:

```bash
codex -c 'mcp_servers.penpot.url="https://YOUR_INSTANCE/mcp/stream?userToken=TOKEN"'
```

Same MCP server as the web UI — desktop and browser users work on the same files.

## Build

```bash
npm run build
```

Artifacts: `src-tauri/target/release/bundle/`

## Rust tests

```bash
npm run check
```
