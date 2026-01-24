const {
  StateParser,
  ConfirmParser,
  StatusParser,
  ToolOutputParser,
  Registry,
  detectState,
  detectConfirm,
  parseStatus,
  parseToolOutput,
  knownTools,
  spinnerChars,
} = require('./index.js')

// Test StateParser
console.log('=== StateParser ===')
const stateParser = new StateParser()

const idleResult = stateParser.detect(['❯ '])
console.log('Idle:', idleResult)

const thinkingResult = stateParser.detect(['Processing...', 'esc to interrupt'])
console.log('Thinking:', thinkingResult)

const confirmResult = stateParser.detect([
  'xjp-mcp - xjp_secret_get(key: "test")',
  '❯ 1. Yes, allow this action',
  '  2. Yes, allow for this session',
  '  3. No, deny this action',
  'Esc to cancel',
])
console.log('Confirming:', confirmResult)

// Test ConfirmParser
console.log('\n=== ConfirmParser ===')
const confirmParser = new ConfirmParser()

const confirm = confirmParser.detect([
  'xjp-mcp - xjp_secret_get(key: "test")',
  '❯ 1. Yes, allow this action',
  '  2. Yes, allow for this session',
  '  3. No, deny this action',
  'Esc to cancel',
])
console.log('Confirm info:', JSON.stringify(confirm, null, 2))
console.log('Format confirm:', JSON.stringify(confirmParser.formatConfirm()))
console.log('Format deny (Options):', JSON.stringify(confirmParser.formatDeny('Options')))
console.log('Format select(2):', JSON.stringify(confirmParser.formatSelect(2)))

// Test StatusParser
console.log('\n=== StatusParser ===')
const statusParser = new StatusParser()

const status = statusParser.parse(['· Precipitating… (esc to interrupt · thinking)'])
console.log('Status:', status)

// Test ToolOutputParser
console.log('\n=== ToolOutputParser ===')
const toolParser = new ToolOutputParser()

const toolOutput = toolParser.parse([
  '⏺ Bash',
  '  │ command: "git status"',
])
console.log('Tool output:', JSON.stringify(toolOutput, null, 2))

// Test Registry
console.log('\n=== Registry ===')
const registry = new Registry()

const fingerprints = registry.extract(['❯ ', '· Processing...'])
console.log('Hints:', fingerprints.hints)
console.log('Has prompt:', registry.hasPrompt(['❯ ']))
console.log('Has spinner:', registry.hasSpinner(['· ']))
console.log('Has error:', registry.hasError(['Error: something wrong']))

// Test convenience functions
console.log('\n=== Convenience Functions ===')
console.log('Known tools:', knownTools())
console.log('Spinner chars:', spinnerChars())

console.log('\n✅ All tests passed!')
