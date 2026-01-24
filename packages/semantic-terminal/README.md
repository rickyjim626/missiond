# @anthropic/semantic-terminal

Claude Code terminal output parser - detect states, parse confirmations, extract tool outputs.

**Rust-powered performance with zero-dependency Node.js API.**

## Installation

```bash
npm install @anthropic/semantic-terminal
```

Pre-built binaries available for:
- macOS (ARM64, x64)
- Linux (x64 glibc, x64 musl)
- Windows (x64)

## Quick Start

```typescript
import { detectState, detectConfirm, parseToolOutput, State } from '@anthropic/semantic-terminal'

// Detect terminal state
const state = detectState(['❯ '])
console.log(state.state) // 'Idle'

// Parse confirmation dialog
const confirm = detectConfirm([
  'xjp-mcp - xjp_secret_get(key: "test")',
  '❯ 1. Yes, allow this action',
  '  2. Yes, allow for this session',
  '  3. No, deny this action',
  'Esc to cancel',
])
console.log(confirm.tool.name) // 'xjp_secret_get'

// Parse tool output
const tool = parseToolOutput([
  '⏺ Bash',
  '  │ command: "git status"',
])
console.log(tool.toolName) // 'Bash'
```

## API

### State Detection

```typescript
import { StateParser, detectState, State } from '@anthropic/semantic-terminal'

// Using class
const parser = new StateParser()
const result = parser.detect(['❯ '])

// Using convenience function
const result = detectState(['❯ '])

// Result type
interface StateResult {
  state: State  // 'Starting' | 'Idle' | 'Thinking' | 'ToolRunning' | 'Confirming' | 'Error'
  confidence: number
  needsTrustConfirm?: boolean
  confirmType?: ConfirmType
}
```

### Confirmation Parsing

```typescript
import { ConfirmParser, detectConfirm, ConfirmType } from '@anthropic/semantic-terminal'

const parser = new ConfirmParser()
const info = parser.detect(lines)

// Format responses for PTY
parser.formatConfirm()     // "\r" (Enter)
parser.formatDeny('Options') // "\x1b[B\x1b[B\r" (Down Down Enter)
parser.formatSelect(2)     // "\x1b[B\r" (Down Enter)
```

### Status Bar Parsing

```typescript
import { StatusParser, parseStatus } from '@anthropic/semantic-terminal'

const status = parseStatus(['· Precipitating… (esc to interrupt · thinking)'])
// { spinner: '·', statusText: 'Precipitating…', phase: 'Thinking', interruptible: true }
```

### Tool Output Parsing

```typescript
import { ToolOutputParser, parseToolOutput } from '@anthropic/semantic-terminal'

const tool = parseToolOutput([
  '⏺ Bash (completed in 0.5s)',
  '  │ command: "git status"',
])
// { toolName: 'Bash', params: { command: 'git status' }, durationMs: 500, status: 'Completed' }
```

### Fingerprint Registry

Fast pattern matching with 22 pre-defined Claude Code fingerprints:

```typescript
import { Registry, extractFingerprints } from '@anthropic/semantic-terminal'

const registry = new Registry()

// Quick checks
registry.hasSpinner(lines)  // true/false
registry.hasPrompt(lines)   // true/false
registry.hasError(lines)    // true/false

// Full extraction
const result = registry.extract(lines)
console.log(result.hints)  // { hasSpinner, hasPrompt, hasToolOutput, hasConfirmDialog, hasError }
```

### Constants

```typescript
import { knownTools, spinnerChars } from '@anthropic/semantic-terminal'

knownTools()   // ['Bash', 'Read', 'Edit', 'Write', 'Glob', 'Grep', ...]
spinnerChars() // ['·', '✻', '✽', '✶', '✳', '✢']
```

## Use Cases

- **PTY Automation**: Detect when Claude Code is waiting for input or confirmation
- **Task Monitoring**: Track tool execution and state changes
- **Custom UIs**: Build terminal UIs that react to Claude Code state
- **Testing**: Validate Claude Code output in automated tests

## License

MIT
