// Verifies the OUTBOUND half of the Rust probe without involving Claude Code:
// drive it over stdio, POST to its HTTP port, and assert that a well-formed
// notifications/claude/channel appears on stdout.
import { spawn } from 'node:child_process'
import { request } from 'node:http'

const proc = spawn('./target/debug/rust-probe', { stdio: ['pipe', 'pipe', 'pipe'] })
const seen = []
let buf = ''
proc.stdout.on('data', d => {
  buf += d
  const lines = buf.split('\n')
  buf = lines.pop()
  for (const l of lines) { try { seen.push(JSON.parse(l)) } catch {} }
})

const send = o => proc.stdin.write(JSON.stringify(o) + '\n')
const sleep = ms => new Promise(r => setTimeout(r, ms))

send({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {
  protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'harness', version: '1' } } })
send({ jsonrpc: '2.0', method: 'notifications/initialized' })
await sleep(1200)

await new Promise((resolve, reject) => {
  const req = request({ host: '127.0.0.1', port: 8789, path: '/', method: 'POST' }, res => {
    res.resume(); res.on('end', resolve)
  })
  req.on('error', reject)
  req.end('RUST_PROBE_PAYLOAD')
})
await sleep(600)

const note = seen.find(m => m.method === 'notifications/claude/channel')
console.log('--- outbound notification on the wire ---')
console.log(note ? JSON.stringify(note, null, 2) : '(NONE FOUND)')
console.log('\nNOTIFICATION EMITTED:', !!note)
if (note) {
  console.log('method correct:', note.method === 'notifications/claude/channel')
  console.log('content correct:', note.params?.content === 'RUST_PROBE_PAYLOAD')
  console.log('meta:', JSON.stringify(note.params?.meta))
}
proc.kill()
process.exit(note ? 0 : 1)
