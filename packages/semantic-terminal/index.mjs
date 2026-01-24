// ESM wrapper
import { createRequire } from 'module'
const require = createRequire(import.meta.url)
const binding = require('./index.js')

// Enums
export const State = binding.State
export const ConfirmType = binding.ConfirmType
export const StatusPhase = binding.StatusPhase
export const ToolStatus = binding.ToolStatus
export const FingerprintCategory = binding.FingerprintCategory

// Classes
export const StateParser = binding.StateParser
export const ConfirmParser = binding.ConfirmParser
export const StatusParser = binding.StatusParser
export const TitleParser = binding.TitleParser
export const ToolOutputParser = binding.ToolOutputParser
export const Registry = binding.Registry

// Convenience functions
export const detectState = binding.detectState
export const detectConfirm = binding.detectConfirm
export const parseStatus = binding.parseStatus
export const parseToolOutput = binding.parseToolOutput
export const extractFingerprints = binding.extractFingerprints
export const knownTools = binding.knownTools
export const spinnerChars = binding.spinnerChars
