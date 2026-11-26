# Docker

Needs **Boot the distribution's init** and **Isolated network** on the image.

```sh
sudo apt install docker.io     # Debian
sudo pacman -S docker          # Arch
sudo systemctl enable --now docker
sudo docker run hello-world
```

If the kernel lacks nftables (`CONFIG_NF_TABLES`), switch iptables to the legacy backend first:

```sh
sudo update-alternatives --set iptables /usr/sbin/iptables-legacy     # Debian
cd /usr/bin && for t in iptables ip6tables; do for s in '' -save -restore; do sudo ln -sf xtables-legacy-multi $t$s; done; done     # Arch
```

If the kernel lacks IPC namespaces, add `--ipc host --tmpfs /dev/mqueue` to `docker run`.

## Without isolated network

Docker would change the phone's firewall. Put this in `/etc/docker/daemon.json` and use `--network host`:

```json
{ "iptables": false, "ip6tables": false, "ip-forward": false, "ip-masq": false, "bridge": "none" }
```
