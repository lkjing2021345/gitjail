# spike

验证架构里四个关键假设的最小代码，不是产品代码。

| 文件 | 验证什么 | 需要 root |
|---|---|---|
| `demo.sh` | userns + RO bind + overlay 的完整笼子 | 否 |
| `landlock_test.c` | 允许写、拒绝删、越界拒绝 | 否 |
| `fanotify_test.c` | 拿 pidfd 并判断写入者的 mount namespace | **是** |

## 跑

```bash
# 1 + 2 + 5
./demo.sh

# 3
gcc -O2 -o /tmp/landlock_test landlock_test.c && /tmp/landlock_test

# 4（需要特权）
gcc -O2 -o /tmp/fanotify_test fanotify_test.c
sudo /tmp/fanotify_test
```

结果记录在 [../docs/spike.md](../docs/spike.md)。

## 环境

- Arch Linux，kernel 7.2.14
- Landlock ABI 10
- 普通用户，无 root
