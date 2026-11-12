//! 策略：哪些目录是「声明过的保护区」。
//!
//! 默认零保护。只有 `gitjail protect` 写进 policy.toml 的路径才受保护；
//! `run` 会拒绝在未声明的目录里启动。
//!
//! 文件位置：`$GITJAIL_CONFIG_DIR` > `$XDG_CONFIG_HOME/gitjail` > `~/.config/gitjail`

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default)]
struct PolicyFile {
    #[serde(default)]
    protect: Protect,
}

#[derive(Serialize, Deserialize, Default)]
struct Protect {
    #[serde(default)]
    paths: Vec<PathBuf>,
}

pub struct Policy {
    pub paths: Vec<PathBuf>,
    file: PathBuf,
}

impl Policy {
    pub fn load() -> Result<Policy> {
        let file = config_dir().join("policy.toml");
        if !file.exists() {
            return Ok(Policy {
                paths: Vec::new(),
                file,
            });
        }
        let text = fs::read_to_string(&file).with_context(|| format!("读 {}", file.display()))?;
        let parsed: PolicyFile =
            toml::from_str(&text).with_context(|| format!("解析 {}", file.display()))?;
        let mut paths = parsed.protect.paths;
        paths.sort();
        paths.dedup();
        Ok(Policy { paths, file })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let doc = PolicyFile {
            protect: Protect {
                paths: self.paths.clone(),
            },
        };
        let text = toml::to_string_pretty(&doc)?;
        let tmp = self.file.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &self.file)?;
        Ok(())
    }

    pub fn protect(&mut self, path: &Path) -> Result<bool> {
        let path = canonical(path)?;
        if self.paths.iter().any(|p| p == &path) {
            return Ok(false);
        }
        self.paths.push(path);
        self.paths.sort();
        self.save()?;
        Ok(true)
    }

    pub fn unprotect(&mut self, path: &Path) -> Result<bool> {
        let path = canonical(path)?;
        let before = self.paths.len();
        self.paths.retain(|p| p != &path);
        if self.paths.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// 项目所在的最外层受保护根。保护 `~/code` 时，`~/code/proj` 也命中。
    pub fn covering_root(&self, project: &Path) -> Option<PathBuf> {
        self.paths
            .iter()
            .filter(|p| project == p.as_path() || project.starts_with(p))
            .max_by_key(|p| p.as_os_str().len())
            .cloned()
    }

    /// 除了本次使用的根，其余受保护根在沙箱里一律只读
    pub fn others(&self, except: &Path) -> Vec<PathBuf> {
        self.paths
            .iter()
            .filter(|p| p.as_path() != except && !p.starts_with(except))
            .cloned()
            .collect()
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}

/// 当前进程是不是在 gitjail 的沙箱里。
///
/// userns 里 uid 0 映射到宿主 uid，uid_map 形如 "0 1000 1"；
/// 真正的 root 是 "0 0 4294967295"。
pub fn in_sandbox() -> bool {
    let real_root = unsafe { libc::getuid() } == 0;
    if !real_root {
        return false;
    }
    match fs::read_to_string("/proc/self/uid_map") {
        Ok(s) => {
            let f: Vec<&str> = s.split_whitespace().collect();
            f.len() >= 3 && f[0] == "0" && f[1] != "0" && f[2] == "1"
        }
        Err(_) => false,
    }
}

fn canonical(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("路径不存在: {}", path.display()))
}

fn config_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("GITJAIL_CONFIG_DIR") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("gitjail");
        }
    }
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h).join(".config/gitjail"),
        _ => PathBuf::from(".gitjail"),
    }
}

pub fn require_outside_sandbox() -> Result<()> {
    if in_sandbox() {
        bail!("该操作必须在沙箱外执行");
    }
    Ok(())
}
