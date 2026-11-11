# 架构

## 0. 一句话

不是「识别 agent 然后阻止它」，而是「给它一个只能写 upper 的世界」。

---

## 1. 为什么不能识别 agent

黑名单有四种死法：

| 死因 | 说明 |
|---|---|
| 名单永远追不上 | 今天 codex、openclaw，下个月又一个 |
| daemon 型抓不到 | OpenClaw 是常驻服务，不在你的 shell 下 |
| 子进程无标记 | agent 跑 `npm install`，postinstall 脚本跟它没有血缘可查 |
| 对抗场景失效 | prompt injection 改个环境变量就绕过 |

所以「是不是 agent」这个问题不该被回答。它应该被**取消**。

---

## 2. 安置，而不是识别

### 2.1 拦截发生在启动时

```bash
gitjail run -- codex

# 内部只做三步，然后 exec：
unshare(CLONE_NEWNS | CLONE_NEWUSER)        # 1. 新挂载命名空间
mount --bind -o ro / /                      # 2. 根只读
mount -t overlay ... /home/niko/code/proj   # 3. 目标路径换成 overlay
exec codex
```

**之后没有任何检查。** 内核只是查挂载表：这个进程的 namespace 里
`/home/niko/code/proj` 指向 overlay，于是写进 upper。

内核不知道调用者是 codex、bash，还是 `curl | sh` 下来的脚本。

### 2.2 同一路径，两个世界

```
你的桌面（初始 mount namespace）
  /home/niko/code/proj  →  真实树（可写）
  ├─ code        ← 正常
  ├─ obsidian    ← 正常
  └─ wechat      ← 正常

codex 会话（新 mount namespace）
  /home/niko/code/proj  →  overlay 视图（写进 upper）
  └─ codex + 它派生的所有子进程
```

VS Code 不受影响，不是因为它「通过了检查」，而是**它没走进这个 namespace**。

### 2.3 完备性来自继承，不来自穷举

| | 语义识别（否） | 结构识别（是） |
|---|---|---|
| 依据 | 二进制名、env、行为 | 是不是 `gitjail run` 的后代 |
| 新 agent | 规则要更新 | 自动覆盖 |
| 可伪造 | 能 | 不能（改不了父进程） |
| 运行时开销 | 每次写都查 | 零 |

「完备」和「不识别」不矛盾：限制的不是**哪些程序**，而是**哪些进程树**。

### 2.4 为什么逃不掉

- `fork` / `exec` 继承 namespace
- `setns` 回父 namespace 需要目标 userns 的 `CAP_SYS_ADMIN`，它没有
- 最根本的：**真实树的可写路径在它的挂载表里根本不存在**

---

## 3. 双平面

```
强制平面（谁都不能写真实目录）
  gitjail run   → userns + RO-bind + overlay + Landlock
  gitjail svc   → systemd ProtectSystem=strict
  gitjail hard  → 独立 UID + ACL

                    ↓ 写落在 upper

记录平面（与 agent 无关）
  broker 收割 upper → commit（带 cgroup 标注）
                    → 3-way merge 回真实树
```

两个平面解耦是架构上最值钱的决定：**强制平面可以换，记录平面不变。**

### 3.1 为什么用 overlay 而不是 FUSE

overlayfs 在内核态，性能接近原生；FUSE 小文件慢 5–50 倍。
`fuse-overlayfs` 只作为老内核兜底。

**例外：读保护必须用 FUSE**（见 §4.6）。

---

## 4. 保护区模型

### 4.1 不推断，只声明

**没有任何启发式能正确判断「这个文件夹敏不敏感」。**

- `~/code/proj` 是项目
- `~/Documents/thesis` 是三年心血
- `~/.ssh` 是另一种敏感（不能读，不是不能改）

所以：

> **敏感是一个声明，不是一个判断。**

- 默认：**零保护**
- `gitjail protect ~/code/proj` 显式声明
- `gitjail scan` 只**建议**（"这 3 个目录像项目，要保护吗？"），人来确认
- 启发式永远不直接授予保护状态

### 4.2 但「用路径声明」本身有四个洞

如果运行时按路径字符串匹配，下面这些操作都能逃出去：

| 逃逸手法 | 路径匹配 | inode / mount 匹配 |
|---|---|---|
| `mv ~/code/proj /tmp/x` 再改 | ❌ 逃出 glob | ✅ inode 不变 |
| `ln ~/code/proj/a /tmp/b` 硬链接 | ❌ 新路径不在 glob | ✅ 同一 inode |
| `ln -s` 符号链接访问 | ⚠️ 看是否 realpath | ✅ 解析到真实 inode |
| `mount --bind /tmp /home/niko/code/proj` | ❌ 路径对、内容错 | ✅ mount ID 变了 |
| 跨目录 rename | ❌ 可能逃出 | ✅ inode 跟随 |

