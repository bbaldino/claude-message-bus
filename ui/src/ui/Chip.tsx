import type { ReactNode } from 'react'
import styles from './Chip.module.css'

export type ChipTone = 'human' | 'attention' | 'destructive' | 'presence'

export function Chip({
  tone,
  size = 'sm',
  children,
}: {
  tone: ChipTone
  size?: 'sm' | 'md'
  children: ReactNode
}) {
  return <span className={`${styles.chip} ${styles[size]} ${styles[tone]}`}>{children}</span>
}
