//! 沙箱：把进程放进一个「只能写 upper」的世界。
//!
//! 顺序：
//!   1. unshare(USER | NS | PID)
//!   2. 写 uid_map / gid_map
//!   3. / 设为 private，项目目录 bind + remount ro
//!   4. overlay(lower=项目, upper=session) 挂到 staging
//!   5. 把合并视图 bind 回原路径（agent 看到的路径不变）
//!   6. fork 出 pid 1，挂 /proc，exec 命令
//!
//! 零依赖：直接用 libc 的 ABI，不引入 crate。

use std::ffi::{CString, OsStr};
use std::fs;
use std::io;
use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ---- 用到的 libc ABI ----------------------------------------------------

type PidT = c_int;

const CLONE_NEWNS: c_int = 0x0002_0000;
const CLONE_NEWUSER: c_int = 0x1000_0000;
const CLONE_NEWPID: c_int = 0x2000_0000;

const MS_BIND: c_ulong = 4096;
const MS_REC: c_ulong = 16384;
const MS_PRIVATE: c_ulong = 1 << 18;

const AT_FDCWD: c_int = -100;
const MOUNT_ATTR_RDONLY: u64 = 0x0000_0001;

#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

extern "C" {
    fn unshare(flags: c_int) -> c_int;
    fn mount(
        src: *const c_char,
        target: *const c_char,
        fstype: *const c_char,
        flags: c_ulong,
        data: *const c_void,
    ) -> c_int;
    fn mount_setattr(
        dfd: c_int,
        path: *const c_char,
        flags: c_uint,
        attr: *const MountAttr,
        size: usize,
    ) -> c_int;
    fn fork() -> PidT;
    fn waitpid(pid: PidT, status: *mut c_int, options: c_int) -> PidT;
    fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    fn _exit(status: c_int) -> !;
    fn getuid() -> u32;
    fn getgid() -> u32;
}

fn wifexited(s: c_int) -> bool {
    s & 0x7f == 0
}
fn wexitstatus(s: c_int) -> i32 {
    (s >> 8) & 0xff
}
fn wifsignaled(s: c_int) -> bool {
    (((s & 0x7f) + 1) >> 1) > 0
}
fn wtermsig(s: c_int) -> i32 {
    s & 0x7f
}

// ---- 入口 ----------------------------------------------------------------

pub fn run(project: &Path, cmd: &[String]) -> io::Result<i32> {
    let base = runtime_base().join(session_id());
    fs::create_dir_all(base.join("upper"))?;
    fs::create_dir_all(base.join("work"))?;
    fs::create_dir_all(base.join("merged"))?;

    let uid = unsafe { getuid() };
    let gid = unsafe { getgid() };

    let pid = unsafe { fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        let code = match child_main(project, cmd, &base, uid, gid) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("gitjail: {e}");
                1
            }
        };
        unsafe { _exit(code) };
    }

    let mut status = 0;
    if unsafe { waitpid(pid, &mut status, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let code = exit_code(status);

    report(&base, project)?;
    Ok(code)
}

fn child_main(project: &Path, cmd: &[String], base: &Path, uid: u32, gid: u32) -> io::Result<i32> {
    // 必须分两步：先 userns，写好映射，再 unmount/pid namespace。
    // 同一次 unshare(USER|NS|PID) 里创建的 mount ns 会继承 locked 挂载，
    // 后续 bind mount 会得到 EPERM。
    if unsafe { unshare(CLONE_NEWUSER) } != 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(
            e.kind(),
            format!("unshare(USER): {e}（需要 CONFIG_USER_NS，且不能用 root 跑）"),
        ));
    }
    write_maps(uid, gid)?;

    if unsafe { unshare(CLONE_NEWNS | CLONE_NEWPID) } != 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(e.kind(), format!("unshare(NS|PID): {e}")));
    }

    setup_mounts(project, base)?;

    // CLONE_NEWPID 只影响之后 fork 的子进程
    let pid = unsafe { fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        if let Err(e) = mnt(Path::new("proc"), Path::new("/proc"), Some("proc"), 0, None) {
            eprintln!("gitjail: mount /proc: {e}");
            unsafe { _exit(1) };
        }
        if let Err(e) = std::env::set_current_dir(project) {
            eprintln!("gitjail: chdir {}: {e}", project.display());
            unsafe { _exit(1) };
        }
        exec(cmd);
    }

    let mut status = 0;
    unsafe { waitpid(pid, &mut status, 0) };
    Ok(exit_code(status))
}

fn write_maps(uid: u32, gid: u32) -> io::Result<()> {
    // setgroups 在部分内核上不存在，失败可忽略
    let _ = fs::write("/proc/self/setgroups", b"deny");
    fs::write("/proc/self/uid_map", format!("0 {uid} 1\n"))?;
    fs::write("/proc/self/gid_map", format!("0 {gid} 1\n"))?;
    Ok(())
}

