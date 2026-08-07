import { screen } from '@testing-library/react'
import { expect, test } from 'vitest'
import { renderWithStore } from './testing/fakeStore'
import { MainPlaceholder } from './Shell'

// Ordering matters here: `MainPlaceholder` must check `!rail` before it reads
// `rail.agents` — checking the other way round crashes on a null rail (and,
// short of crashing, would risk telling a populated bus's owner that nothing
// has joined it on every page load until the rail arrives).

test('a null rail renders neither the new-bus state nor the placeholder', () => {
  renderWithStore(<MainPlaceholder />, { rail: null })
  expect(screen.queryByTestId('main-placeholder')).toBeNull()
  expect(screen.queryByText('The bus is running. Nothing has joined it.')).toBeNull()
})

test('an empty rail renders the new-bus state, not the placeholder', () => {
  renderWithStore(<MainPlaceholder />, { rail: { rooms: [], agents: [] } })
  expect(screen.getByText('The bus is running. Nothing has joined it.')).toBeDefined()
  expect(screen.queryByTestId('main-placeholder')).toBeNull()
})

test('a populated rail renders the placeholder, not the new-bus state', () => {
  renderWithStore(<MainPlaceholder />, {
    rail: {
      rooms: [],
      agents: [
        {
          name: 'caas',
          host: 'h',
          version: '1',
          online: true,
          isHuman: false,
          lastSeen: 5,
          buckets: [1],
        },
      ],
    },
  })
  expect(screen.getByTestId('main-placeholder')).toBeDefined()
  expect(screen.queryByText('The bus is running. Nothing has joined it.')).toBeNull()
})
