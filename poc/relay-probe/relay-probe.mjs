#!/usr/bin/env node
// Milestone 0 for the hub/permission-relay design: does permission relay actually
// work here, and does --dangerously-skip-permissions really suppress every prompt?
//
// Answers five questions from the spec:
//   1. Does relay fire at all on this account and client version?
//   2. What do request_id / tool_name / description / input_preview actually contain?
//   3. Does --dangerously-skip-permissions suppress every prompt, or do some survive?
//   4. Does a verdict sent over the channel actually satisfy the prompt?
//   5. What happens to a request nobody answers?
//
// Written in Node against the reference SDK on purpose: a failure here is the
// platform or our config, never our Rust.
//
// NOTE: stdout is the JSON-RPC transport. All logging goes to stderr and a file.

import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { CallToolRequestSchema, ListToolsRequestSchema } from '@modelcontextprotocol/sdk/types.js'
import { writeFileSync, appendFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { z } from 'zod'

const HERE = dirname(fileURLToPath(import.meta.url))
const LOG = join(HERE, 'relay-probe.log')

// How long to wait before answering, so a human can watch the local dialog and the
// relayed request coexist. VERDICT=deny tests the other branch; VERDICT=none leaves
// it unanswered to answer question 5.
const DELAY_MS = Number(process.env.PROBE_DELAY_MS ?? 6000)
const VERDICT = process.env.PROBE_VERDICT ?? 'allow'

const log = (...a) => {
  const line = `[${new Date().toISOString()}] ${a.join(' ')}\n`
  process.stderr.write(line)
  try { appendFileSync(LOG, line) } catch {}
}

writeFileSync(LOG, '')
log(`probe starting — verdict=${VERDICT} delay=${DELAY_MS}ms`)

const mcp = new Server(
  { name: 'relay-probe', version: '0.0.1' },
  {
    capabilities: {
      experimental: {
        'claude/channel': {},
        // The whole experiment. Without this key Claude Code never forwards a
        // permission prompt, and question 1 answers itself.
        'claude/channel/permission': {},
      },
      tools: {},
    },
    instructions:
      'This is a permission-relay probe. It observes approval prompts; it does not ' +
      'need you to do anything special. Behave normally.',
  },
)

// The first attempt declared `tools: {}` with no `tools/list` handler, so Claude Code
// asked for the tool list and got "Method not found" — and no permission prompt was ever
// relayed, despite a real Write dialog being on screen. The docs describe relay in the
// context of a *two-way* channel, i.e. one that actually exposes a reply tool, so a
// server advertising a capability it cannot answer is the prime suspect. This tool exists
// to make the probe genuinely two-way; it is also a useful escape hatch for answering a
// prompt by hand.
mcp.setRequestHandler(ListToolsRequestSchema, async () => {
  log('tools/list requested')
  return {
    tools: [{
      name: 'probe_answer',
      description: 'Answer a relayed permission request by id',
      inputSchema: {
        type: 'object',
        properties: {
          request_id: { type: 'string', description: 'The request_id from the prompt' },
          behavior: { type: 'string', description: '"allow" or "deny"' },
        },
        required: ['request_id', 'behavior'],
      },
    }],
  }
})

mcp.setRequestHandler(CallToolRequestSchema, async req => {
  if (req.params.name !== 'probe_answer') throw new Error(`unknown tool: ${req.params.name}`)
  const { request_id, behavior } = req.params.arguments ?? {}
  log(`probe_answer called: ${request_id} -> ${behavior}`)
  await mcp.notification({
    method: 'notifications/claude/channel/permission',
    params: { request_id, behavior },
  })
  return { content: [{ type: 'text', text: `sent ${behavior} for ${request_id}` }] }
})

// Claude Code -> us, when a permission dialog opens.
const PermissionRequestSchema = z.object({
  method: z.literal('notifications/claude/channel/permission_request'),
  params: z.object({
    request_id: z.string(),
    tool_name: z.string(),
    description: z.string(),
    input_preview: z.string(),
  }),
})

let seen = 0

mcp.setNotificationHandler(PermissionRequestSchema, async ({ params }) => {
  seen += 1
  // Dump every field verbatim and with its length — question 2 is entirely about
  // whether `description` is useful or the bare "Run shell command" constant, and
  // whether input_preview is truncated.
  log('')
  log(`=== PERMISSION REQUEST #${seen} ===`)
  log(`  request_id     ${JSON.stringify(params.request_id)}`)
  log(`  tool_name      ${JSON.stringify(params.tool_name)}`)
  log(`  description    (${params.description.length} chars) ${JSON.stringify(params.description)}`)
  log(`  input_preview  (${params.input_preview.length} chars) ${JSON.stringify(params.input_preview)}`)
  log(`  full params    ${JSON.stringify(params)}`)

  if (VERDICT === 'none') {
    log(`  -> PROBE_VERDICT=none: deliberately not answering (question 5)`)
    return
  }

  log(`  -> answering "${VERDICT}" in ${DELAY_MS}ms — watch the local dialog meanwhile`)
  setTimeout(async () => {
    try {
      await mcp.notification({
        method: 'notifications/claude/channel/permission',
        params: { request_id: params.request_id, behavior: VERDICT },
      })
      log(`  -> verdict "${VERDICT}" written to transport for ${params.request_id}`)
      log(`     (resolving here means it hit the wire, NOT that Claude accepted it)`)
    } catch (e) {
      log(`  -> FAILED to send verdict: ${e?.message}`)
    }
  }, DELAY_MS)
})

await mcp.connect(new StdioServerTransport())
log('mcp connected over stdio; waiting for permission prompts')

// Periodic heartbeat so a silent run is distinguishable from a crashed one — which
// matters, because "no prompts fired" is itself a result for question 3.
setInterval(() => log(`still listening; ${seen} permission request(s) so far`), 30000)
