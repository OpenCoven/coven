# Gateway Hardening Reference

## Auth Modes

| Mode | Security Level | When to Use |
|------|---------------|-------------|
| `device` | Highest | Default — requires device identity verification |
| `token` | Medium | Behind reverse proxy or Funnel with restricted access |
| `password` | Low | Temporary/break-glass only |
| `none` | Dangerous | Never in production |

## Control UI Security Checklist

### `allowedOrigins`
- **Never use `"*"` in production.** Wildcard allows any browser origin to reach Control UI / WebChat WebSocket.
- Set explicitly to trusted origins: your Funnel URL, `http://localhost:18789`, your dashboard domain.
- Pattern: `["https://your-funnel-hostname", "http://localhost:18789"]`

### `dangerouslyDisableDeviceAuth`
- When `true`: any caller with the auth token can access Control UI without device identity verification.
- **Fix**: Set to `false` (or remove — defaults to `false`).
- Only enable during short-lived break-glass debugging sessions, then immediately disable.

### `dangerouslyAllowHostHeaderOriginFallback`
- When `true`: WebSocket origin checks fall back to the Host header, weakening DNS rebinding protections.
- **Fix**: Set to `false` and configure explicit `allowedOrigins` instead.
- This flag exists for environments where the browser doesn't send an Origin header (rare).

### `trustedProxies`
- If gateway is behind a reverse proxy (Tailscale Funnel, nginx, Cloudflare), configure trusted proxy IPs.
- Without this, the gateway cannot distinguish proxy-forwarded headers from spoofed headers.
- For Tailscale Funnel: typically `["127.0.0.1", "::1"]` (Funnel connects via loopback).

## Bind Address

- `127.0.0.1` (loopback) — only local access. Safe default.
- `0.0.0.0` — binds all interfaces. Requires firewall + auth hardening.
- If using Funnel/proxy, keep loopback bind + configure `trustedProxies`.

## Config Patch Patterns

### Lock down Control UI (typical fix)
```json
{
  "gateway": {
    "controlUi": {
      "allowedOrigins": ["https://your-dashboard.example.com", "http://localhost:18789"],
      "dangerouslyDisableDeviceAuth": false,
      "dangerouslyAllowHostHeaderOriginFallback": false
    },
    "trustedProxies": ["127.0.0.1", "::1"]
  }
}
```

### Verify after patching
```bash
openclaw security audit --deep
```
Expect: CRITICAL count drops to 0, WARN count decreases.
