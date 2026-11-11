mod sandbox;
mod session;

use anyhow::{Context, Result};
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
    /// 在沙箱里运行一个命令
    Run {
        /// 受保护的项目目录（默认当前目录）
        #[arg(long, short)]
        project: Option<PathBuf>,

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
        Cmd::Run { project, command } => {
            let project = canonical(project)?;
            let mut s = Session::create(&project, &command)?;

            let code = sandbox::run(&project, &command, &s.dir)?;
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
                let opened = session::Session::open(&session::dir_of(&m.id)).ok();
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
