# Security

Bareboot is for running your own software, not a sandbox.

- Root in Linux is root on the phone. Bareboot drops some capabilities and uses SELinux, but don't run untrusted code.
- **Boot the distribution's init** gives PID 1 more privileges. Use it for software you trust.
- With the shared network, Linux root can change the phone's routing and firewall.
- The service stays in SELinux Enforcing and only accepts the Bareboot app and root.

Report vulnerabilities through a private security advisory on GitHub.
