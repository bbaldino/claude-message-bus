// Drives the whole loop without Claude Code: a bus and three agent processes,
// each spoken to over stdio the way Claude Code would. Asserts that a message
// sent by one agent surfaces as a notifications/claude/channel on another, that
// replies flow back, and that messages for an absent agent are held and
// delivered on connect.
import { spawn } from 'node:child_process'

const BIN = './target/debug/round-trip'
const PORT = 7788
const BUS = `ws://127.0.0.1:${PORT}/ws`
const sleep = ms => new Promise(r => setTimeout(r, ms))

const procs = []
const cleanup = () => procs.forEach(p => { try { p.kill() } catch {} })
process.on('exit', cleanup)

function startAgent(name) {
  const p = spawn(BIN, ['agent', '--bus', BUS, '--name', name], { stdio: ['pipe', 'pipe', 'pipe'] })
  procs.push(p)
  const seen = []
  let buf = ''
  p.stdout.on('data', d => {
    buf += d
    const lines = buf.split('\n')
    buf = lines.pop()
    for (const l of lines) { try { seen.push(JSON.parse(l)) } catch {} }
  })
  p.stderr.on('data', () => {})   // agent logs to stderr; quiet here
  const send = o => p.stdin.write(JSON.stringify(o) + '\n')
  send({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {
    protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name, version: '1' } } })
  send({ jsonrpc: '2.0', method: 'notifications/initialized' })
  let id = 100
  return {
    name, seen, proc: p,
    call: (tool, args) => send({ jsonrpc: '2.0', id: ++id, method: 'tools/call',
                                 params: { name: tool, arguments: args } }),
    channelEvents: () => seen.filter(m => m.method === 'notifications/claude/channel'),
    toolResults: () => seen.filter(m => m.result?.content).map(m =>
      m.result.content.map(c => c.text).join('')),
  }
}

let failures = 0
const check = (label, cond, detail = '') => {
  console.log(`${cond ? '  ✓' : '  ✗'} ${label}${detail ? ` — ${detail}` : ''}`)
  if (!cond) failures++
}

// --- bus ---------------------------------------------------------------------
const busProc = spawn(BIN, ['serve', '--port', String(PORT)], { stdio: ['ignore', 'pipe', 'pipe'] })
procs.push(busProc)
busProc.stderr.on('data', () => {})
await sleep(700)

// --- two agents connect ------------------------------------------------------
console.log('\n[1] two agents register and see each other')
const alpha = startAgent('alpha')
const beta = startAgent('beta')
await sleep(1500)

alpha.call('agents', {})
await sleep(400)
const roster = alpha.toolResults().join(' | ')
check('roster shows both agents', roster.includes('alpha') && roster.includes('beta'), roster.trim())

// --- alpha → beta ------------------------------------------------------------
console.log('\n[2] alpha sends; beta receives it as a channel event')
alpha.call('send', { to: 'beta', text: 'PING from alpha' })
await sleep(700)

const betaEvents = beta.channelEvents()
check('beta got exactly one channel event', betaEvents.length === 1, `got ${betaEvents.length}`)
if (betaEvents.length) {
  const p = betaEvents[0].params
  check('content correct', p.content === 'PING from alpha', JSON.stringify(p.content))
  check('meta.from is alpha', p.meta?.from === 'alpha', JSON.stringify(p.meta))
  check('meta.msg_id present', !!p.meta?.msg_id, JSON.stringify(p.meta?.msg_id))
}
check('alpha saw the send echoed back',
  alpha.toolResults().some(t => t.includes('sent → beta: PING from alpha')))

// --- beta → alpha (the reply half) ------------------------------------------
console.log('\n[3] beta replies; alpha receives while idle')
beta.call('send', { to: 'alpha', text: 'PONG from beta' })
await sleep(700)
const alphaEvents = alpha.channelEvents()
check('alpha got the reply', alphaEvents.length === 1, `got ${alphaEvents.length}`)
if (alphaEvents.length) {
  check('reply content correct', alphaEvents[0].params.content === 'PONG from beta')
  check('reply meta.from is beta', alphaEvents[0].params.meta?.from === 'beta')
}

// --- offline queueing --------------------------------------------------------
console.log('\n[4] message to an absent agent is held, then delivered on connect')
alpha.call('send', { to: 'gamma', text: 'held for gamma' })
await sleep(500)
// NOTE: the echo says "sent → gamma" even though gamma is offline. The bus's
// "offline; queued" notice arrives asynchronously over the socket, so the tool
// result cannot reflect it. The real `send` needs an ack from the bus before
// returning, or it will tell the model a message was delivered when it wasn't.
check('send returns an echo (delivery status NOT yet reflected — see note)',
  alpha.toolResults().some(t => t.includes('sent → gamma')))

const gamma = startAgent('gamma')
await sleep(1600)
const gammaEvents = gamma.channelEvents()
check('gamma received the queued message on connect', gammaEvents.length === 1,
  `got ${gammaEvents.length}`)
if (gammaEvents.length) {
  check('queued content correct', gammaEvents[0].params.content === 'held for gamma')
}

// --- result ------------------------------------------------------------------
console.log(`\n${failures === 0 ? 'ALL CHECKS PASSED' : `${failures} CHECK(S) FAILED`}`)
cleanup()
process.exit(failures === 0 ? 0 : 1)