### 4.3 所以：路径只用于声明，运行时按 inode / mount 匹配

```
声明时：  ~/code/proj  →  解析成 (st_dev, inode) + mount ID
运行时：  内核按 inode 匹配，不看路径字符串
```

这恰好和内核自己的模型一致：Landlock 接收的是目录 fd，fanotify 的 FID 是
file handle。不需要另造一套。

**效果**：文件被 `mv` 到 `/tmp`、被硬链接到别处、被 symlink 指向，
保护都跟着 inode 走。只有**复制**出去才不受保护——而复制本来就不该被拦。

### 4.4 保护状态不能自我解除

这是第二个洞，比第一个更隐蔽。

如果 agent 能执行 `gitjail unprotect ~/code/proj`，前面全部作废。

| 动作 | 谁能做 |
|---|---|
| 读取保护状态 | daemon |
| 设置保护 | 用户，在沙箱外，TTY 确认 |
| 解除保护 | 用户，在沙箱外，TTY 确认 + 打印"将有 N 个文件失去保护" |
| agent 调用 `unprotect` | 直接拒绝，并记一条 audit |

实现上，标记放在 agent 不可写的位置：

- 策略文件属主 root / daemon，`0644`
- 目录上打 `trusted.gitjail.protected` xattr（`trusted.*` 需要 `CAP_SYS_ADMIN`，
  agent 读不到也删不掉）
- 每个保护根是一个独立 bind mount，记录 mount ID（顺带获得 `EBUSY`：
  agent 连 `mv` 整个根都做不到）

三者可以叠加。

### 4.5 保护的继承

新建的文件天然在保护根之下，随 inode 继承，不需要额外打标。
目录被 rename 不影响。目录被删除——那正是我们要拦的操作。

### 4.6 「敏感」其实有三个含义，别混在一起

| 需求 | 例子 | 机制 | 默认 |
|---|---|---|---|
| **完整性**：不能改坏 | 源码、论文、配置 | overlay + broker + 快照 | ✅ |
| **机密性**：不能看到 | `~/.ssh`、`~/.aws`、`.env` | mount namespace **遮蔽** | 可选 |
| **可携带性**：不能带走 | 密钥、客户数据 | 只能靠网络策略，文件层做不到 | ❌ 非目标 |

机密性用**遮蔽**而不是「禁止读」：

```bash
gitjail run --hide ~/.ssh --hide ~/.aws -- codex
# 实现：在 agent 的 namespace 里用空 tmpfs 覆盖这些路径
```

比 FUSE 脱敏简单一个数量级，且不可绕过。FUSE 脱敏（比如 `.env` 只暴露
前 3 行）留到 v2，因为绝大多数场景不需要。

三者混为一谈是这类项目最常见的概念错误。

---

## 5. 三行判断

```python
def can_write(proc, path):
    if path not in PROTECTED:        return ALLOW   # 不在保护区
    if proc not in CONFINED_SESSION: return ALLOW   # 不是受限进程树
    return BROKER_ONLY
```

注意：第三行的 `CONFINED_SESSION` **不是运行时判断**，而是「这个进程在不在
那个 namespace 里」——一个静态事实。见 §2。

| | 未保护路径 | 受保护路径 |
|---|---|---|
| **用户进程** | 随便写 | 随便写（默认） |
| **agent 进程树** | 随便写 | 只能写 upper，必须 commit |

全盘只有右下角一格被限制。

---

## 6. 故障模式

| 故障 | 后果 |
|---|---|
| broker 崩溃 | 只坏当前 agent 会话，你的桌面不受影响 |
| 策略文件损坏 | 降级为「零保护」，不阻塞任何进程 |
| overlay 挂载失败 | agent 启动失败，明确报错，不静默降级 |
| fanotify 队列满 | 丢弃 audit 事件并告警，不影响强制平面 |

**原则：强制平面的失败不能扩散到强制平面之外。**

这也是为什么坚持用 namespace 而不是 `chmod -R a-w`：后者一旦出错，
你自己的编辑器也进不去。

---

## 7. 待定

- 保护根是否强制每个都是独立 bind mount（防 `mv` 根目录）
- inode 复用的兜底：`FS_IOC_GETVERSION` 还是 daemon 侧校验
- 3-way merge 的冲突交互（CLI / TUI / 编辑器）
- 是否支持 Windows（ProjFS）与 macOS（APFS snapshot + sandbox-exec）
