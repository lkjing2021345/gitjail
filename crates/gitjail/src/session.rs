//! 会话：一次 `gitjail run` 的全部痕迹。
//!
//! 目录结构：
//! ```text
//! ~/.local/share/gitjail/sessions/<id>/
//! ├── meta.json      项目、命令、时间、状态
//! ├── changes.json   A/M/D/L 列表
//! ├── upper/         overlay 上层（agent 的产物）
//! ├── work/          overlay workdir
//! ├── merged/        挂载点
//! └── pre/           apply 前的原文件（undo 用）
//! ```

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone)]
pub struct Meta {
    pub id: String,
    pub project: PathBuf,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub applied_at: Option<u64>,
    pub command: Vec<String>,
    pub applied: bool,
    #[serde(default)]
    pub undone: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Change {
    pub kind: char,
    pub path: String,
}

pub struct Session {
    pub dir: PathBuf,
    pub meta: Meta,
    pub changes: Vec<Change>,
}

impl Session {
    pub fn create(project: &Path, command: &[String]) -> Result<Session> {
        let id = format!("{}-{}", std::process::id(), now_nanos());
        let dir = sessions_root().join(&id);
        fs::create_dir_all(dir.join("upper"))?;
        fs::create_dir_all(dir.join("work"))?;
        fs::create_dir_all(dir.join("merged"))?;

        let meta = Meta {
            id: id.clone(),
            project: project.to_path_buf(),
            started_at: now_ns(),
            finished_at: None,
            applied_at: None,
            command: command.to_vec(),
            applied: false,
            undone: false,
        };
        let s = Session {
            dir,
            meta,
            changes: Vec::new(),
        };
        s.save_meta()?;
        Ok(s)
    }

    pub fn open_by_id(id: &str) -> Result<Session> {
        let dir = sessions_root().join(id);
        if !dir.is_dir() {
            bail!("找不到会话 {id}");
        }
        Self::open(&dir)
    }

    pub fn open(dir: &Path) -> Result<Session> {
        let meta: Meta =
            serde_json::from_slice(&fs::read(dir.join("meta.json")).context("读 meta.json")?)
                .context("解析 meta.json")?;
        let changes: Vec<Change> = match fs::read(dir.join("changes.json")) {
            Ok(b) => serde_json::from_slice(&b).context("解析 changes.json")?,
            Err(_) => Vec::new(),
        };
        Ok(Session {
            dir: dir.to_path_buf(),
            meta,
            changes,
        })
    }

    pub fn latest_for(project: &Path) -> Result<Session> {
        let mut best: Option<Meta> = None;
        for m in list(Some(project))? {
            if best.as_ref().map_or(true, |b| m.started_at >= b.started_at) {
                best = Some(m);
            }
        }
        match best {
            Some(m) => Self::open(&sessions_root().join(&m.id)),
            None => bail!("{} 还没有任何会话", project.display()),
        }
    }

    pub fn save_meta(&self) -> Result<()> {
        let b = serde_json::to_vec_pretty(&self.meta)?;
        fs::write(self.dir.join("meta.json"), b)?;
        Ok(())
    }

    /// 沙箱退出后调用：把 upper 层翻译成变更列表并落盘
    pub fn finish(&mut self) -> Result<()> {
        let upper = self.dir.join("upper");
        let mut out = Vec::new();
        walk(&upper, &upper, &self.meta.project, &mut out)?;
        out.sort_by(|a, b| a.path.cmp(&b.path));

        fs::write(
            self.dir.join("changes.json"),
            serde_json::to_vec_pretty(&out)?,
        )?;
        self.changes = out;
        self.meta.finished_at = Some(now_ns());
        self.save_meta()?;
        Ok(())
    }

    /// 把会话的改动落到真实目录，同时保存 pre-image
    pub fn apply(&mut self, force: bool) -> Result<()> {
        if self.meta.applied {
            bail!("会话 {} 已经 apply 过了", self.meta.id);
        }
        if self.changes.is_empty() {
            bail!("会话 {} 没有变更", self.meta.id);
        }

        let finished = self.meta.finished_at.unwrap_or_else(now_ns);
        let pre_root = self.dir.join("pre");
        let upper = self.dir.join("upper");

        // 先做完整冲突检查，任何一个冲突都不动文件。
        // 否则部分 apply 会让 pre-image 与真实状态错位，undo 就不准了。
        let mut conflicts: Vec<&str> = Vec::new();
        for ch in &self.changes {
            let dst = self.meta.project.join(&ch.path);
            let hit = match ch.kind {
                'A' => exists(&dst),
                _ => is_newer_than(&dst, finished),
            };
            if hit {
                conflicts.push(&ch.path);
            }
        }
        if !conflicts.is_empty() && !force {
            for p in &conflicts {
                eprintln!("  ! {p} 在会话结束后被改过");
            }
            bail!(
                "{} 个文件有冲突，未做任何修改（--force 覆盖）",
                conflicts.len()
            );
        }

        for ch in &self.changes {
            let dst = self.meta.project.join(&ch.path);
            let src = upper.join(&ch.path);

            match ch.kind {
                'D' => {
                    if exists(&dst) {
                        save_pre(&dst, &pre_root.join(&ch.path))?;
                        remove_entry(&dst)?;
                    }
                }
                'A' => copy_entry(&src, &dst)?,
                'M' | 'L' => {
                    if exists(&dst) {
                        save_pre(&dst, &pre_root.join(&ch.path))?;
                    }
                    copy_entry(&src, &dst)?;
                }
                other => bail!("未知变更类型 {other}"),
            }
        }

        self.meta.applied = true;
        self.meta.applied_at = Some(now_ns());
        self.meta.undone = false;
        self.save_meta()?;
        Ok(())
    }

