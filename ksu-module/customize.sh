set_perm "$MODPATH/bin/bareboot" 0 0 0755
set_perm "$MODPATH/bin/bareboot-display" 0 0 0755
set_perm "$MODPATH/bin/slirp4netns" 0 0 0755
if [ -f "$MODPATH/gpu/host/kumquat" ]; then
    set_perm "$MODPATH/gpu/host/kumquat" 0 0 0755
    set_perm "$MODPATH/gpu/host/bareboot-media" 0 0 0755
    set_perm "$MODPATH/gpu/guest/glibc/bin/check" 0 0 0755
    set_perm "$MODPATH/gpu/guest/glibc/lib/hwdec/libavcodec.so.63" 0 0 0755
    set_perm "$MODPATH/gpu/guest/musl/bin/check" 0 0 0755
    set_perm "$MODPATH/gpu/guest/bin/bareboot-gpu" 0 0 0755
fi

if [ "$KSU" = true ]; then
    export MODULE_HOT_INSTALL_REQUEST=true
    export MODULE_HOT_RUN_SCRIPT=hotinstall.sh
elif [ "$APATCH" != true ]; then
    cat >>"$MODPATH/sepolicy.rule" <<'EOF'
allow magisk bareboot_daemon process { transition dyntransition }
allow magisk bareboot_daemon fd use
allow magisk bareboot_daemon unix_stream_socket { read write connectto getopt getattr }
allow magisk bareboot_socket dir { search getattr }
allow magisk bareboot_socket sock_file { getattr write }
EOF
    ui_print "- Reboot to start the Bareboot service"
fi
