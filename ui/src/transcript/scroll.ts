export type Measurements = { scrollTop: number; scrollHeight: number; clientHeight: number }

/// jsdom has no layout — `scrollHeight` and `clientHeight` are always zero there
/// — so a test that drives the DOM would pass vacuously. Keeping the decision
/// pure is what makes it testable at all; the wiring around it stays thin and is
/// covered by the manual pass.
const BOTTOM_SLACK = 4
const TOP_SLACK = 80

export function scrollAction(m: Measurements & { grew: boolean }): 'pin' | 'notify' | 'none' {
  if (!m.grew) return 'none'
  const distanceFromBottom = m.scrollHeight - m.clientHeight - m.scrollTop
  return distanceFromBottom <= BOTTOM_SLACK ? 'pin' : 'notify'
}

export function shouldLoadOlder(m: Measurements): boolean {
  return m.scrollTop <= TOP_SLACK
}
