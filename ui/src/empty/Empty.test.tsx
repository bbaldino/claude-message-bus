import { screen } from '@testing-library/react'
import { expect, test } from 'vitest'
import { renderWithStore } from '../testing/fakeStore'
import { NewBus } from './NewBus'

test('states what is true and what to run', () => {
  renderWithStore(<NewBus />, { rail: { rooms: [], agents: [] } })
  expect(screen.getByText('The bus is running. Nothing has joined it.')).toBeDefined()
  // The command must name a real subcommand. `claude-bus register` does not
  // exist; `init` writes the MCP config and the agent registers on session start.
  const cmd = screen.getByTestId('command').textContent ?? ''
  expect(cmd).toMatch(/^claude-bus init /)
  expect(cmd).not.toMatch(/register/)
  // The scope must be named. Without it `init` asks, and a blank answer selects
  // USER scope — configuring every project on the machine, the opposite of the
  // "any project directory" this screen tells the reader to run it in.
  expect(cmd).toContain('--project')
})

test('the command carries the address the page was actually served from', () => {
  // location.host is provably reachable — the reader is looking at a page served
  // from it. The server's own hostname may not resolve from where they are.
  renderWithStore(<NewBus />, { rail: { rooms: [], agents: [] } })
  expect(screen.getByTestId('command').textContent).toContain(location.host)
})
