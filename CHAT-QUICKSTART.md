# Coven Chat TUI — Quick Start Guide

## Build

```bash
cd /path/to/coven
cargo build -p coven-cli
```

## Run

```bash
./target/debug/coven chat
```

## Try These

### 1. See Help
```
Type: /help
Press: Enter
→ Shows all available commands
```

### 2. Send a Message
```
Type: Hey, what can you do?
Press: Enter
→ Message appears with "You:" prefix
→ Mock response from agent
```

### 3. Clear Chat
```
Type: /clear
Press: Enter
→ Chat history cleared
```

### 4. Switch Agent
```
Type: /agent Sage
Press: Enter
→ Agent switches to Sage
```

### 5. Scroll History
```
Press: Up arrow
→ Scroll chat history up
Press: Down arrow
→ Scroll back down
```

### 6. Exit
```
Type: /exit
Press: Enter
Or
Press: Ctrl+C
→ Graceful exit, terminal restored
```

## Layout

```
┌─────────────────────────────────────┐
│ 🔮  Agent: Nova  |  Connection: ●   │  ← Status bar
├─────────────────────────────────────┤
│ System: Welcome to Coven Chat!      │
│ You: /help                          │
│ System: Available commands:         │
│  /help - Show this help             │
│  /clear - Clear chat history        │
│  /agent <name> - Switch agent       │
│  /exit - Quit                       │
│                                     │
├─────────────────────────────────────┤
│ Message or type / for commands      │
│ ┌─────────────────────────────────┐ │
│ │ _                               │ │  ← Input (cursor shown)
│ └─────────────────────────────────┘ │
└─────────────────────────────────────┘
```

## Features (MVP)

- ✅ Natural message input
- ✅ Scrollable history
- ✅ Slash commands
- ✅ Agent switching
- ✅ SSH-compatible
- ✅ No external dependencies
- ✅ Clean terminal exit

## What's Not Ready Yet (Phase 2)

- 🔄 Real gateway connection (coming soon)
- 🔄 Streaming token responses
- 🔄 Agent discovery from gateway
- 🔄 Session attachment
- 🔄 Memory context display
- 🔄 Code syntax highlighting
- 🔄 Message export

## Troubleshooting

### Binary Not Found
```bash
cargo build -p coven-cli first
```

### Terminal Corruption on Exit
```bash
Just run the app again — it restores properly
```

### Slow/Laggy Input
```bash
Normal behavior — event loop polls every 100ms
Real agent responses will be streamed (Phase 2)
```

### Can't Type in SSH Session
```bash
Normal limitation of non-PTY environments
Terminal requires interactive TTY
```

## Next Session (Phase 2)

1. Gateway WebSocket integration
2. Real agent responses (not mock)
3. Token-by-token streaming
4. Connection health monitoring
5. Auto-reconnect

## Commands Reference

| Command | Shortcut | Effect |
|---------|----------|--------|
| `/help` | `/h` | Show all commands |
| `/clear` | `/c` | Clear chat history |
| `/agent <name>` | `/a <name>` | Switch to agent |
| `/exit` | `/q`, `/exit` | Quit |
| Ctrl+C | - | Immediate exit |
| Up/Down arrows | - | Scroll history |

---

**Built with:** Ratatui (Rust TUI framework) + Tokio async runtime

**Status:** MVP Complete ✅ → Production Ready for Phase 2