fn setup_mounts(project: &Path, base: &Path) -> io::Result<()> {
    // 1. 挂载不传播到宿主
    mnt(
        Path::new("none"),
        Path::new("/"),
        None,
        MS_REC | MS_PRIVATE,
        None,
    )?;

    // 2. 项目目录成为独立挂载点，然后只读。
    //    注意：不能用 mount(MS_REMOUNT|MS_BIND|MS_RDONLY)：MS_RDONLY 是
    //    superblock 标志，在 userns 里改会 EPERM。要用 mount_setattr(2)，
    //    它是 per-mount 的（Linux 5.12+）。
    mnt(project, project, None, MS_BIND, None)?;
    ro_mount(project)?;

    // 3. overlay：lower = 只读项目，upper/work = 会话目录
    let merged = base.join("merged");
    let opts = format!(
        "lowerdir={},upperdir={},workdir={}",
        project.display(),
        base.join("upper").display(),
        base.join("work").display()
    );
    mnt(Path::new("overlay"), &merged, Some("overlay"), 0, Some(&opts))?;

    // 4. 合并视图挂回原路径：agent 看到的路径不变
    mnt(&merged, project, None, MS_BIND, None)?;
    Ok(())
}

fn mnt(
    src: &Path,
    target: &Path,
    fstype: Option<&str>,
    flags: c_ulong,
    data: Option<&str>,
) -> io::Result<()> {    let src_c = cstr(src.as_os_str())?;
    let tgt_c = cstr(target.as_os_str())?;
    let fst_c = match fstype {
        Some(s) => Some(CString::new(s).map_err(bad)?),
        None => None,
    };
    let dat_c = match data {
        Some(s) => Some(CString::new(s).map_err(bad)?),
        None => None,
    };
    let r = unsafe {
        mount(
            src_c.as_ptr(),
            tgt_c.as_ptr(),
            fst_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            flags,
            dat_c
                .as_ref()
                .map_or(std::ptr::null(), |c| c.as_ptr() as *const c_void),
        )
    };
    if r != 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(
            e.kind(),
            format!("mount {} -> {}: {e}", src.display(), target.display()),
        ));
    }
    Ok(())
}

fn ro_mount(path: &Path) -> io::Result<()> {
    let c = cstr(path.as_os_str())?;
    let attr = MountAttr {
        attr_set: MOUNT_ATTR_RDONLY,
        attr_clr: 0,
        propagation: 0,
        userns_fd: 0,
    };
    let r = unsafe {
        mount_setattr(
            AT_FDCWD,
            c.as_ptr(),
            0,
            &attr as *const MountAttr,
            std::mem::size_of::<MountAttr>(),
        )
    };
    if r != 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(
            e.kind(),
            format!("mount_setattr(ro) {}: {e}", path.display()),
        ));
    }
    Ok(())
}

fn exec(cmd: &[String]) -> ! {
    let c: Vec<CString> = cmd
        .iter()
        .map(|s| CString::new(s.as_str()).expect("arg contains NUL"))
        .collect();
    let mut ptrs: Vec<*const c_char> = c.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe { execvp(c[0].as_ptr(), ptrs.as_ptr()) };
    eprintln!("gitjail: exec {}: {}", cmd[0], io::Error::last_os_error());
    unsafe { _exit(127) }
}

// ---- 会话结束后：把 upper 层翻译成人能看的变更列表 ----------------------

fn report(base: &Path, project: &Path) -> io::Result<()> {
    let upper = base.join("upper");
    let mut rows: Vec<(char, String)> = Vec::new();
    walk(&upper, &upper, project, &mut rows)?;
    rows.sort_by(|a, b| a.1.cmp(&b.1));

    println!();
    println!("session {}", base.file_name().unwrap().to_string_lossy());
    if rows.is_empty() {
        println!("  (no changes)");
    }
    for (k, rel) in rows {
        println!("  {k} {rel}");
    }
    println!("  upper: {}", upper.display());
    Ok(())
}

fn walk(root: &Path, dir: &Path, project: &Path, out: &mut Vec<(char, String)>) -> io::Result<()> {
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
            out.push(('D', rel)); // whiteout
        } else if ft.is_dir() {
            walk(root, &path, project, out)?;
        } else if ft.is_symlink() {
            out.push(('L', rel));
        } else if ft.is_file() {
            let kind = if project.join(&rel).exists() { 'M' } else { 'A' };
            out.push((kind, rel));
        }
    }
    Ok(())
}

// ---- 杂项 ----------------------------------------------------------------

fn runtime_base() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") {
        if !x.is_empty() {
            return PathBuf::from(x).join("gitjail");
        }
    }
    PathBuf::from(format!("/tmp/gitjail-{}", unsafe { getuid() }))
}

fn session_id() -> String {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), ns)
}

fn exit_code(status: i32) -> i32 {
    if wifexited(status) {
        wexitstatus(status)
    } else if wifsignaled(status) {
        128 + wtermsig(status)
    } else {
        1
    }
}

fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(bad)
}

fn bad<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
}
