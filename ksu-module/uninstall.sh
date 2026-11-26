#!/system/bin/sh
STATE_DIR=/data/adb/bareboot

if [ -f "$STATE_DIR/purge-on-uninstall" ]; then
    rm -rf "$STATE_DIR"
fi
