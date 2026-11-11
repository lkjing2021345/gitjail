# 策略模型

策略分三层，**层的强度不同，不要混用**。

| 层 | 管什么 | 强度 | 实现 |
|---|---|---|---|
| **路径层** | 哪些目录受保护 | 强（安全边界） | mount namespace + inode 标记 |
| **操作层** | 允许写 / 允许删 / 允许执行 | 强（安全边界） | overlay 语义 + Landlock |
| **进程层** | 哪个程序能干什么 | 弱（策略/便利） | 启动时分流 + 审计标签 |

---

## 1. 敏感文件夹到底怎么定义

### 1.1 结论：定义 = 声明 + 标记 + 锁死

**不判断，只声明。**

```
gitjail protect ~/code/proj
  1. 解析路径 → 拿到 (st_dev, inode)
  2. 打标： trusted.gitjail.protected=1   （需要 CAP_SYS_ADMIN）
           并记录 mount ID
  3. 写策略： ~/.config/gitjail/policy.toml（属主 root）
  4. 校验： 在 sandbox 内读不到、删不掉、改不了
```

三个要素缺一不可：

| 要素 | 防的是什么 |
|---|---|
| **声明** | 防「猜错」：不推断，所以不会误判 |
| **inode 标记** | 防「路径逃逸」：mv / 硬链接 / symlink 都跟着 inode 走 |
| **不可自解除** | 防「agent 自己 unprotect」 |

### 1.2 为什么要 inode 而不是路径

路径匹配有五个已知逃逸口（见 architecture.md §4.2）。inode 匹配全部封死：

```bash
mv ~/code/proj /tmp/x        # inode 不变 → 仍然受保护
ln ~/code/proj/a /tmp/b      # 同一 inode → 仍然受保护
ln -s ~/code/proj /tmp/c     # 解析到真实 inode → 仍然受保护
```

只有**复制**出去不受保护。复制本来就不该拦——那是新数据。

### 1.3 谁能解除保护

```bash
# ✅ 用户，在沙箱外
$ gitjail unprotect ~/code/proj
⚠ 将有 1,842 个文件失去保护，且 agent 可立即删除它们。
  输入目录名确认: code/proj

# ❌ agent，在沙箱内
$ gitjail unprotect ~/code/proj
gitjail: 该操作必须在沙箱外执行
[audit] unprotect_attempt pid=8123 ns=agent-codex-42
```

### 1.4 「敏感」是三种不同的东西

| 类型 | 例子 | 机制 | 默认 |
|---|---|---|---|
| **完整性** | 源码、论文、配置 | overlay + 快照 + 回滚 | ✅ 开 |
| **机密性** | `~/.ssh`、`~/.aws`、`.env` | namespace 遮蔽（空 tmpfs） | 关，`--hide` 开 |
| **可携带性** | 密钥、客户数据 | 文件层做不到，只能靠网络策略 | ❌ 非目标 |

**默认只做完整性。** 因为绝大多数人说「敏感文件夹」时，想的是「别把我东西删了」，
不是「别让它看见」。

机密性用遮蔽，不用「禁止读」：

```bash
gitjail run --hide ~/.ssh --hide ~/.aws -- codex
```

效果是 agent 眼里根本没有这个目录，比任何权限规则都彻底。

---

## 2. 进程黑白名单

> ⚠️ **先读这一节再决定要不要用。这一层不是安全边界。**

### 2.1 为什么它不是安全边界

三个无法修复的弱点：

| 弱点 | 例子 |
|---|---|
| **名字可伪造** | `cp /bin/rm ./codex && ./codex`；`exec -a git rm -rf x` |
| **参数可绕过** | 允许 `git` = 允许 `git checkout <恶意 commit>` = 任意写 |
| **confused deputy** | 允许 `git` 写，agent 就把 git 当写通道 |

更根本的：**agent 有十种方式发起同一个系统调用**。

```bash
rm -rf x
bash -c "rm -rf x"
python -c "import shutil; shutil.rmtree('x')"
perl -e 'unlink ...'
```

禁掉 `rm` 这个进程，等于什么都没禁。

### 2.2 所以正确的分工

