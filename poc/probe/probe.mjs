#!/usr/bin/env node
// POC 1 — does the channel mechanism work on this account, in this environment?
//
// Answers three of the four open unknowns from the design spec:
//   1. Does org policy permit channels at all?
//   3. Does the MCP subprocess inherit the project dir, and is CLAUDE_PROJECT_DIR set?
//   4. Does a tool's return value render in the terminal?
//
// Written in Node against the reference SDK on purpose: any failure here is org
// policy or config, never our own protocol implementation.
//
// NOTE: stdout is the JSON-RPC transport. All logging goes to stderr or a file.

import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { ListToolsRequestSchema, CallToolRequestSchema } from '@modelcontextprotocol/sdk/types.js'
import { createServer } from 'node:http'
import { writeFileSync, appendFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join, basename } from 'node:path'

const HERE = dirname(fileURLToPath(import.meta.url))
const DIAG = join(HERE, 'diagnostics.json')
const LOG = join(HERE, 'probe.log')
const PORT = 8788

const log = (...a) => {
  const line = `[${new Date().toISOString()}] ${a.join(' ')}\n`
  process.stderr.write(line)
  try { appendFileSync(LOG, line) } catch {}
}

// --- Unknown #3: what does the spawned subprocess actually see? --------------
// The design's default agent name is the cwd basename. That only works if the
// MCP subprocess inherits Claude Code's launch directory. Capture the truth.
const diagnostics = {
  captured_at: new Date().toISOString(),
  cwd: process.cwd(),
  cwd_basename: basename(process.cwd()),
  argv: process.argv,
  ppid: process.ppid,
  claude_env: Object.fromEntries(
    Object.entries(process.env).filter(([k]) => /^(CLAUDE|ANTHROPIC)/i.test(k)),
  ),
  has_CLAUDE_PROJECT_DIR: 'CLAUDE_PROJECT_DIR' in process.env,
}
writeFileSync(DIAG, JSON.stringify(diagnostics, null, 2))
log('probe starting; cwd=', diagnostics.cwd, 'CLAUDE_PROJECT_DIR=', String(diagnostics.has_CLAUDE_PROJECT_DIR))

// --- The channel declaration -------------------------------------------------
const mcp = new Server(
  { name: 'probe', version: '0.0.1' },
  {
    capabilities: {
      // This key is the whole experiment: its presence is what makes Claude Code
      // register a notification listener and treat this server as a channel.
      experimental: { 'claude/channel': {} },
      tools: {},
    },
    instructions:
      'This is a connectivity probe. Messages arrive as <channel source="probe" probe_id="...">. ' +
      'When one arrives, call probe_reply with the probe_id and a short acknowledgement, ' +
      'then tell the user in plain text what the channel tag contained. Do not modify any files.',
  },
)

mcp.setRequestHandler(ListToolsRequestSchema, async () => {
  log('tools/list requested')
  return {
    tools: [
      {
        name: 'probe_reply',
        description: 'Acknowledge a probe message received over the channel',
        inputSchema: {
          type: 'object',
          properties: {
            probe_id: { type: 'string', description: 'The probe_id from the channel tag' },
            text: { type: 'string', description: 'A short acknowledgement' },
          },
          required: ['probe_id', 'text'],
        },
      },
    ],
  }
})

mcp.setRequestHandler(CallToolRequestSchema, async req => {
  if (req.params.name === 'probe_reply') {
    const { probe_id, text } = req.params.arguments ?? {}
    log(`probe_reply called: probe_id=${probe_id} text=${JSON.stringify(text)}`)
    // Unknown #4: does this return value render in the human's terminal? If it
    // does, the real `send` tool can echo outbound text back into the transcript,
    // which Claude Code otherwise hides.
    return {
      content: [{
        type: 'text',
        text: `SENT_ECHO_MARKER probe_id=${probe_id} text=${JSON.stringify(text)}`,
      }],
    }
  }
  throw new Error(`unknown tool: ${req.params.name}`)
})

await mcp.connect(new StdioServerTransport())
log('mcp connected over stdio')

// --- Inbound: anything POSTed here gets pushed into the live session ---------
let nextId = 1
createServer((req, res) => {
  if (req.method !== 'POST') {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ ok: true, diagnostics }))
    return
  }
  let body = ''
  req.on('data', c => (body += c))
  req.on('end', async () => {
    const probe_id = String(nextId++)
    log(`emitting notification probe_id=${probe_id} body=${JSON.stringify(body)}`)
    try {
      await mcp.notification({
        method: 'notifications/claude/channel',
        params: {
          content: body,
          // meta keys must be identifiers: letters, digits, underscores only.
          meta: { probe_id, sent_at: String(Date.now()) },
        },
      })
      // The await resolves when the message hits the transport, NOT when Claude
      // processes it. A silent drop looks identical to success from here.
      log(`notification written to transport probe_id=${probe_id}`)
      res.writeHead(200); res.end('emitted\n')
    } catch (e) {
      log(`notification FAILED probe_id=${probe_id}: ${e?.message}`)
      res.writeHead(500); res.end(`failed: ${e?.message}\n`)
    }
  })
}).listen(PORT, '127.0.0.1', () => log(`http listening on 127.0.0.1:${PORT}`))
