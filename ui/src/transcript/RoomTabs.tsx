import styles from './Files.module.css'

/// The count lives in the label so an empty room announces itself as empty
/// without being opened. `count === null` means the read failed — render no
/// count at all rather than `· 0`, which would claim the room has no files when
/// we do not know.
export function RoomTabs({
  view,
  onView,
  count,
}: {
  view: 'transcript' | 'files'
  onView: (v: 'transcript' | 'files') => void
  count: number | null
}) {
  return (
    <div className={styles.tabs}>
      <button
        className={view === 'transcript' ? styles.tabOn : styles.tab}
        onClick={() => onView('transcript')}
      >
        transcript
      </button>
      <button
        className={view === 'files' ? styles.tabOn : styles.tab}
        onClick={() => onView('files')}
      >
        {count === null ? 'files' : `files · ${count}`}
      </button>
    </div>
  )
}
