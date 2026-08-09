/// The precondition for `Composer`'s `submit()`, pulled out pure so it can be
/// tested directly rather than through the DOM. Same reason `scroll.ts` holds
/// its decisions as pure functions: jsdom can't drive the real render-branch
/// path that makes the `!name` case unreachable today (there is no message
/// control mounted at all until a name is set, so nothing can invoke `submit`
/// while `name` is falsy) — a component test that fires `Enter` at the
/// name field to "prove" the guard would pass whether or not the guard
/// exists, because it never reaches `submit` either way. Keeping the
/// predicate here, independent of that structure, is what still catches a
/// regression if a future refactor keeps the message control mounted (hidden
/// rather than absent) or animates the name -> message transition, either of
/// which would make the `!name` path reachable again.
///
/// `sending` is the third precondition the spec names — "no send already in
/// flight" — alongside the name and the text. It is not reachable today
/// either: `store.send` clears the draft synchronously before its first
/// `await`, so a second `Enter` before the first send settles sees an empty
/// draft and fails the text check regardless. `canSubmit` is nonetheless the
/// named home of every precondition, not just the two that happen to be load-
/// bearing under the current implementation of `store.send`.
export function canSubmit(name: string | null, text: string, sending: boolean): boolean {
  if (!name) return false
  if (sending) return false
  return text.trim().length > 0
}
