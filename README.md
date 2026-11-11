# gitjail

> **不是识别 agent 然后阻止它，而是给它一个只能写 upper 的世界。**

OS 层的写屏障。任何 agent —— Codex、OpenClaw、明天的无名 agent、它派生出来的
`curl | sh` 脚本 —— 在你**声明过**的目录里都只能通过 broker 写入；每一次变更都是
一个可回滚的 commit。

不需要改 agent 一行代码，也不需要它知道 git 存在。

---

## 名字

`git` + `jail`：把 agent 关进一个只能用 git 落地的笼子。

笼子里的路径看起来和外面一模一样，但它写的每一个字节都先进入暂存层，
由 broker 提交之后才落到真实目录。agent 看不见笼子的边界，也不需要看见。

---

## 三条不变量

1. **不识别 agent。** 没有身份检查，没有特征匹配，没有进程名黑名单参与决策。
2. **只安置进程。** `gitjail run` 把它放进一个新的 mount namespace，
   那个 namespace 里真实目录是不可写的。之后内核只查挂载表。
3. **只保护声明过的目录。** 默认一个都不保护。Obsidian、微信、浏览器、
   `~/.cache`、`/tmp` 全部照旧。

## 一个例子

```bash
$ gitjail run --project ~/code/proj -- codex

session 138001-1794406523599844481
  A new.txt
  D old.txt
  M src/main.rs

  next: gitjail apply 138001-1794406523599844481
```

此时**真实目录还没变**。agent 的所有改动都在 `upper/` 里，等你看过再决定：

```bash
$ gitjail log
ID                       STATUS      CHANGES  PROJECT
138001-179440652359984  pending           3  /home/niko/code/proj

$ gitjail apply 138001-179440652359984
applied 138001-179440652359984

$ gitjail undo 138001-179440652359984
undone 138001-179440652359984
```

`apply` 之前会先保存 pre-image，所以 `undo` 能精确还原——包括被删掉的文件。

如果文件在会话结束后被你手动改过，`apply` 会整体拒绝，一个字节都不动；
确认要覆盖时加 `--force`。

## 快速开始

需要：Linux 5.12+（`mount_setattr`）、开启非特权 user namespace、**不要用 root 跑**。

```bash
cargo build --release

# 1. 在沙箱里跑
./target/release/gitjail run --project ~/code/proj -- bash

# 2. 看会话
./target/release/gitjail log

# 3. 落到真实目录 / 撤销
./target/release/gitjail apply
./target/release/gitjail undo
```

沙箱内部：PID namespace 里只有沙箱自己的进程，`/proc/<host-pid>/root` 不可见。

## 开发

```bash
cargo build --release
./target/release/gitjail run --project ~/code/proj -- bash
```

依赖只有 `libc`、`clap`、`anyhow`——没有理由为了"零依赖"去手写 syscall 封装。

如果 `cargo` 卡在 `static.crates.io`（国内常见，索引能通但下载超时），配置镜像：

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
```

## 不做什么

- 不是 agent 的插件，不是某个 agent 的功能
- 不监控整个文件系统（只监听声明过的目录）
- 不拦截网络、数据库、进程内存
- 不假装能约束 IDE 插件（见 [docs/non-goals.md](docs/non-goals.md)）

## 文档

| 文档 | 内容 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 安置 vs 识别、双平面、保护区模型 |
| [docs/policy.md](docs/policy.md) | 路径 / 操作 / 进程三层策略，敏感文件夹怎么定义 |
| [docs/non-goals.md](docs/non-goals.md) | 已知边界，以及为什么它们是边界 |
| [docs/faq.md](docs/faq.md) | inode 是什么、会不会拦住 Obsidian、IDE 插件怎么办 |
| [docs/spike.md](docs/spike.md) | 四个关键假设的实测结果 |

## 状态

- `run`：沙箱（userns + RO bind + overlay + pidns）✅
- `log` / `apply` / `undo`：基于 pre-image 的精确回滚 ✅
- git 提交历史：未接入（数据模型已按 pre/post 树设计）
- `protect` / `audit` / daemon：未实现

## License

MIT（见 [LICENSE](LICENSE)）
