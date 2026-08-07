import type { RoomFile } from '../types/RoomFile'
import { age } from '../ui/time'
import styles from './Files.module.css'

/// Bytes at the precision a reader scans for, not the precision a machine
/// stores. Sizes are a comparison target in this table, not a value to quote.
function bytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

export function FilesTable({ files, now }: { files: RoomFile[]; now: number }) {
  if (files.length === 0) {
    return <p className={styles.empty}>No files in this room.</p>
  }
  return (
    <div className={styles.table} data-testid="files-table">
      <div className={styles.head}>key</div>
      <div className={`${styles.head} ${styles.right}`}>size</div>
      <div className={styles.head}>uploaded by</div>
      <div className={`${styles.head} ${styles.right}`}>when</div>
      {files.map((f) => (
        <div key={f.key} className={styles.row}>
          <div className={styles.keyCell}>
            <span className={styles.key}>{f.key}</span>
            {/* Content type rides under the key: it is secondary to what you
                scan for, which is the key, the size and who put it there. */}
            {f.contentType && <span className={styles.contentType}>{f.contentType}</span>}
          </div>
          <div className={`${styles.size} ${styles.right}`}>{bytes(f.size)}</div>
          <div className={styles.uploader}>{f.updatedBy}</div>
          <div className={`${styles.when} ${styles.right}`}>{age(f.updatedAt, now)} ago</div>
        </div>
      ))}
    </div>
  )
}
