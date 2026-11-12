//! Shadow repo：每个项目一个 bare git 仓库，记录 apply 前后的树。
//!
//! 不碰项目自己的 `.git`。仓库在
//! `~/.local/share/gitjail/repos/<project-hash>.git`，通过
//! `GIT_DIR` + `GIT_WORK_TREE` 指向项目目录。
//!
//! 只 add 会话里出现过的路径（带 `-f` 绕过 .gitignore），
//! 所以不会把 node_modules 之类的东西卷进来。

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Shadow {
    repo: PathBuf,
    work_tree: PathBuf,
}

impl Shadow {
    /// 打开已有仓库；不存在则创建
    pub fn open(work_tree: &Path) -> Result<Shadow> {
        if let Some(s) = Self::existing(work_tree) {
            return Ok(s);
        }
        let repo = repo_path(work_tree);
        if let Some(parent) = repo.parent() {
            fs::create_dir_all(parent)?;
        }
        let out = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg("--bare")
            .arg(&repo)
            .output()
            .context("执行 git（是不是没装 git？）")?;
        if !out.status.success() {
            anyhow::bail!(
                "git init 失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(Shadow {
            repo,
            work_tree: work_tree.to_path_buf(),
        })
    }

    pub fn existing(work_tree: &Path) -> Option<Shadow> {
        let repo = repo_path(work_tree);
        if repo.is_dir() {
            Some(Shadow {
                repo,
                work_tree: work_tree.to_path_buf(),
            })
        } else {
            None
        }
    }

    /// 把这些路径的当前状态提交成一个 commit。
    /// 没有变化时返回 None。
    pub fn commit(&self, message: &str, paths: &[String]) -> Result<Option<String>> {
        if paths.is_empty() {
            return Ok(None);
        }

        let mut add = self.cmd();
        add.arg("add").arg("-A").arg("-f").arg("--");
        for p in paths {
            add.arg(p);
        }
        self.check(add, "git add")?;

        // 暂存区为空就不提交
        let mut diff = self.cmd();
        diff.arg("diff").arg("--cached").arg("--quiet");
        let status = diff.stdout(Stdio::null()).stderr(Stdio::null()).status()?;
        if status.success() {
            return Ok(None);
        }

        let mut commit = self.cmd();
        commit
            .arg("-c")
            .arg("commit.gpgsign=false")
            .arg("commit")
            .arg("--quiet")
            .arg("-m")
            .arg(message);
        self.check(commit, "git commit")?;

        let out = self.cmd().arg("rev-parse").arg("HEAD").output()?;
        if !out.status.success() {
            anyhow::bail!("git rev-parse HEAD 失败");
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    pub fn show(&self, commit: &str, patch: bool) -> Result<()> {
        let mut c = self.cmd();
        c.arg("show").arg("--stat");
        if patch {
            c.arg("--patch");
        } else {
            c.arg("--format=%H%n%an <%ae>%n%ad%n%n%s%n%n%b");
        }
        c.arg(commit);
        let status = c.stdout(Stdio::inherit()).status()?;
        if !status.success() {
            anyhow::bail!("git show 失败");
        }
        Ok(())
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("git");
        let mut git_dir = OsString::from("--git-dir=");
        git_dir.push(&self.repo);
        let mut wt = OsString::from("--work-tree=");
        wt.push(&self.work_tree);

        c.arg(git_dir)
            .arg(wt)
            .arg("-c")
            .arg("core.bare=false")
            .env("GIT_AUTHOR_NAME", "gitjail")
            .env("GIT_AUTHOR_EMAIL", "gitjail@localhost")
            .env("GIT_COMMITTER_NAME", "gitjail")
            .env("GIT_COMMITTER_EMAIL", "gitjail@localhost");
        c
    }

    fn check(&self, mut c: Command, what: &str) -> Result<()> {
        let out = c.output()?;
        if !out.status.success() {
            anyhow::bail!(
                "{what} 失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }
}

fn repo_path(project: &Path) -> PathBuf {
    crate::session::data_root()
        .join("repos")
        .join(format!("{}.git", hash(project)))
}

/// FNV-1a 64：稳定、够用，不引依赖
fn hash(project: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in project.as_os_str().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}
