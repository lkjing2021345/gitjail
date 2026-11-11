mod sandbox;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => {
            let rest: Vec<String> = args.collect();
            match parse_run(&rest) {
                Ok((project, cmd)) => match sandbox::run(&project, &cmd) {
                    Ok(code) => ExitCode::from(code as u8),
                    Err(e) => {
                        eprintln!("gitjail: {e}");
                        ExitCode::FAILURE
                    }
                },
                Err(e) => {
                    eprintln!("gitjail: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Some("version") | Some("-V") | Some("--version") => {
            println!("gitjail {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!("gitjail {}", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    gitjail run [--project <PATH>] -- <COMMAND>...");
    eprintln!();
    eprintln!("EXAMPLE:");
    eprintln!("    gitjail run -- codex");
    eprintln!("    gitjail run --project ~/code/proj -- bash");
}

fn parse_run(args: &[String]) -> Result<(PathBuf, Vec<String>), String> {
    let mut project: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" | "-p" => {
                i += 1;
                let v = args.get(i).ok_or("--project 需要一个路径")?;
                project = Some(PathBuf::from(v));
            }
            "--" => {
                i += 1;
                break;
            }
            s if s.starts_with('-') => return Err(format!("未知参数: {s}")),
            _ => break,
        }
        i += 1;
    }

    let cmd = args[i..].to_vec();
    if cmd.is_empty() {
        return Err("缺少命令，例如: gitjail run -- bash".into());
    }

    let project = match project {
        Some(p) => p,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let project = project
        .canonicalize()
        .map_err(|e| format!("project {}: {e}", project.display()))?;

    Ok((project, cmd))
}
