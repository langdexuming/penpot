use std::path::{Path, PathBuf};

const MARKERS: &[&str] = &["manage.sh", "docker/images/docker-compose.yaml", "mcp/package.json"];

pub fn resolve_repo_root() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("PENPOT_REPO_ROOT") {
        let path = PathBuf::from(explicit);
        if is_penpot_repo(&path) {
            return Some(path);
        }
    }

    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..8 {
        if is_penpot_repo(&dir) {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        for _ in 0..8 {
            if is_penpot_repo(&dir) {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    None
}

fn is_penpot_repo(path: &Path) -> bool {
    MARKERS.iter().all(|marker| path.join(marker).exists())
}
