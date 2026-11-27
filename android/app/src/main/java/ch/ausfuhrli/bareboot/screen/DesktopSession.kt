package ch.ausfuhrli.bareboot.screen

object DesktopSession {
    val prepareArgv = listOf(
        "/bin/sh",
        "-c",
        "command -v getcap >/dev/null 2>&1 && [ -n \"${'$'}(getcap /usr/bin/kwin_wayland 2>/dev/null)\" ] && setcap -r /usr/bin/kwin_wayland; exit 0",
    )

    val argv = listOf(
        "/bin/sh",
        "-c",
        """
        for f in /proc/[0-9]*/comm; do
            read -r name < "${'$'}f" 2>/dev/null || continue
            case "${'$'}name" in kwin_wayland|weston|labwc) exit 0 ;; esac
        done
        set --
        gpu=/run/bareboot-gpu/guest/bin/bareboot-gpu
        [ -x "${'$'}gpu" ] && set -- "${'$'}gpu" run
        [ -z "${'$'}DBUS_SESSION_BUS_ADDRESS" ] && command -v dbus-run-session >/dev/null 2>&1 && set -- "${'$'}@" dbus-run-session
        cd
        if command -v startplasma-wayland >/dev/null 2>&1; then
            export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=KDE KWIN_FORCE_SW_CURSOR=1
            export MESA_VK_WSI_DEBUG=linear QT_QUICK_BACKEND=software QT_WAYLAND_CLIENT_BUFFER_INTEGRATION=none
            exec "${'$'}@" startplasma-wayland
        fi
        command -v weston >/dev/null 2>&1 && exec "${'$'}@" weston --fullscreen
        command -v labwc >/dev/null 2>&1 && exec "${'$'}@" labwc
        echo "Install kde-plasma-desktop, weston, or labwc in this image to start a desktop." >&2
        exit 1
        """.trimIndent(),
    )
}
