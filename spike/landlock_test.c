// spike: Landlock 能否「允许写、拒绝删」
// 编译: gcc -O2 -o landlock_test landlock_test.c
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <linux/landlock.h>

#ifndef __NR_landlock_create_ruleset
#define __NR_landlock_create_ruleset 444
#define __NR_landlock_add_rule       445
#define __NR_landlock_restrict_self  446
#endif

static int ll_create(const struct landlock_ruleset_attr *a, size_t sz, __u32 f) {
    return syscall(__NR_landlock_create_ruleset, a, sz, f);
}
static int ll_add(int fd, enum landlock_rule_type t, const void *a, __u32 f) {
    return syscall(__NR_landlock_add_rule, fd, t, a, f);
}
static int ll_restrict(int fd, __u32 f) {
    return syscall(__NR_landlock_restrict_self, fd, f);
}

static void check(const char *what, int ok) {
    printf("  %-34s %s\n", what, ok ? "OK" : "DENIED");
}

static void mkdirs(const char *p) {
    char buf[512];
    snprintf(buf, sizeof buf, "%s", p);
    for (char *s = buf + 1; *s; s++) {
        if (*s == '/') { *s = 0; mkdir(buf, 0755); *s = '/'; }
    }
    mkdir(buf, 0755);
}

int main(int argc, char **argv) {
    const char *root = argc > 1 ? argv[1] : "/tmp/opencode/spike/ll/root";
    char path[512], path2[512];

    mkdirs(root);
    snprintf(path, sizeof path, "%s/sub1", root); mkdir(path, 0755);
    snprintf(path, sizeof path, "%s/sub2", root); mkdir(path, 0755);
    snprintf(path, sizeof path, "%s/a.txt", root);
    int f = open(path, O_CREAT | O_WRONLY, 0644);
    if (f >= 0) { write(f, "hello", 5); close(f); }

    int abi = ll_create(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
    printf("Landlock ABI = %d\n", abi);
    if (abi < 1) { puts("Landlock 不可用"); return 1; }

    __u64 handled = LANDLOCK_ACCESS_FS_WRITE_FILE
                  | LANDLOCK_ACCESS_FS_REMOVE_FILE
                  | LANDLOCK_ACCESS_FS_REMOVE_DIR
                  | LANDLOCK_ACCESS_FS_MAKE_REG
                  | LANDLOCK_ACCESS_FS_MAKE_DIR
                  | LANDLOCK_ACCESS_FS_MAKE_SYM;
    __u64 allowed = LANDLOCK_ACCESS_FS_WRITE_FILE
                  | LANDLOCK_ACCESS_FS_MAKE_REG
                  | LANDLOCK_ACCESS_FS_MAKE_DIR
                  | LANDLOCK_ACCESS_FS_MAKE_SYM;
    if (abi >= 2) handled |= LANDLOCK_ACCESS_FS_REFER;
    if (abi >= 3) { handled |= LANDLOCK_ACCESS_FS_TRUNCATE; allowed |= LANDLOCK_ACCESS_FS_TRUNCATE; }

    struct landlock_ruleset_attr attr = { .handled_access_fs = handled };
    int rfd = ll_create(&attr, sizeof attr, 0);
    if (rfd < 0) { perror("landlock_create_ruleset"); return 1; }

    int pfd = open(root, O_PATH | O_CLOEXEC);
    struct landlock_path_beneath_attr pb = { .allowed_access = allowed, .parent_fd = pfd };
    if (ll_add(rfd, LANDLOCK_RULE_PATH_BENEATH, &pb, 0) < 0) { perror("landlock_add_rule"); return 1; }

    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0) { perror("no_new_privs"); return 1; }
    if (ll_restrict(rfd, 0) < 0) { perror("landlock_restrict_self"); return 1; }
    puts("规则已生效\n");

    snprintf(path,  sizeof path,  "%s/a.txt", root);
    snprintf(path2, sizeof path2, "%s/b.txt", root);

    int w = open(path, O_WRONLY | O_TRUNC);
    check("写已有文件 (truncate+write)", w >= 0); if (w >= 0) close(w);

    int c = open(path2, O_CREAT | O_WRONLY, 0644);
    check("新建文件", c >= 0); if (c >= 0) close(c);

    snprintf(path, sizeof path, "%s/sub1/x.txt", root);
    int c2 = open(path, O_CREAT | O_WRONLY, 0644);
    check("在子目录新建文件", c2 >= 0); if (c2 >= 0) close(c2);

    snprintf(path, sizeof path, "%s/newdir", root);
    check("新建目录", mkdir(path, 0755) == 0);

    printf("  --- 以下应当全部 DENIED ---\n");
    check("删除文件 b.txt", unlink(path2) == 0);
    snprintf(path, sizeof path, "%s/a.txt", root);
    check("删除文件 a.txt", unlink(path) == 0);

    snprintf(path,  sizeof path,  "%s/b.txt", root);
    snprintf(path2, sizeof path2, "%s/sub1/b.txt", root);
    check("跨目录 rename", rename(path, path2) == 0);

    snprintf(path, sizeof path, "%s/d1", root);
    mkdir(path, 0755);
    check("删除空目录 d1", rmdir(path) == 0);

    snprintf(path, sizeof path, "/tmp/opencode/spike/ll/outside.txt");
    int o = open(path, O_CREAT | O_WRONLY, 0644);
    check("在规则外新建文件", o >= 0); if (o >= 0) close(o);

    return 0;
}
