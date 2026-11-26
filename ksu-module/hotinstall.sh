#!/system/bin/sh
set -u

MODDIR=$(dirname "$(readlink -f "$0")")
BIN="$MODDIR/bin/bareboot"
SOCK=/dev/socket/bareboot/barebootd.sock

if command -v ksud >/dev/null 2>&1; then
    ksud sepolicy apply "$MODDIR/sepolicy.rule" >/dev/null
elif [ -x /data/adb/ksu/bin/ksud ]; then
    /data/adb/ksu/bin/ksud sepolicy apply "$MODDIR/sepolicy.rule" >/dev/null
fi

pid=$("$BIN" --socket "$SOCK" ping 2>/dev/null | sed -n 's/.*daemon_pid=\([0-9]*\).*/\1/p')
if [ -z "$pid" ]; then
    pid=$("$BIN" --socket @barebootd ping 2>/dev/null | sed -n 's/.*daemon_pid=\([0-9]*\).*/\1/p')
fi
if [ -n "$pid" ]; then
    echo "Stopping the running Bareboot service (PID $pid)..."
    kill "$pid"
    waited=0
    while kill -0 "$pid" 2>/dev/null; do
        if [ "$waited" -ge 100 ]; then
            echo "The old service did not stop within 20 seconds. Reboot to finish the update."
            exit 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
fi

sh "$MODDIR/service.sh"
sleep 1
if "$BIN" --socket "$SOCK" ping; then
    echo "Bareboot service updated without a reboot."
else
    echo "The new service did not start. Check /data/adb/bareboot/barebootd.log or reboot."
    exit 1
fi
