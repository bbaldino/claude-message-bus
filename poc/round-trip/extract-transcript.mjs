// Turn bus.log into a readable transcript. The bus logs every routed message as
// "from → to: text", with continuation lines for multiline bodies.
import { readFileSync, writeFileSync } from 'node:fs'

const raw = readFileSync('bus.log', 'utf8').split('\n')
const out = [
  '# POC 3 — live two-session transcript',
  '',
  'Captured 2026-07-25 from `bus.log`. Two Claude Code sessions in different project',
  'directories, negotiating an RPC wire format over the message bus.',
  '',
  'The human typed **one** prompt, into `project-alpha` only. Everything below —',
  "including every one of project-beta's messages — happened with no further human",
  'input.',
  '',
  '---',
  '',
]

let n = 0
let started = false
for (const line of raw) {
  const m = line.match(/^(project-[\w-]+) → (project-[\w-]+): (.*)$/)
  if (m) {
    if (started) out.push('')
    n++
    out.push(`## ${n}. ${m[1]} → ${m[2]}`, '', m[3])
    started = true
  } else if (started && !/^(registered|disconnected|bus listening)/.test(line)) {
    out.push(line)
  }
}

writeFileSync('TRANSCRIPT.md', out.join('\n'))
console.log(`wrote TRANSCRIPT.md — ${n} messages, ${out.join('\n').length} chars`)
