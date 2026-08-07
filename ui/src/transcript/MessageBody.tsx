import { parseBlocks, parseInline } from './markdown'
import styles from './Transcript.module.css'

function Inlines({ text }: { text: string }) {
  return (
    <>
      {parseInline(text).map((piece, i) => {
        if (piece.kind === 'code')
          return (
            <code key={i} className={styles.code}>
              {piece.text}
            </code>
          )
        if (piece.kind === 'bold') return <strong key={i}>{piece.text}</strong>
        return <span key={i}>{piece.text}</span>
      })}
    </>
  )
}

/// Every fragment below is a React element, so message text is escaped by
/// construction. There is no `dangerouslySetInnerHTML` here and there must never
/// be: the bus is unauthenticated, and this is where arbitrary text is rendered.
export function MessageBody({ body }: { body: string }) {
  return (
    <div className={styles.body}>
      {parseBlocks(body).map((block, i) => {
        if (block.kind === 'code') {
          return (
            <pre key={i} className={styles.pre}>
              <code>{block.text}</code>
            </pre>
          )
        }
        if (block.kind === 'ul' || block.kind === 'ol') {
          const List = block.kind === 'ul' ? 'ul' : 'ol'
          return (
            <List key={i} className={styles.list}>
              {block.items.map((item, j) => (
                <li key={j}>
                  <Inlines text={item} />
                </li>
              ))}
            </List>
          )
        }
        return (
          <p key={i} className={styles.para}>
            <Inlines text={block.text} />
          </p>
        )
      })}
    </div>
  )
}
