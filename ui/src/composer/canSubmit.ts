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
export function canSubmit(name: string | null, text: string): boolean {
  if (!name) return false
  return text.trim().length > 0
}
