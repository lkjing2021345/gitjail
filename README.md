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
$ gitjail run -- codex

# codex 在笼子里：
agent$ rm -rf src            # 记录，可撤销
agent$ echo hi > src/a.txt   # 落到 upper，随后成为一次 commit

# 你在外面：
$ gitjail log
c3f1  codex  14:02:11  write   src/a.txt
c3f2  codex  14:02:19  delete  src/old.rs
c3f3  codex  14:02:31  bash    rm -rf tests/fixtures  (3 files)

$ gitjail undo c3f3
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

设计阶段。代码未开始。

## License

MIT（见 [LICENSE](LICENSE)）
