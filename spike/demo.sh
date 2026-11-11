#!/usr/bin/env bash
# spike: userns + RO bind + overlay，最小可运行的"笼子"
set -euo pipefail

ROOT=${1:-/tmp/opencode/spike/demo}
if [ -d "$ROOT" ]; then
  chmod -R u+rwx "$ROOT" 2>/dev/null || true
  rm -rf "$ROOT"
fi
mkdir -p "$ROOT"/{real/src,upper,work,merged}
echo "v1"   > "$ROOT/real/src/main.c"
echo "keep" > "$ROOT/real/src/keep.c"
echo "doc"  > "$ROOT/real/README.md"

echo "=== 沙箱外（你的世界） ==="
find "$ROOT/real" -type f | sed "s|$ROOT|.|" | sort

unshare -Urmp --fork --mount-proc bash -c '
  set -u
  ROOT=$1
  HOSTPID=$2
  mount --bind "$ROOT/real" "$ROOT/real"
  mount -o remount,bind,ro "$ROOT/real"
  mount -t overlay overlay \
        -o lowerdir="$ROOT/real",upperdir="$ROOT/upper",workdir="$ROOT/work" \
        "$ROOT/merged"

  echo
  echo "=== 沙箱内（agent 的世界） ==="
  echo -n "  能读 lower: "; cat "$ROOT/merged/src/main.c"
  echo -n "  写 main.c:  "; (echo modified > "$ROOT/merged/src/main.c" && echo OK) || echo FAIL
  echo -n "  删 keep.c:  "; (rm "$ROOT/merged/src/keep.c" && echo OK) || echo FAIL
  echo -n "  建 new.txt: "; (echo new > "$ROOT/merged/new.txt" && echo OK) || echo FAIL
  echo -n "  写真实路径: "; (touch "$ROOT/real/escape" 2>&1 || true)
  echo -n "  mv 出视图:  "; (mv "$ROOT/merged/README.md" "$ROOT/real/" 2>&1 || true)
  echo -n "  看 host pid $HOSTPID: "; (ls -d /proc/$HOSTPID 2>/dev/null && echo "可见(危险)" || echo "不可见(好)")

  echo
  echo "=== 沙箱内的合并视图 ==="
  find "$ROOT/merged" -type f | sed "s|$ROOT|.|" | sort
' _ "$ROOT" "$$"

echo
echo "=== 沙箱外：真实目录（应完全没变） ==="
find "$ROOT/real" -type f | sed "s|$ROOT|.|" | sort
echo -n "main.c 内容: "; cat "$ROOT/real/src/main.c"

echo
echo "=== upper 层：agent 写了什么 ==="
find "$ROOT/upper" -printf '%y %P\n' | sort
