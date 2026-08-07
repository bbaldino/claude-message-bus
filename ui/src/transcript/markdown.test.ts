import { expect, test } from 'vitest'
import { parseBlocks, parseInline } from './markdown'

test('splits paragraphs on blank lines', () => {
  expect(parseBlocks('one\n\ntwo')).toEqual([
    { kind: 'p', text: 'one' },
    { kind: 'p', text: 'two' },
  ])
})

test('keeps a fenced code block verbatim, including blank lines inside it', () => {
  const src = '```\na\n\nb\n```'
  expect(parseBlocks(src)).toEqual([{ kind: 'code', text: 'a\n\nb', lang: '' }])
})

test('captures a fence language', () => {
  expect(parseBlocks('```yaml\nkey: 1\n```')).toEqual([
    { kind: 'code', text: 'key: 1', lang: 'yaml' },
  ])
})

test('an unclosed fence renders as literal text rather than swallowing the rest', () => {
  // The whole point of the fallthrough rule: an incomplete construct must not
  // eat the message.
  expect(parseBlocks('```\nstill talking')).toEqual([{ kind: 'p', text: '```\nstill talking' }])
})

test('parses bullet and numbered lists', () => {
  expect(parseBlocks('- a\n- b')).toEqual([{ kind: 'ul', items: ['a', 'b'] }])
  expect(parseBlocks('1. a\n2. b')).toEqual([{ kind: 'ol', items: ['a', 'b'] }])
})

test('parses inline code and bold', () => {
  expect(parseInline('use `x` and **y**')).toEqual([
    { kind: 'text', text: 'use ' },
    { kind: 'code', text: 'x' },
    { kind: 'text', text: ' and ' },
    { kind: 'bold', text: 'y' },
  ])
})

test('unmatched inline markers stay literal', () => {
  expect(parseInline('a `b')).toEqual([{ kind: 'text', text: 'a `b' }])
  expect(parseInline('2 ** 3')).toEqual([{ kind: 'text', text: '2 ** 3' }])
})

test('constructs we deliberately do not support pass through as text', () => {
  // Links, italics, tables and raw HTML are out of scope; each must render as
  // what the sender typed, never be dropped.
  expect(parseInline('[a](b)')).toEqual([{ kind: 'text', text: '[a](b)' }])
  expect(parseInline('_a_')).toEqual([{ kind: 'text', text: '_a_' }])
  expect(parseInline('<b>hi</b>')).toEqual([{ kind: 'text', text: '<b>hi</b>' }])
})
