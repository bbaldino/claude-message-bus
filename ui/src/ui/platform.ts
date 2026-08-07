/// The handoff writes this chord as ⌘E throughout, which is wrong everywhere
/// except macOS — this bus commonly runs on Linux, where there is no Command key
/// and the glyph names something the reader cannot press. Detect once and label
/// what actually works.
///
/// `navigator.platform` is deprecated and frozen in some browsers, so this reads
/// `userAgentData` where it exists and falls back to the user-agent string. The
/// fallback's failure mode is benign: an undetected Mac gets Ctrl+E, which still
/// works there.
export function modKey() {
  const nav = typeof navigator === 'undefined' ? undefined : navigator
  const uaPlatform = (nav as { userAgentData?: { platform?: string } } | undefined)?.userAgentData
    ?.platform
  const isMac = /mac/i.test(uaPlatform ?? nav?.userAgent ?? '')
  return {
    label: isMac ? '⌘E' : 'Ctrl E',
    matches(e: { key: string; metaKey: boolean; ctrlKey: boolean }) {
      return e.key.toLowerCase() === 'e' && (isMac ? e.metaKey : e.ctrlKey)
    },
  }
}

/// True when focus is somewhere that swallows plain keystrokes. Shared by every
/// global shortcut, so none of them can steal a character from a text field.
export function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || el.isContentEditable
}
