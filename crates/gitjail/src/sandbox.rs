//! 沙箱：把进程放进一个「只能写 upper」的世界。
//!
//! 顺序：
//!   1. unshare(CLONE_NEWUSER)，写 uid_map / gid_map
//!   2. unshare(CLONE_NEWNS | CLONE_NEWPID)
//!   3. / 设为 private
//!   4. 其余受保护根 bind 成只读
//!   5. 本次会话的根 bind 成只读 + overlay
//!   6. 合并视图 bind 回原路径（agent 看到的路径不变）
//!   7. fork 出 pid 1，挂 /proc，exec 命令

use anyhow::{Context, Result};
use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// mount_setattr(2) 的参数
#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

const MOUNT_ATTR_RDONLY: u64 = 0x0000_0001;

pub fn run(
    project: &Path,
    cmd: &[String],
    session_dir: &Path,
    root: &Path,
    others: &[PathBuf],
) -> Result<i32> {
    let base = session_dir.to_path_buf();
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error()).context("fork");
    }
    if pid == 0 {
        let code = match child_main(project, cmd, &base, root, others, uid, gid) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("gitjail: {e:#}");
                1
            }
        };
        unsafe { libc::_exit(code) };
    }

    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return Err(io::Error::last_os_error()).context("waitpid");
    }
    Ok(exit_code(status))
}

fn child_main(
    project: &Path,
    cmd: &[String],
    base: &Path,
    root: &Path,
    others: &[PathBuf],
    uid: u32,
    gid: u32,
) -> Result<i32> {
    // 必须分两步：先 userns，写好映射，再 mount/pid namespace。
    if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0 {
        return Err(io::Error::last_os_error())
            .context("unshare(CLONE_NEWUSER)：需要 CONFIG_USER_NS，且不能用 root 跑");
    }
    write_maps(uid, gid)?;

    if unsafe { libc::unshare(libc::CLONE_NEWNS | libc::CLONE_NEWPID) } != 0 {
        return Err(io::Error::last_os_error()).context("unshare(NS|PID)");
    }

    setup_mounts(root, base, others)?;

    // CLONE_NEWPID 只影响之后 fork 的子进程
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error()).context("fork");
    }
    if pid == 0 {
        if let Err(e) = mnt(Path::new("proc"), Path::new("/proc"), Some("proc"), 0, None) {
            eprintln!("gitjail: mount /proc: {e:#}");
            unsafe { libc::_exit(1) };
        }
        if let Err(e) = std::env::set_current_dir(project) {
            eprintln!("gitjail: chdir {}: {e}", project.display());
            unsafe { libc::_exit(1) };
        }
        exec(cmd);
    }

    let mut status = 0;
    unsafe { libc::waitpid(pid, &mut status, 0) };
    Ok(exit_code(status))
}

fn write_maps(uid: u32, gid: u32) -> Result<()> {
    // setgroups 在部分内核上不存在，失败可忽略
    let _ = std::fs::write("/proc/self/setgroups", b"deny");
    std::fs::write("/proc/self/uid_map", format!("0 {uid} 1\n")).context("写 uid_map")?;
    std::fs::write("/proc/self/gid_map", format!("0 {gid} 1\n")).context("写 gid_map")?;
    Ok(())
}

fn setup_mounts(root: &Path, base: &Path, others: &[PathBuf]) -> Result<()> {
    // 1. 挂载不传播到宿主
    mnt(
        Path::new("none"),
        Path::new("/"),
        None,
        libc::MS_REC | libc::MS_PRIVATE,
        None,
    )?;

    // 2. 其余受保护目录：一律只读。
    //    不能用 mount(MS_REMOUNT|MS_BIND|MS_RDONLY)：MS_RDONLY 是 superblock
    //    标志，在 userns 里改会 EPERM。mount_setattr 是 per-mount 的。
    for other in others {
        mnt(other, other, None, libc::MS_BIND, None)?;
        ro_mount(other)?;
    }

    // 3. 本次会话的根：只读 + overlay
    mnt(root, root, None, libc::MS_BIND, None)?;
    ro_mount(root)?;

    let merged = base.join("merged");
    let opts = format!(
        "lowerdir={},upperdir={},workdir={}",
        root.display(),
        base.join("upper").display(),
        base.join("work").display()
    );
    mnt(
        Path::new("overlay"),
        &merged,
        Some("overlay"),
        0,
        Some(&opts),
    )?;

    // 4. 合并视图挂回原路径：agent 看到的路径不变
    mnt(&merged, root, None, libc::MS_BIND, None)?;
    Ok(())
}

fn mnt(
    src: &Path,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> Result<()> {
    let src_c = cstr(src.as_os_str())?;
    let tgt_c = cstr(target.as_os_str())?;
    let fst_c = fstype.map(CString::new).transpose()?;
    let dat_c = data.map(CString::new).transpose()?;

    let r = unsafe {
        libc::mount(
            src_c.as_ptr(),
            tgt_c.as_ptr(),
            fst_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            flags,
            dat_c
                .as_ref()
                .map_or(std::ptr::null(), |c| c.as_ptr() as *const libc::c_void),
        )
    };
    if r != 0 {
        let e = io::Error::last_os_error();
        anyhow::bail!("mount {} -> {}: {e}", src.display(), target.display());
    }
    Ok(())
}

fn ro_mount(path: &Path) -> Result<()> {
    let c = cstr(path.as_os_str())?;
    let attr = MountAttr {
        attr_set: MOUNT_ATTR_RDONLY,
        attr_clr: 0,
        propagation: 0,
        userns_fd: 0,
    };
    let r = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            libc::AT_FDCWD,
            c.as_ptr(),
            0 as libc::c_uint,
            &attr as *const MountAttr,
            std::mem::size_of::<MountAttr>(),
        )
    };
    if r != 0 {
        let e = io::Error::last_os_error();
        anyhow::bail!("mount_setattr(ro) {}: {e}", path.display());
    }
    Ok(())
}

fn exec(cmd: &[String]) -> ! {
    let c: Vec<CString> = cmd
        .iter()
        .map(|s| CString::new(s.as_str()).expect("arg contains NUL"))
        .collect();
    let mut ptrs: Vec<*const libc::c_char> = c.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe { libc::execvp(c[0].as_ptr(), ptrs.as_ptr()) };
    eprintln!("gitjail: exec {}: {}", cmd[0], io::Error::last_os_error());
    unsafe { libc::_exit(127) }
}

fn exit_code(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

fn cstr(s: &OsStr) -> Result<CString> {
    Ok(CString::new(s.as_bytes())?)
}
