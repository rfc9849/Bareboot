#!/system/bin/sh
set -eu

STATE_DIR=/data/adb/bareboot
ALLOWED="$STATE_DIR/allowed_uids"
PACKAGE=ch.ausfuhrli.bareboot

USER_ID=$(am get-current-user 2>/dev/null || echo 0)
APP_UID=$(cmd package list packages -U --user "$USER_ID" "$PACKAGE" |
    awk -v pkg="package:$PACKAGE" '$1 == pkg && $2 ~ /^uid:[0-9]+$/ { sub(/^uid:/, "", $2); print $2 }')
case "$APP_UID" in
    ''|*[!0-9]*)
        echo "Bareboot app is not installed for user $USER_ID."
        exit 1
        ;;
esac

umask 077
mkdir -p "$STATE_DIR"
touch "$ALLOWED"
chown 0:0 "$ALLOWED"
chmod 0600 "$ALLOWED"
if grep -qx "$APP_UID" "$ALLOWED"; then
    echo "Bareboot app (UID $APP_UID) is already allowed."
else
    echo "$APP_UID" >>"$ALLOWED"
    echo "Allowed Bareboot app (UID $APP_UID). Return to Bareboot to continue."
fi
