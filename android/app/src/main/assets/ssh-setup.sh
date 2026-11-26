stop_sshd() {
    if [ -f /run/bareboot-sshd.pid ]; then
        kill "$(cat /run/bareboot-sshd.pid)" 2>/dev/null || true
        rm -f /run/bareboot-sshd.pid
    fi
}

install_sshd() {
    if command -v sshd >/dev/null 2>&1; then return 0; fi
    echo "Installing the OpenSSH server..."
    if command -v apt-get >/dev/null 2>&1; then
        apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y openssh-server
    elif command -v dnf >/dev/null 2>&1; then
        dnf install -y openssh-server
    elif command -v zypper >/dev/null 2>&1; then
        zypper -n install openssh-server
    elif command -v pacman >/dev/null 2>&1; then
        pacman -Sy --noconfirm openssh
    elif command -v apk >/dev/null 2>&1; then
        apk update && apk add openssh-server
    elif command -v xbps-install >/dev/null 2>&1; then
        xbps-install -Sy openssh
    else
        echo "No supported package manager found."
        return 1
    fi
}

setup() {
    set -e
    user=$(awk -F: '$3 == 1000 { print $1; exit }' /etc/passwd)
    home=$(awk -F: '$3 == 1000 { print $6; exit }' /etc/passwd)
    if [ -z "$user" ] || [ -z "$home" ]; then
        echo "This image has no user with UID 1000."
        return 1
    fi
    if [ "$BAREBOOT_SSH_ENABLE" != 1 ]; then
        stop_sshd
        rm -f /etc/bareboot/autostart
        echo "SSH access is off."
        return 0
    fi
    install_sshd
    sshd_bin=$(command -v sshd)
    ssh-keygen -A
    mkdir -p "$home/.ssh"
    printf '%s\n' "$BAREBOOT_SSH_KEYS" > "$home/.ssh/authorized_keys"
    chown "$user" "$home/.ssh" "$home/.ssh/authorized_keys"
    chmod 700 "$home/.ssh"
    chmod 600 "$home/.ssh/authorized_keys"
    mkdir -p /etc/bareboot
    cat > /etc/bareboot/autostart <<EOF
#!/bin/sh
mkdir -p /run/sshd
exec $sshd_bin -E /var/log/bareboot-sshd.log -p $BAREBOOT_SSH_PORT -o PidFile=/run/bareboot-sshd.pid \
    -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no -o PermitRootLogin=no \
    -o PubkeyAuthentication=yes
EOF
    chmod 755 /etc/bareboot/autostart
    stop_sshd
    /etc/bareboot/autostart
    sleep 1
    if [ ! -f /run/bareboot-sshd.pid ]; then
        echo "sshd did not start. Its log:"
        tail -n 20 /var/log/bareboot-sshd.log 2>/dev/null
        return 1
    fi
    echo
    echo "SSH is running on port $BAREBOOT_SSH_PORT for $user."
}

( setup )
status=$?
echo
if [ "$status" -eq 0 ]; then echo "Done."; else echo "SSH setup failed."; fi
printf 'Press Enter to close this tab. '
read -r _ || true