| 你想拦的 | 该用哪层 |
|---|---|
| 「不许删文件」 | **操作层**：Landlock 不放 `REMOVE_FILE` / `REMOVE_DIR` |
| 「不许写这个目录」 | **路径层**：不放进 overlay 的可写集合 |
| 「不许看这个目录」 | **路径层**：`--hide` 遮蔽 |
| 「不许跑 `shred` 这类工具」 | 进程层（便利，不防对抗） |
| 「出了问题能定位是谁」 | cgroup 标签（审计，不做决策） |

**进程名单的价值在「减少误操作」和「审计标注」，不在「安全」。**

### 2.3 但既然要，就做成可用的

```toml
[process.deny]
# 禁止在沙箱内执行（直接 exec 失败）
exec = ["shred", "wipefs", "dd"]

[process.deny_write]
# 允许执行，但拿不到受保护目录的写视图
exec = ["ffmpeg", "convert"]

[process.allow_write]
# ⚠️ 危险：这些程序获得真实树的可写视图
# 只在明确知道 confused deputy 风险时使用
exec = []
deny_subcommands = ["git reset --hard", "git clean -fdx", "git checkout -- ."]
```

`deny_subcommands` 才是真正有用的部分：agent 的实际操作面是**子命令**，
不是二进制名。

### 2.4 白名单模式（默认关闭）

```toml
[process.whitelist_mode]
enabled = false            # 打开后只有列表内的程序能写
allow = ["git", "cargo", "npm"]
```

打开后行为：未列出的程序写受保护目录时得到 `EPERM`。

**必须打印警告**：

```
⚠ 白名单模式已启用
  被允许的程序会成为代理写通道（confused deputy）
  例如 git 可被用来写入任意内容
  这不是安全边界，只是操作约束
```

---

## 3. 操作层策略（这才是安全边界）

```toml
[operations]
delete      = "record"     # record | deny
truncate    = "record"     # record | deny
chmod       = "record"
rename      = "record"
create      = "allow"
write       = "allow"
```

- `record`：操作成功，但立即产生一次 commit，可 undo
- `deny`：操作返回 `EPERM`（Landlock 不授予对应 access）

`delete = "deny"` 的实现：

```
Landlock ruleset:
  grant  WRITE_FILE | MAKE_FILE | MAKE_DIR | REFER
  NOT grant REMOVE_FILE | REMOVE_DIR      ← 这一条让 unlink 失败
```

**建议默认 `record`。** 理由：99% 的事故是手滑，`record + undo` 的价值和
`deny` 相同，但对 agent 友好得多（它不会在报错后进入奇怪状态）。
面对 prompt injection 这类对抗场景，再打开 `deny`。

---

## 4. 默认值

| 项 | 默认 | 说明 |
|---|---|---|
| 受保护目录 | **空** | 零保护，必须显式声明 |
| agent 可写区域 | `~/.cache` `~/.npm` `~/.cargo` `/tmp` `**/target` `**/node_modules` | 否则 `npm install` 直接崩 |
| 遮蔽目录 | 空 | 建议 `--hide ~/.ssh`，但不强制 |
| 删除 | `record` | 可撤销，不阻断 |
| 进程名单 | 空 | 默认不用 |
| 白名单模式 | 关 | |

---

## 5. 完整示例

```toml
# ~/.config/gitjail/policy.toml  (root:root, 0644)

[protect]
paths = ["~/code/**", "~/Documents/thesis"]

[hide]
paths = ["~/.ssh", "~/.aws", "~/.config/gh"]

[agent.allow]
paths = ["~/.cache", "~/.npm", "~/.cargo", "/tmp", "**/target", "**/node_modules"]

[operations]
delete   = "record"
truncate = "record"

[process.deny]
exec = ["shred", "wipefs"]

[audit]
watch_unconfined = true      # 真实树上出现非受限进程的写 → 报警
ignore = ["**/target/**", "**/node_modules/**", "**/.git/objects/**"]
```

---

## 6. 三层如何组合

```
        路径在保护区?
          │否 → 放行
          │是
          ▼
        进程在受限命名空间?
          │否 → 放行（可 audit）
          │是
          ▼
        操作类型?
          ├─ write  → 写 upper → commit
          ├─ delete → record 或 deny（按策略）
          └─ exec   → 过进程名单（弱策略，仅便利）
```

安全由前两层保证。第三层是锦上添花，写文档时要诚实标注。
