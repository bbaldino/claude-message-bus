import { expect, test } from 'vitest'
import { kindTone } from './eventKind'

test('maps kinds to the handoff hue families', () => {
  // These are the kinds `append_event(` actually writes (grepped in src/),
  // not the guessed set from the design handoff — see eventKind.ts for the
  // full mapping and why each guess was wrong.
  expect(kindTone('message_sent')).toBe('accent') // blue — delivery
  expect(kindTone('agent_registered')).toBe('human') // violet — lifecycle
  expect(kindTone('room_paused')).toBe('attention') // amber — attention
  expect(kindTone('agent_deleted')).toBe('destructive') // red — destructive
  expect(kindTone('file_stored')).toBe('files') // teal — files
  expect(kindTone('agent_disconnected')).toBe('presence') // green — presence
})

test('an unknown kind falls back rather than throwing', () => {
  // New bus events must render, not crash a screen.
  expect(kindTone('something_new')).toBe('accent')
})
