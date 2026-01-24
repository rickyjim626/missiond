# missiond

**Multi-agent orchestration for Claude Code** - Spawn and control multiple Claude Code instances from a single session via MCP.

[![Crates.io](https://img.shields.io/crates/v/missiond-core.svg)](https://crates.io/crates/missiond-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

## Features

- **PTY Sessions** - Spawn Claude Code in pseudo-terminals with full terminal emulation
- **Semantic Parsing** - Real-time state detection (idle/thinking/confirming), tool output extraction, status bar parsing
- **MCP Integration** - Control agents through Model Context Protocol tools
- **WebSocket API** - Attach to sessions in real-time, monitor Claude Code tasks across all sessions
- **Permission System** - Role-based tool permissions (allow/confirm/deny)
- **Cross-Session Task Monitoring** - Watch what other Claude Code instances are working on

## Architecture

```
┌─────────────────┐     MCP      ┌──────────────┐
│  Claude Code    │◄────────────►│  mission-mcp │
│  (Main Agent)   │              └──────┬───────┘
└─────────────────┘                     │ IPC
                                        ▼
                               ┌──────────────────┐
                               │    missiond      │
                               │  (Daemon)        │
                               ├──────────────────┤
                               │ • Task Queue     │
                               │ • PTY Manager    │
                               │ • Permission Mgr │
                               │ • WebSocket API  │
                               └────────┬─────────┘
                                        │
              ┌─────────────────────────┼─────────────────────────┐
              ▼                         ▼                         ▼
        ┌───────────┐            ┌───────────┐            ┌───────────┐
        │  slot-1   │            │  slot-2   │            │  slot-N   │
        │  Claude   │            │  Claude   │            │  Claude   │
        └───────────┘            └───────────┘            └───────────┘
```

## Installation

### From Cargo (Rust)

```bash
cargo install missiond-mcp --bin mission-mcp
cargo install missiond-daemon --bin missiond
cargo install missiond-attach --bin missiond-attach
```

### From npm (Node.js)

```bash
npm install @missiond/core
```

The npm package includes pre-built binaries for:
- macOS (ARM64, x64)
- Linux (x64 glibc, x64 musl)
- Windows (x64)

## Quick Start

### 1. Configure MCP

Add to `~/.claude/settings.json`:

```json
{
  "mcpServers": {
    "mission": {
      "command": "mission-mcp",
      "args": [],
      "env": { "MISSION_LOG_LEVEL": "warn" }
    }
  }
}
```

### 2. Configure Slots

Create `~/.xjp-mission/slots.yaml`:

```yaml
slots:
  - id: coder-1
    role: coder
    description: "Coding specialist"
    cwd: /path/to/projects

  - id: researcher-1
    role: researcher
    description: "Research and documentation"
    cwd: /path/to/docs
```

### 3. Use from Claude Code

```
User: "Spawn an agent to refactor the auth module"

Claude: I'll spawn a coding agent for that task.
[Uses mission_pty_spawn tool]
[Uses mission_pty_send with the refactoring instructions]
```

## MCP Tools

### Task Operations

| Tool | Description |
|------|-------------|
| `mission_submit` | Submit async task to agent |
| `mission_ask` | Sync expert consultation |
| `mission_status` | Query task status |
| `mission_cancel` | Cancel running task |

### PTY Control

| Tool | Description |
|------|-------------|
| `mission_pty_spawn` | Start PTY session |
| `mission_pty_send` | Send message, wait for response |
| `mission_pty_screen` | Get current terminal screen |
| `mission_pty_confirm` | Handle tool confirmations |
| `mission_pty_interrupt` | Send Ctrl+C |
| `mission_pty_kill` | Close PTY session |

### Claude Code Tasks Monitoring

| Tool | Description |
|------|-------------|
| `mission_cc_sessions` | List all Claude Code sessions |
| `mission_cc_tasks` | Get tasks for a session |
| `mission_cc_overview` | Global task statistics |
| `mission_cc_in_progress` | All in-progress tasks |

## Semantic Terminal Parsing

The daemon includes sophisticated terminal parsing for Claude Code:

- **State Detection** - Tracks idle, thinking, confirming, error states
- **Confirm Dialog Parsing** - Extracts tool name, parameters, file paths from confirmation prompts
- **Status Bar Parsing** - Reads spinner state and status text
- **Tool Output Extraction** - Parses both boxed and inline tool outputs
- **Title Parsing** - Monitors terminal title changes

```rust
pub enum SessionEvent {
    StateChange { new_state, prev_state },
    ConfirmRequired { prompt, info },
    StatusUpdate(ClaudeCodeStatus),
    ToolOutput(ClaudeCodeToolOutput),
    TitleChange(ClaudeCodeTitle),
    // ...
}
```

## WebSocket API

### PTY Attach

```
ws://localhost:9120/pty/<slot-id>
```

Connect to watch or interact with a PTY session in real-time.

### Tasks Events

```
ws://localhost:9120/tasks
```

Subscribe to task lifecycle events:
- `cc_tasks_changed` - Tasks updated
- `cc_task_started` - Task started
- `cc_task_completed` - Task completed
- `cc_session_active` / `cc_session_inactive`

## Node.js Client

```typescript
import { MissionControl } from '@missiond/core';

const mission = new MissionControl();
await mission.connect();  // Auto-starts daemon

// Spawn PTY session
const pty = await mission.pty.spawn('slot-1', 'claude');
pty.on('state', (state) => console.log('State:', state));
pty.on('confirm', (info) => console.log('Confirm:', info));

// Send message and wait for response
const response = await pty.send('Explain this codebase');
console.log(response);

await pty.kill();
mission.close();
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `XJP_MISSION_HOME` | `~/.xjp-mission` | Config directory |
| `MISSION_DB_PATH` | `$HOME/mission.db` | SQLite database |
| `MISSION_SLOTS_CONFIG` | `$HOME/slots.yaml` | Slot definitions |
| `MISSION_IPC_SOCKET` | `$HOME/missiond.sock` | Unix socket path |
| `MISSION_WS_PORT` | `9120` | WebSocket port |
| `MISSION_LOG_LEVEL` | `warn` | Log level |

### Permissions

Configure tool permissions in `~/.xjp-mission/permissions.yaml`:

```yaml
roles:
  coder:
    allow:
      - "Bash(*)"
      - "Read(*)"
      - "Write(*)"
    confirm:
      - "Edit(*)"
    deny:
      - "Bash(rm -rf*)"

  researcher:
    allow:
      - "Read(*)"
      - "WebSearch(*)"
    deny:
      - "Bash(*)"
      - "Write(*)"
```

## Crates

| Crate | Description |
|-------|-------------|
| `missiond-core` | Core library: PTY, semantic parsing, task queue |
| `missiond-mcp` | MCP server binary (`mission-mcp`) |
| `missiond-daemon` | Daemon binary (`missiond`) |
| `missiond-runner` | Claude CLI wrapper |
| `missiond-attach` | PTY attach CLI (`missiond-attach`) |

## License

MIT License - see [LICENSE](LICENSE) for details.
