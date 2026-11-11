// spike: fanotify 能否拿到 pidfd，并据此判断写入者所在 mount namespace
// 编译: gcc -O2 -o fanotify_test fanotify_test.c
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/fanotify.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static int init_try(const char *name, unsigned int flags) {
    int fd = fanotify_init(flags, O_RDONLY | O_CLOEXEC);
    if (fd < 0) {
        printf("  %-14s init 失败: %s\n", name, strerror(errno));
        return -1;
    }
    printf("  %-14s init OK\n", name);
    return fd;
}

static int mark(int fd, const char *dir, const char *how) {
    unsigned long long mask = FAN_CREATE | FAN_DELETE | FAN_MODIFY | FAN_CLOSE_WRITE;
    unsigned int flag = FAN_MARK_ADD;
    if (!strcmp(how, "filesystem")) flag |= FAN_MARK_FILESYSTEM;
    if (fanotify_mark(fd, flag, mask, AT_FDCWD, dir) < 0) {
        printf("  mark[%s] 失败: %s\n", how, strerror(errno));
        return -1;
    }
    printf("  mark[%s] OK\n", how);
    return 0;
}

static int pid_from_pidfd(int pfd) {
    char p[64], line[256];
    snprintf(p, sizeof p, "/proc/self/fdinfo/%d", pfd);
    FILE *f = fopen(p, "r");
    if (!f) return -1;
    int pid = -1;
    while (fgets(line, sizeof line, f))
        if (!strncmp(line, "Pid:", 4)) pid = atoi(line + 4);
    fclose(f);
    return pid;
}

static void show(const char *tag, int pid, int pfd) {
    char path[64], ns[PATH_MAX];
    if (pfd >= 0) pid = pid_from_pidfd(pfd);
    if (pid <= 0) { printf("  %-10s pid 未知\n", tag); return; }
    snprintf(path, sizeof path, "/proc/%d/ns/mnt", pid);
    ssize_t n = readlink(path, ns, sizeof ns - 1);
    if (n < 0) { printf("  %-10s pid=%d (ns 读取失败)\n", tag, pid); return; }
    ns[n] = 0;
    printf("  %-10s pid=%-6d pidfd=%-3d mnt_ns=%s\n", tag, pid, pfd, strrchr(ns, '[') ? ns : ns);
}

int main(int argc, char **argv) {
    const char *dir = argc > 1 ? argv[1] : "/tmp/opencode/spike/fo";
    mkdir(dir, 0755);

    char self[PATH_MAX];
    ssize_t n = readlink("/proc/self/ns/mnt", self, sizeof self - 1);
    if (n > 0) { self[n] = 0; printf("本进程 mnt_ns = %s\n", self); }

    puts("\n[1] 尝试各种 init 组合");
    int fd = init_try("PIDFD+FID", FAN_CLASS_NOTIF | FAN_REPORT_PIDFD | FAN_REPORT_FID | FAN_CLOEXEC);
    int mode = 0;
    if (fd < 0)
        fd = init_try("PIDFD", FAN_CLASS_NOTIF | FAN_REPORT_PIDFD | FAN_CLOEXEC), mode = 1;
    else
        mode = 0;
    if (fd < 0)
        fd = init_try("TID", FAN_CLASS_NOTIF | FAN_REPORT_TID | FAN_CLOEXEC), mode = 2;
    if (fd < 0)
        fd = init_try("PLAIN", FAN_CLASS_NOTIF | FAN_CLOEXEC), mode = 3;
    if (fd < 0) { puts("\nfanotify 不可用（可能需要 CAP_SYS_ADMIN）"); return 2; }

    puts("\n[2] 尝试 mark");
    if (mark(fd, dir, "filesystem") < 0) mark(fd, dir, "inode");

    puts("\n[3] 子进程写入 + 删除，父进程收事件");
    pid_t child = fork();
    if (child == 0) {
        char p[512];
        snprintf(p, sizeof p, "%s/hello.txt", dir);
        int f = open(p, O_CREAT | O_WRONLY | O_TRUNC, 0644);
        if (f >= 0) { write(f, "x", 1); close(f); }
        unlink(p);
        _exit(0);
    }
    waitpid(child, NULL, 0);

    char buf[8192];
    ssize_t len = read(fd, buf, sizeof buf);
    if (len <= 0) { printf("  没读到事件: %s\n", strerror(errno)); return 3; }

    struct fanotify_event_metadata *m = (void *)buf;
    while (FAN_EVENT_OK(m, len)) {
        const char *what = (m->mask & FAN_CREATE) ? "CREATE" :
                           (m->mask & FAN_DELETE) ? "DELETE" :
                           (m->mask & FAN_CLOSE_WRITE) ? "CLOSE_WRITE" : "OTHER";

        // pidfd 在 info record 里，不在 metadata.fd
        int pidfd = -1;
        char *p = (char *)m + m->metadata_len;
        char *end = (char *)m + m->event_len;
        while (p + sizeof(struct fanotify_event_info_header) <= end) {
            struct fanotify_event_info_header *h = (void *)p;
            if (h->len < sizeof(*h) || p + h->len > end) break;
            if (h->info_type == FAN_EVENT_INFO_TYPE_PIDFD) {
                struct fanotify_event_info_pidfd *pd = (void *)p;
                pidfd = pd->pidfd;
            }
            p += h->len;
        }

        if (pidfd >= 0)      show(what, -1, pidfd);
        else if (m->pid > 0) show(what, m->pid, -1);
        else                 printf("  %-10s pid 未知 (metadata.fd=%d)\n", what, m->fd);
        m = FAN_EVENT_NEXT(m, len);
    }
    return 0;
}
