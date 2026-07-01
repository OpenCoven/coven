# Host OS Hardening Reference

## macOS Checks

### Firewall
```bash
# Application Firewall status
/usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate

# pf firewall status
sudo pfctl -s info 2>/dev/null | head -5
```
- Application Firewall should be enabled.
- Stealth mode recommended: `--getstealthmode`

### Listening Ports
```bash
lsof -nP -iTCP -sTCP:LISTEN
```
- Review each listening service.
- Flag unexpected services or services bound to `0.0.0.0` (all interfaces).
- Expected for OpenClaw: gateway on `127.0.0.1:18789`.

### Disk Encryption (FileVault)
```bash
fdesetup status
```
- Should return "FileVault is On."
- If off: **HIGH priority** — all credentials on disk are unprotected.

### Automatic Security Updates
```bash
# Check auto-update settings
defaults read /Library/Preferences/com.apple.SoftwareUpdate AutomaticCheckEnabled 2>/dev/null
defaults read /Library/Preferences/com.apple.SoftwareUpdate CriticalUpdateInstall 2>/dev/null
```
- Both should return `1` (enabled).

### Backup Status (Time Machine)
```bash
tmutil status 2>/dev/null
tmutil listbackups 2>/dev/null | tail -3
```
- Verify backups are running and recent (within 24h).

## Linux Checks

### Firewall
```bash
# UFW
ufw status verbose 2>/dev/null

# firewalld
firewall-cmd --state 2>/dev/null
firewall-cmd --list-all 2>/dev/null

# nftables
nft list ruleset 2>/dev/null | head -20
```

### Listening Ports
```bash
ss -ltnup 2>/dev/null || ss -ltnp
```

### Disk Encryption
```bash
# LUKS
lsblk -f | grep -i crypt
```

### Automatic Updates
```bash
# Debian/Ubuntu
cat /etc/apt/apt.conf.d/20auto-upgrades 2>/dev/null

# RHEL/Fedora
systemctl status dnf-automatic.timer 2>/dev/null
```

## OpenClaw Version Check

```bash
openclaw update status
```
- Note current channel (stable/beta/nightly).
- Flag if update is available.
- Version currency matters for security patches.

## Audit Summary Template

```
### Host Hardening — [date]
- OS: [name + version]
- Firewall: [on/off + mode]
- Disk encryption: [on/off]
- Auto-updates: [enabled/disabled]
- Backup status: [current/stale/none]
- Listening ports: [count] ([unexpected count] unexpected)
- OpenClaw version: [version] ([up to date/update available])
```
