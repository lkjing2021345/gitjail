# Spike：四个假设的验证

**环境**：Arch Linux，kernel 7.2.14，Landlock ABI 10，bwrap 可用，**普通用户（无 root）**

| # | 假设 | 结果 |
|---|---|---|
| 1 | 无 root 的 userns + RO bind 能挡住写 | ✅ EROFS |
| 2 | userns 里能挂 overlay | ✅ 不需要 `userxattr` |
| 3 | Landlock 能「允许写、拒绝删」 | ✅ unlink → EACCES |
| 4 | fanotify 能拿 pidfd 做归属 | ⚠️ 需要 `CAP_SYS_ADMIN` |

两大修正：**必须带 PID namespace**、**daemon 必须特权**。见 §5、§4。

---

## 1. userns + RO bind

```bash
unshare -Urm --propagation private sh -c '
  mount --bind /tmp/opencode/spike/real /tmp/opencode/spike/real
  mount -o remount,bind,ro /tmp/opencode/spike/real
  touch /tmp/opencode/spike/real/new.txt
'
```

```
touch: cannot touch '.../new.txt': Read-only file system
```

✅ 成立。读取正常，写入 `EROFS`，全程无 root。

---

## 2. userns 里的 overlay

```bash
unshare -Urm sh -c '
  mount -t overlay overlay \
    -o lowerdir=lower,upperdir=upper,workdir=work merged
'
```

```
create ok
copy-up ok
delete ok (whiteout)
```

✅ 成立，**不需要 `userxattr`**（kernel 7.2）。

删除后的 upper 层：

```
c--------- 2 niko niko 0, 0 a.txt     ← whiteout，字符设备 0:0
-rw-r--r-- 1 niko niko    3 b.txt
```

这验证了之前的判断：**overlay 下 `rm` 会成功，用 whiteout 表达删除**，
所以「阻止删除」不能靠 overlay，得靠 Landlock。

---

## 3. Landlock：允许写、拒绝删

代码：[`spike/landlock_test.c`](../spike/landlock_test.c)

规则：handled 包含 `WRITE_FILE | REMOVE_FILE | REMOVE_DIR | MAKE_* | REFER | TRUNCATE`，
但只对目标目录授予 `WRITE_FILE | MAKE_* | TRUNCATE`，**不授予 `REMOVE_*`**。

```
Landlock ABI = 10
规则已生效

  写已有文件 (truncate+write)   OK
  新建文件                       OK
  在子目录新建文件               OK
  新建目录                       OK
  --- 以下应当全部 DENIED ---
  删除文件 b.txt                 DENIED
  删除文件 a.txt                 DENIED
  跨目录 rename                  DENIED
  删除空目录 d1                  DENIED
  在规则外新建文件               DENIED
```

✅ 完全符合设计：**写可以，删不行，越界不行**，且不需要 root。

注意最后一条：规则外的路径默认拒绝（handled 的权限没被授予），
这正是「只允许写 upper」需要的语义。

---

## 4. fanotify：需要特权

代码：[`spike/fanotify_test.c`](../spike/fanotify_test.c)

三种 init 组合在普通用户下全部失败：

```
PIDFD+FID      init 失败: Operation not permitted
PIDFD          init 失败: Operation not permitted
TID            init 失败: Operation not permitted
PLAIN          init 失败: Operation not permitted
```

在 `unshare -Ur`（userns 内有 `CAP_SYS_ADMIN`）里**同样失败** ——
因为 fanotify 的检查针对 **init user namespace**。

### 设计影响

```
gitjaild 必须以 root（或 CAP_SYS_ADMIN）运行
CLI 不能自己监听，必须通过 unix socket 问 daemon
```

这不是障碍，但意味着安装流程里有一个特权组件，需要单独审计。

在有 root 的机器上验证：

```bash
sudo ./spike/fanotify_test
# 期望看到 pidfd → /proc/<pid>/ns/mnt
```

> **实现细节**：pidfd 不在 `fanotify_event_metadata.fd` 里，而是在
> `FAN_EVENT_INFO_TYPE_PIDFD` 类型的 info record 里（`metadata_len` 之后）。
> 第一版解析错了，导致打印 "pid 未知"，已修正。

---

## 5. 意外发现：`/proc/<pid>/root` 是逃生口

只做 mount namespace、不做 PID namespace 时：

```bash
unshare -Urm sh -c '
  touch /tmp/.../real/direct            # EROFS ✅
  touch /proc/<hostpid>/root/tmp/.../real/bypass   # ← 这里
'
```

`/proc/<pid>/root` 是按**目标进程的 mount namespace** 解析的，所以它能绕过你的 RO bind。

本机测试被 `kernel.yama.ptrace_scope=1` 挡住了（`Permission denied`），
但**不能依赖 yama**：把它设为 0 的发行版和容器里，这就是一条现成的逃逸路径。

### 修复

必须加 PID namespace：

```bash
unshare -Urmp --fork --mount-proc ...
```

验证：沙箱内看不到 host pid。

```
看 host pid 121255: 不可见(好)
```

---

## 6. 端到端 demo

[`spike/demo.sh`](../spike/demo.sh) 把 1 + 2 + 5 串起来：

```
=== 沙箱内（agent 的世界） ===
  能读 lower: v1
  写 main.c:  OK
  删 keep.c:  OK
  建 new.txt: OK
  写真实路径: Read-only file system        ← 挡住
  mv 出视图:  inter-device move failed     ← 挡住

=== 沙箱外：真实目录 ===
main.c 内容: v1                            ← 完全没变

=== upper 层 ===
c src/keep.c     ← whiteout
f src/main.c     ← 修改
f new.txt        ← 新建
```

---

## 7. 结论

架构成立。两处必须写进实现：

1. **`gitjail run` 必须创建 PID namespace**，否则 `/proc/<pid>/root` 是逃生口
2. **`gitjaild` 必须特权运行**，fanotify 在 userns 里不可用

其余四个假设全部通过，且不需要 root。
