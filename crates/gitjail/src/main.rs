mod policy;
mod sandbox;
mod session;
mod shadow;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use session::Session;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "gitjail",
    version,
    about = "OS-level write barrier for AI agents"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 声明一个受保护的项目目录
    Protect {
        /// 目录（会被 canonicalize 后写入策略）
        path: PathBuf,
    },

    /// 取消保护
    Unprotect {
        path: PathBuf,

        /// 必须显式确认
        #[arg(long)]
        yes: bool,
    },

    /// 列出受保护的目录
    Policy,

    /// 在沙箱里运行一个命令
    Run {
        /// 项目目录（默认当前目录）。必须已被 protect
        #[arg(long, short)]
        project: Option<PathBuf>,

        /// 即使没有 protect 也运行（仅用于试用）
        #[arg(long)]
        unprotected: bool,

        /// 要执行的命令，放在 -- 之后
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },

    /// 列出会话
    Log {
        /// 只看这个项目的会话
        #[arg(long, short)]
        project: Option<PathBuf>,
    },

    /// 把会话的改动落到真实目录
    Apply {
        /// 会话 id（默认：当前项目最近一次）
        session: Option<String>,

        #[arg(long, short)]
        project: Option<PathBuf>,

        /// 覆盖会话结束后被改过的文件
        #[arg(long)]
        force: bool,
    },

    /// 显示会话对应的 git 提交
    Show {
        /// 会话 id（默认：当前项目最近一次）
        session: Option<String>,

        #[arg(long, short)]
        project: Option<PathBuf>,

        /// 显示完整 diff
        #[arg(long)]
        patch: bool,
    },

    /// 撤销已经 apply 的会话
    Undo {
        /// 会话 id（默认：当前项目最近一次）
        session: Option<String>,

        #[arg(long, short)]
        project: Option<PathBuf>,

        /// 覆盖 apply 之后被改过的文件
        #[arg(long)]
        force: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Protect { path } => {
            policy::require_outside_sandbox()?;
            let mut p = policy::Policy::load()?;
            let added = p.protect(&path)?;
            println!(
                "{} {}",
                if added {
                    "protected"
                } else {
                    "already protected"
                },
                path.display()
            );
            println!("policy: {}", p.file().display());
        }

        Cmd::Unprotect { path, yes } => {
            policy::require_outside_sandbox()?;
            if !yes {
                bail!(
                    "取消保护需要显式确认：gitjail unprotect {} --yes",
                    path.display()
                );
            }
            let mut p = policy::Policy::load()?;
            let removed = p.unprotect(&path)?;
            println!(
                "{} {}",
                if removed {
                    "unprotected"
                } else {
                    "not protected"
                },
                path.display()
            );
        }

        Cmd::Policy => {
            let p = policy::Policy::load()?;
            if p.paths.is_empty() {
                println!("(no protected paths)");
                println!("add one with: gitjail protect <path>");
            } else {
                for path in &p.paths {
                    println!("{}", path.display());
                }
            }
        }

        Cmd::Run {
            project,
            unprotected,
            command,
        } => {
            let project = canonical(project)?;
            let p = policy::Policy::load()?;

            let root = match p.covering_root(&project) {
                Some(r) => r,
                None if unprotected => project.clone(),
                None => bail!(
                    "{} 没有被 protect\n\n\
                     先声明：gitjail protect {}\n\
                     或者只在沙箱里跑、不要保护声明：gitjail run --unprotected ...",
                    project.display(),
                    project.display()
                ),
            };

            let others = p.others(&root);
            let mut s = Session::create(&project, &root, &command)?;

            let code = sandbox::run(&project, &command, &s.dir, &root, &others)?;
            s.finish()?;
            print_session(&s, true);
            std::process::exit(code);
        }

        Cmd::Log { project } => {
            let project = match project {
                Some(p) => Some(canonical(Some(p))?),
                None => None,
            };
            let list = session::list(project.as_deref())?;
            if list.is_empty() {
                println!("(no sessions)");
                return Ok(());
            }

            println!("{:<24} {:<11} {:>7}  PROJECT", "ID", "STATUS", "CHANGES");
            for m in list {
                let opened = Session::open(&session::dir_of(&m.id)).ok();
                let count = opened.as_ref().map(|s| s.changes.len()).unwrap_or(0);
                let status = opened.as_ref().map(|s| s.status()).unwrap_or("incomplete");
                println!(
                    "{:<24} {:<11} {:>7}  {}",
                    m.id,
                    status,
                    count,
                    m.project.display()
                );
            }
        }

        Cmd::Apply {
            session,
            project,
            force,
        } => {
            let mut s = resolve(session, project)?;
            s.apply(force)?;
            println!("applied {}", s.meta.id);
            print_session(&s, false);
        }

        Cmd::Show {
            session,
            project,
            patch,
        } => {
            let s = resolve(session, project)?;
            let commit = s
                .meta
                .post_commit
                .as_ref()
                .or(s.meta.pre_commit.as_ref())
                .with_context(|| format!("会话 {} 没有 git 历史（先 apply）", s.meta.id))?;
            let shadow = shadow::Shadow::existing(s.root())
                .with_context(|| format!("{} 还没有 shadow repo", s.root().display()))?;
            shadow.show(commit, patch)?;
        }

        Cmd::Undo {
            session,
            project,
            force,
        } => {
            let mut s = resolve(session, project)?;
            s.undo(force)?;
            println!("undone {}", s.meta.id);
            print_session(&s, false);
        }
    }

    Ok(())
}

fn resolve(session: Option<String>, project: Option<PathBuf>) -> Result<Session> {
    match session {
        Some(id) => Session::open_by_id(&id),
        None => {
            let project = canonical(project)?;
            Session::latest_for(&project)
        }
    }
}

fn canonical(p: Option<PathBuf>) -> Result<PathBuf> {
    let p = match p {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    p.canonicalize()
        .with_context(|| format!("project {}", p.display()))
}

fn print_session(s: &Session, with_hint: bool) {
    println!();
    println!("session {}", s.meta.id);
    println!("  root: {}", s.root().display());
    if s.changes.is_empty() {
        println!("  (no changes)");
        return;
    }
    for ch in &s.changes {
        println!("  {} {}", ch.kind, ch.path);
    }
    if with_hint {
        println!();
        println!("  next: gitjail apply {}", s.meta.id);
    }
}
