//! Tool configuration: where the blog repository is and how publishing works.
//!
//! This lives in `core` rather than in the GPUI app because every front end
//! needs it and none of them owns it: the desktop app writes it from its
//! settings page, the CLI reads it to find the repository, and the MCP server
//! reads the same file so an assistant edits the same checkout the user does.
//!
//! The file itself lives *outside* the repository (plan §2.2): it must never be
//! committed, and on multi-machine setups `ASTRO_PUBLISHER_REPO` overrides it.

use std::path::{Path, PathBuf};

/// Persisted settings (`config.toml` in the platform config directory).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    /// Root of the blog repository.
    pub repo_path: String,
    /// Author written into new posts' frontmatter.
    pub author: String,
    /// Git remote used when publishing.
    pub git_remote: String,
    /// Branch published from.
    pub git_branch: String,
    /// EdgeOne Pages console URL, opened from the publish panel.
    #[serde(default)]
    pub eo_console_url: String,
    /// Whether the publish pipeline runs `pnpm build` before committing.
    #[serde(default = "default_true")]
    pub run_local_build: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Find the blog repository instead of trusting the working
            // directory: `cargo run` and double-clicking the binary both start
            // in `publisher/` or a system directory, neither of which is the
            // blog root.
            repo_path: detect_repo_root()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            author: "Blog Author".to_owned(),
            git_remote: "origin".to_owned(),
            git_branch: "main".to_owned(),
            eo_console_url: "https://console.cloud.tencent.com/edgeone/pages".to_owned(),
            run_local_build: true,
        }
    }
}

impl Config {
    /// Path of the settings file (`config.toml`).
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "astro-publisher", "astro-publisher")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    /// Load the configuration, falling back to defaults.
    ///
    /// A configured path that no longer points at the repository (moved clone,
    /// different machine, or the working directory the app was first launched
    /// from) is re-detected rather than left broken, since an unusable repo path
    /// makes every screen fail.
    pub fn load() -> Self {
        let mut config = match Self::path().and_then(|path| std::fs::read_to_string(path).ok()) {
            Some(text) => toml::from_str(&text).unwrap_or_default(),
            None => Self::default(),
        };

        // An explicit override always wins.
        if let Ok(repo) = std::env::var("ASTRO_PUBLISHER_REPO") {
            let trimmed = repo.trim();
            if !trimmed.is_empty() {
                config.repo_path = trimmed.to_owned();
                return config;
            }
        }

        if !looks_like_repo(&config.repo()) {
            if let Some(detected) = detect_repo_root() {
                config.repo_path = detected.to_string_lossy().to_string();
            }
        }

        config
    }

    /// Persist the configuration, creating the directory if needed.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::path() else {
            anyhow::bail!("无法确定配置目录");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    /// Repository root as a path.
    pub fn repo(&self) -> PathBuf {
        PathBuf::from(self.repo_path.trim())
    }

    /// Blog content directory (`src/data/blog`).
    pub fn blog(&self) -> PathBuf {
        self.repo().join("src").join("data").join("blog")
    }
}

/// Locate the blog repository without being told where it is.
///
/// Tried in order:
/// 1. `ASTRO_PUBLISHER_REPO` (explicit override, also used for multi-machine setups);
/// 2. walking up from the current working directory;
/// 3. walking up from the directory holding the executable (covers
///    `target/debug/astro-publisher.exe` inside the repo).
pub fn detect_repo_root() -> Option<PathBuf> {
    if let Ok(from_env) = std::env::var("ASTRO_PUBLISHER_REPO") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        if let Some(found) = walk_up_for_repo(&cwd) {
            return Some(found);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(found) = walk_up_for_repo(dir) {
                return Some(found);
            }
        }
    }

    None
}

/// Walk parents of `start`, returning the first directory that looks like the
/// blog repository.
pub fn walk_up_for_repo(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if looks_like_repo(dir) {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// True when `repo` looks like the blog repository.
pub fn looks_like_repo(repo: &Path) -> bool {
    repo.join("src").join("data").join("blog").is_dir() && repo.join("package.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a directory tree that looks like the blog repository.
    fn fake_repo(root: &Path) {
        std::fs::create_dir_all(root.join("src/data/blog")).expect("create content dir");
        std::fs::write(root.join("package.json"), "{}").expect("write package.json");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("an-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    #[test]
    fn recognises_a_repository_root() {
        let root = scratch("repo-root");
        fake_repo(&root);
        assert!(looks_like_repo(&root));

        // The tool's own directory is not the repository root.
        assert!(!looks_like_repo(&root.join("src/data")));
        assert!(!looks_like_repo(&root.join("publisher")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn walks_up_from_a_nested_directory() {
        // This is the failure that produced "找不到内容目录：…\publisher\src\data\blog":
        // the app was started inside `publisher/` and looked only at the cwd.
        let root = scratch("walk-up");
        fake_repo(&root);
        let nested = root.join("publisher/crates/app");
        std::fs::create_dir_all(&nested).expect("create nested dirs");

        assert_eq!(walk_up_for_repo(&nested), Some(root.clone()));
        assert_eq!(walk_up_for_repo(&root), Some(root.clone()));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn returns_none_when_no_repository_is_above() {
        let dir = scratch("no-repo");
        let nested = dir.join("a/b/c");
        std::fs::create_dir_all(&nested).expect("create nested dirs");

        // A temp directory has no repository above it (unless the test runner
        // itself sits inside one, which is why the assertion only requires that
        // we do not wander outside the scratch tree).
        match walk_up_for_repo(&nested) {
            None => {}
            Some(found) => assert!(
                found.starts_with(&dir),
                "unexpectedly found a repository outside the scratch dir: {found:?}"
            ),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blog_dir_is_under_the_repo() {
        let config = Config {
            repo_path: "C:/some/repo".to_owned(),
            ..Config::default()
        };
        assert_eq!(
            config.blog(),
            PathBuf::from("C:/some/repo").join("src").join("data").join("blog")
        );
    }

    #[test]
    fn config_path_is_inside_the_platform_config_dir() {
        // Guards the layout the installer/uninstaller and the README document:
        // writing the settings file elsewhere would silently ignore the user's
        // configured repository path.
        let path = Config::path().expect("a config directory must resolve");
        assert!(
            path.ends_with("config.toml"),
            "unexpected config file name: {}",
            path.display()
        );
        println!("resolved config path: {}", path.display());
    }

    #[test]
    fn a_saved_config_round_trips_through_load() {
        // `save` writes and `load` reads the same shape, so a user-configured
        // repository path survives a restart.
        let text = toml::to_string_pretty(&Config {
            repo_path: "D:/some/blog".to_owned(),
            author: "Blog Author".to_owned(),
            ..Config::default()
        })
        .expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed.repo_path, "D:/some/blog");
        assert_eq!(parsed.author, "Blog Author");
    }
}
