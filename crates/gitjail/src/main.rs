mod sandbox;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Run { project, command } => {
            let project = match project {
                Some(p) => p,
                None => std::env::current_dir()?,
            };
            let project = project
                .canonicalize()
                .with_context(|| format!("project {}", project.display()))?;

            let code = sandbox::run(&project, &command)?;
            std::process::exit(code);
        }
    }
}
