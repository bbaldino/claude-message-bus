import { render } from '@testing-library/react'
import { expect, test } from 'vitest'
import { VolumeStrip } from './VolumeStrip'
import styles from './VolumeStrip.module.css'

test('renders one bar per bucket', () => {
  const { container } = render(<VolumeStrip buckets={[0, 1, 2, 3]} variant="rail" />)
  expect(container.querySelectorAll(`.${styles.volumeBar}`)).toHaveLength(4)
})

test('an empty bucket still renders a visible tick, not a gap', () => {
  // The 8% floor is the whole point: a flat strip must read as "quiet", not as
  // "broken layout".
  const { container } = render(<VolumeStrip buckets={[0, 0, 0]} variant="rail" />)
  const bars = container.querySelectorAll<HTMLElement>(`.${styles.volumeBar}`)
  for (const bar of bars) {
    expect(bar.style.height).toBe('8%')
  }
})

test('the tallest bucket reaches full height and others scale against it', () => {
  const { container } = render(<VolumeStrip buckets={[0, 5, 10]} variant="rail" />)
  const bars = container.querySelectorAll<HTMLElement>(`.${styles.volumeBar}`)
  expect(bars[2].style.height).toBe('100%')
  expect(bars[1].style.height).toBe('50%')
})

test('an all-zero strip is labelled as silent for screen readers', () => {
  const { container } = render(<VolumeStrip buckets={[0, 0]} variant="rail" />)
  expect(container.querySelector(`.${styles.volumeStrip}`)?.getAttribute('aria-label')).toBe(
    'no messages in the last 10 min',
  )
})
