# FAQ

## inode 是什么？

文件系统给「一个文件」发的身份证号。

名字和它是两回事：**目录**里存的是「名字 → inode 号」的映射，inode 里存的是
权限、属主、大小、时间戳、数据块指针——**唯独没有文件名**。

| 现实 | 文件系统 |
|---|---|
| 身份证号 | inode 号 |
| 本人 | inode 里的元数据 + 数据块 |
| 家庭住址 | 文件名 / 路径 |
| 搬家 | `mv`（换地址，人没变） |
| 登记第二个住址 | `ln` 硬链接（一个 inode，两个名字） |
| 克隆一个人 | `cp`（新 inode） |
| 「去 XX 地址找人」的纸条 | `ln -s` 符号链接 |

```bash
$ stat -c '%d:%i  %n' a.txt
2050:1234567  a.txt

$ mv a.txt c.txt
$ stat -c '%d:%i  %n' c.txt
2050:1234567  c.txt      # inode 没变

$ ln c.txt /tmp/b.txt
$ stat -c '%d:%i  %n' /tmp/b.txt
2050:1234567  /tmp/b.txt # 同一个 inode

$ cp c.txt /tmp/d.txt
$ stat -c '%d:%i  %n' /tmp/d.txt
2050:9876543  /tmp/d.txt # 新 inode
```

**为什么 gitjail 必须用它：** 安全规则如果认路径，`mv` 一下就逃出去了；
认 inode，搬家、硬链接、symlink 都跟着走。

详见 [architecture.md §4](architecture.md)。

---

## 会不会把我 Obsidian 的笔记拦住？

不会。默认**零保护**。

只有你显式 `gitjail protect` 过的目录才受保护，而且只有 `gitjail run` 启动的
进程树在里面受限制。

| 场景 | 结果 |
|---|---|
| Obsidian 写 `~/Obsidian Vault` | ✅ 正常（没声明保护） |
| Obsidian 写受保护目录里的文件 | ✅ 正常（它是用户进程，不是 agent） |
| 微信写 `~/xwechat_files` | ✅ 正常（没声明保护） |
| codex 写受保护目录 | 📦 进 upper，commit |

---

## 会不会把我系统搞坏？

不会。**真实目录的权限位一个都不改。**

限制只存在于 agent 的 mount namespace 里。broker 崩了，坏的只是那个 agent 会话；
你的桌面、编辑器、文件管理器完全不受影响。

这也是为什么不用 `chmod -R a-w` 或 ACL——那才是会搞坏系统的做法。

---

## 为什么不能直接识别 agent？

因为它是黑名单，必然漏：

- 新 agent 每天都在出现
- OpenClaw 这类是 daemon，不在你的 shell 下
- agent 派生的 `npm install` 子进程没有可查的标记
- prompt injection 改个环境变量就绕过

所以 gitjail 不回答「你是不是 agent」，而是回答「你在哪个 namespace 里」。
后者不可伪造。

---

## agent 需要跑 `npm install` / `cargo build` 怎么办？

不受影响。`~/.cache`、`~/.npm`、`~/.cargo`、`/tmp`、`**/target`、
`**/node_modules` 默认都不在保护区，agent 可以正常写。

只有你显式保护的目录才受限。

---

## 它和 Codex / Cursor 自带的 checkpoint 有什么区别？

| | 自带 checkpoint | gitjail |
|---|---|---|
| 覆盖范围 | 只有它自己的编辑工具 | 进程树里的一切，包括 shell |
| `rm -rf` | 多数抓不到或事后才发现 | 记录并可撤销 |
| `git reset --hard` | 不管 `.git` | 连 `.git` 一起快照 |
| 换 agent | 每家一套 | 一套策略，所有 agent |
| 绕过方式 | 用 shell 就行 | 要逃出 namespace |

---

## 支持 macOS / Windows 吗？

第一版只做 Linux。

- macOS：EndpointSecurity 需要 Apple 特批 entitlement，个人开发者基本拿不到；
  退路是 `sandbox-exec` + APFS local snapshot，强度低不少
- Windows：ProjFS 或签名 minifilter 驱动，工作量另算

**不要一开始就三平台。** 先把 Linux 的语义做对。

---

## 为什么叫 gitjail？

`git` + `jail`：把 agent 关进一个只能用 git 落地的笼子。

笼子里的路径看起来和外面一样，但它写的每个字节都先进入暂存层，
由 broker 提交之后才落到真实目录。
