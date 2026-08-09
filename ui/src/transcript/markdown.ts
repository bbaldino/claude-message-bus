export type Block =
  | { kind: 'p'; text: string }
  | { kind: 'code'; text: string; lang: string }
  | { kind: 'ul'; items: string[] }
  | { kind: 'ol'; items: string[] }

export type Inline = { kind: 'text' | 'code' | 'bold'; text: string }

/// A deliberately small subset: paragraphs, fenced code, lists, inline code and
/// bold. Agent prose really does contain all of these.
///
/// The governing rule is that anything unmatched falls through as literal text.
/// An unclosed fence, a stray backtick, a link — each renders as what the sender
/// typed. That is what makes shipping an incomplete parser safe: it can look
/// plain, but it can never lose a message.
export function parseBlocks(text: string): Block[] {
  // Agents run on any platform; normalize CRLF to LF so that trailing \r does not
  // defeat the list regexes or the exact-match fence comparison.
  const lines = text.replace(/\r\n/g, '\n').split('\n')
  const blocks: Block[] = []
  let para: string[] = []

  const flush = () => {
    if (para.length) blocks.push({ kind: 'p', text: para.join('\n') })
    para = []
  }

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]

    if (line.startsWith('```')) {
      const close = lines.indexOf('```', i + 1)
      if (close === -1) {
        // Unclosed: not a code block at all. Treat as ordinary text.
        para.push(line)
        continue
      }
      flush()
      blocks.push({
        kind: 'code',
        lang: line.slice(3).trim(),
        text: lines.slice(i + 1, close).join('\n'),
      })
      i = close
      continue
    }

    const bullet = /^[-*]\s+(.*)$/.exec(line)
    const numbered = /^\d+\.\s+(.*)$/.exec(line)
    if (bullet || numbered) {
      flush()
      const kind = bullet ? 'ul' : 'ol'
      const items: string[] = [(bullet ?? numbered)![1]]
      while (i + 1 < lines.length) {
        const next = bullet
          ? /^[-*]\s+(.*)$/.exec(lines[i + 1])
          : /^\d+\.\s+(.*)$/.exec(lines[i + 1])
        if (!next) break
        items.push(next[1])
        i++
      }
      blocks.push({ kind, items })
      continue
    }

    if (line.trim() === '') flush()
    else para.push(line)
  }

  flush()
  return blocks
}

/// Single-pass, non-nesting. Nesting is where hand-rolled inline parsers get
/// gnarly, and a monitoring transcript does not need bold-inside-code.
export function parseInline(text: string): Inline[] {
  const out: Inline[] = []
  let buf = ''
  let i = 0

  const push = (kind: Inline['kind'], t: string) => {
    if (kind === 'text') {
      buf += t
      return
    }
    if (buf) out.push({ kind: 'text', text: buf })
    buf = ''
    out.push({ kind, text: t })
  }

  while (i < text.length) {
    if (text[i] === '`') {
      const end = text.indexOf('`', i + 1)
      if (end > i + 1) {
        push('code', text.slice(i + 1, end))
        i = end + 1
        continue
      }
    }
    if (text.startsWith('**', i)) {
      const end = text.indexOf('**', i + 2)
      if (end > i + 2) {
        push('bold', text.slice(i + 2, end))
        i = end + 2
        continue
      }
    }
    buf += text[i]
    i++
  }

  if (buf) out.push({ kind: 'text', text: buf })
  return out
}
