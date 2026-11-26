#!/system/bin/sh
set -eu

MODDIR=$(dirname "$(readlink -f "$0")")
STATE_DIR=/data/adb/bareboot
BIN="$MODDIR/bin/bareboot"

umask 077
mkdir -p "$STATE_DIR"
test -x "$BIN"
LOG="$STATE_DIR/barebootd.log"
if [ -f "$LOG" ] && [ "$(stat -c %s "$LOG")" -gt 1048576 ]; then
    mv -f "$LOG" "$LOG.1"
fi
while IFS=: read -r _ controllers group; do
    case "$group" in
        */uid_[0-9]*/pid_[0-9]*) ;;
        *) continue ;;
    esac
    case ",$controllers," in
        ,,)
            cgroup_root=$(awk '$3 == "cgroup2" { print $2; exit }' /proc/mounts)
            ;;
        *,cpuacct,*) cgroup_root=/acct ;;
        *,memory,*) cgroup_root=/dev/memcg ;;
        *) continue ;;
    esac
    printf '%s\n' "$$" >"${cgroup_root:?}/cgroup.procs"
done <"/proc/$$/cgroup"
if [ -f "$MODDIR/bin/bareboot-display" ] &&
   ! /system/bin/chcon u:object_r:bareboot_display_exec:s0 "$MODDIR/bin/bareboot-display"; then
    printf '%s\n' 'Could not label the Bareboot display server.' >>"$LOG"
fi
if [ -f "$MODDIR/gpu/host/kumquat" ]; then
    if ! /system/bin/chcon u:object_r:bareboot_gpu_exec:s0 "$MODDIR/gpu/host/kumquat"          "$MODDIR/gpu/host/libgfxstream_backend.so"          "$MODDIR/gpu/host/bareboot-media" ||
       ! /system/bin/chcon -R u:object_r:bareboot_gpu_guest:s0 "$MODDIR/gpu/guest"; then
        printf '%s\n' 'Could not label the Bareboot GPU bundle.' >>"$LOG"
    fi
fi
nohup "$BIN" --socket /dev/socket/bareboot/barebootd.sock daemon --state-dir "$STATE_DIR" \
    >>"$LOG" 2>&1 </dev/null &
