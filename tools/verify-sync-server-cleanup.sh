#!/usr/bin/env bash
# Verify that the SyncServer test harness leaves no orphan processes.
#
# The failure this guards against is a test run that ends without running the
# test binary's destructors: Ctrl-C, SIGKILL, a panic in a detached thread.
# The server is a real process, so it survives and accumulates, holding its
# data-directory lock and its listening socket.
#
# The check is deliberately violent. It starts a real server through the same
# mechanism the harness uses — its own process group plus
# `PR_SET_PDEATHSIG` — and then kills the *parent* with SIGKILL so no
# destructor can run. Only the kernel-level parent-death backstop can clean
# up after that, which is exactly the guarantee being tested.
#
# Usage: tools/verify-sync-server-cleanup.sh
set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SYNC_SERVER_BIN="${SYNC_SERVER_BIN:-}"
if [ -z "$SYNC_SERVER_BIN" ]; then
    for build in debug release; do
        if [ -x "../syncserver/target/$build/syncserver" ]; then
            SYNC_SERVER_BIN="$(cd -- "../syncserver/target/$build" && pwd)/syncserver"
            break
        fi
    done
fi
if [ -z "$SYNC_SERVER_BIN" ] || [ ! -x "$SYNC_SERVER_BIN" ]; then
    echo "SKIP: no syncserver binary; build the sibling checkout first" >&2
    exit 0
fi

if ! command -v cc >/dev/null 2>&1; then
    echo "SKIP: no C compiler; cannot build the verification helper" >&2
    exit 0
fi

work="$(mktemp -d)"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

count_servers() {
    pgrep -f 'syncserver/target/(debug|release)/syncserver' 2>/dev/null | wc -l
}

# The helper mirrors what `tests/common/sync_server.rs` does: the server leads
# its own process group, and it asks the kernel to kill it when this helper
# dies. The helper then blocks, so it can be killed to simulate an
# interrupted test run.
cat > "$work/parent.c" <<'EOF'
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/prctl.h>
#include <signal.h>

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: parent BIN ROOT PORT\n"); return 2; }
    char port[32];
    snprintf(port, sizeof(port), "%s", argv[3]);

    pid_t pid = fork();
    if (pid < 0) { perror("fork"); return 1; }
    if (pid == 0) {
        /* Own process group: the terminal's SIGINT must not reach the
           server, so only the harness stops it. */
        if (setpgid(0, 0) != 0) { perror("setpgid"); _exit(1); }
        /* Kernel backstop: if the parent dies without running its
           destructors, the kernel kills this child. */
        if (prctl(PR_SET_PDEATHSIG, SIGKILL) != 0) { perror("prctl"); _exit(1); }
        setenv("SYNC_DATA_DIR", argv[2], 1);
        setenv("SYNC_BIND", port, 1);
        setenv("SYNC_PUBLIC_BASE_URL", "", 1);
        execl(argv[1], argv[1], (char *)NULL);
        perror("exec");
        _exit(1);
    }
    printf("%d\n", pid);
    fflush(stdout);
    /* Hold the server open until this process is killed. */
    for (;;) sleep(1);
}
EOF

if ! cc -O2 -o "$work/parent" "$work/parent.c" 2>"$work/build.log"; then
    echo "SKIP: could not build the verification helper" >&2
    cat "$work/build.log" >&2
    exit 0
fi

before="$(count_servers)"
echo "servers running before: $before"

port="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
root="$work/data"
mkdir -p "$root"
chmod 700 "$root"

"$work/parent" "$SYNC_SERVER_BIN" "$root" "$port" > "$work/pid" 2>"$work/err" &
helper_pid=$!

server_pid=""
for _ in $(seq 1 100); do
    server_pid="$(head -1 "$work/pid" 2>/dev/null || true)"
    [ -n "$server_pid" ] && break
    sleep 0.1
done

if [ -z "$server_pid" ] || ! kill -0 "$server_pid" 2>/dev/null; then
    echo "FAIL: the helper did not start a server" >&2
    cat "$work/err" >&2
    exit 1
fi

echo "server pid: $server_pid (parent pid: $helper_pid)"

# Kill the parent with SIGKILL: no destructor runs, so only the kernel
# backstop can reclaim the server.
kill -KILL "$helper_pid" 2>/dev/null
wait "$helper_pid" 2>/dev/null

gone=no
for _ in $(seq 1 50); do
    if ! kill -0 "$server_pid" 2>/dev/null; then
        gone=yes
        break
    fi
    sleep 0.1
done

after="$(count_servers)"
echo "servers running after:  $after"

if [ "$gone" = "yes" ]; then
    echo "PASS: the server died with its killed parent (no orphan)"
    exit 0
fi

echo "FAIL: the server outlived its killed parent — it is an orphan" >&2
kill -KILL "$server_pid" 2>/dev/null
exit 1