    /// 撤销 apply：pre 里有就还原，没有（原本不存在）就删掉
    pub fn undo(&mut self, force: bool) -> Result<()> {
        if !self.meta.applied {
            bail!("会话 {} 还没有 apply，没有可撤销的东西", self.meta.id);
        }
        let applied_at = self.meta.applied_at.unwrap_or_else(now_ns);
        let pre_root = self.dir.join("pre");

        // 同样先检查：apply 之后被改过的文件不回滚
        let mut conflicts: Vec<&str> = Vec::new();
        for ch in &self.changes {
            let dst = self.meta.project.join(&ch.path);
            if exists(&dst) && is_newer_than(&dst, applied_at) {
                conflicts.push(&ch.path);
            }
        }
        if !conflicts.is_empty() && !force {
            for p in &conflicts {
                eprintln!("  ! {p} 在 apply 之后被改过");
            }
            bail!(
                "{} 个文件有冲突，未做任何修改（--force 覆盖）",
                conflicts.len()
            );
        }

        for ch in self.changes.iter().rev() {
            let dst = self.meta.project.join(&ch.path);
            let pre = pre_root.join(&ch.path);

            if exists(&pre) {
                copy_entry(&pre, &dst)?;
            } else if ch.kind == 'A' && exists(&dst) {
                remove_entry(&dst)?;
            }
        }

        self.meta.applied = false;
        self.meta.applied_at = None;
        self.meta.undone = true;
        self.save_meta()?;
        Ok(())
    }

    pub fn status(&self) -> &'static str {
        if self.meta.applied {
            "applied"
        } else if self.meta.finished_at.is_none() {
            "incomplete"
        } else if self.meta.undone {
            "undone"
        } else {
            "pending"
        }
    }
}

pub fn list(project: Option<&Path>) -> Result<Vec<Meta>> {
    let root = sessions_root();
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    for entry in fs::read_dir(root)? {
        let p = entry?.path();
        if !p.is_dir() {
            continue;
        }
        if let Ok(meta) = read_meta(&p) {
            if project.map_or(true, |f| meta.project == f) {
                out.push(meta);
            }
        }
    }
    out.sort_by_key(|m| m.started_at);
    Ok(out)
}

fn read_meta(dir: &Path) -> Result<Meta> {
    Ok(serde_json::from_slice(&fs::read(dir.join("meta.json"))?)?)
}

pub fn dir_of(id: &str) -> PathBuf {
    sessions_root().join(id)
}

// ---- 文件操作 ------------------------------------------------------------

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn is_newer_than(p: &Path, ts_ns: u64) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(p)
        .map(|m| m.mtime() as u64 * 1_000_000_000 + m.mtime_nsec() as u64 > ts_ns)
        .unwrap_or(false)
}

fn save_pre(src: &Path, dst: &Path) -> Result<()> {
    copy_entry(src, dst).with_context(|| format!("保存 pre-image {}", src.display()))
}

fn copy_entry(src: &Path, dst: &Path) -> Result<()> {
    let md = fs::symlink_metadata(src).with_context(|| format!("stat {}", src.display()))?;
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    if md.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        if exists(dst) {
            remove_entry(dst)?;
        }
        std::os::unix::fs::symlink(target, dst)?;
    } else {
        if exists(dst) {
            remove_entry(dst)?;
        }
        fs::copy(src, dst)?;
        fs::set_permissions(dst, md.permissions())?;
    }
    Ok(())
}

fn remove_entry(p: &Path) -> Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.file_type().is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        fs::remove_file(p)?;
    }
    Ok(())
}

fn walk(root: &Path, dir: &Path, project: &Path, out: &mut Vec<Change>) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let md = fs::symlink_metadata(&path)?;
        let ft = md.file_type();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();

        if ft.is_char_device() && md.rdev() == 0 {
            out.push(Change {
                kind: 'D',
                path: rel,
            });
        } else if ft.is_dir() {
            walk(root, &path, project, out)?;
        } else if ft.is_symlink() {
            out.push(Change {
                kind: 'L',
                path: rel,
            });
        } else if ft.is_file() {
            let kind = if project.join(&rel).exists() {
                'M'
            } else {
                'A'
            };
            out.push(Change { kind, path: rel });
        }
    }
    Ok(())
}

// ---- 路径 ----------------------------------------------------------------

fn sessions_root() -> PathBuf {
    data_root().join("sessions")
}

fn data_root() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("gitjail");
        }
    }
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h).join(".local/share/gitjail"),
        _ => PathBuf::from(".gitjail"),
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
