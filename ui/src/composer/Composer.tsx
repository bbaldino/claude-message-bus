import { useState } from 'react'
import { useStore, store } from '../useStore'
import { canSubmit } from './canSubmit'
import { readSendAs, writeSendAs } from './identity'
import styles from './Composer.module.css'

/// The one control in this console that changes the bus rather than reading it.
///
/// EVERY PRECONDITION IS CHECKED INSIDE `submit`, not by disabling the button.
/// The delete modal shipped the inverse in a previous phase — the control was
/// correctly disabled while `Enter` called the action directly, and the action
/// carried only half the guard, so `Enter` deleted an agent the UI had already
/// refused to delete. Two paths reach one action here too. The precondition
/// itself lives in `canSubmit` (pure, tested directly) rather than inline here
/// — see its comment for why a component-level test of the `!name` branch
/// would be vacuous in this render structure.
export function Composer({ room }: { room: string }) {
  const { drafts, rail, sendAs } = useStore()
  const [name, setName] = useState(() => readSendAs())
  const [typedName, setTypedName] = useState('')
  const [done, setDone] = useState(false)

  const text = drafts[room] ?? ''

  const submit = () => {
    if (!canSubmit(name, text)) return
    void store.send(room, text, done)
    setDone(false)
  }

  const saveName = () => {
    const trimmed = typedName.trim()
    if (!trimmed) return
    writeSendAs(trimmed)
    setName(trimmed)
  }

  if (!name) {
    return (
      <div className={styles.composer}>
        <form
          className={styles.nameRow}
          onSubmit={(e) => {
            e.preventDefault()
            saveName()
          }}
        >
          <label className={styles.nameLabel} htmlFor="send-as">
            send as
          </label>
          <input
            id="send-as"
            aria-label="send as"
            className={styles.nameInput}
            value={typedName}
            onChange={(e) => setTypedName(e.target.value)}
          />
        </form>
      </div>
    )
  }

  const members = rail?.rooms.find((r) => r.name === room)?.members ?? []
  const online = members.filter((m) => rail?.agents.find((a) => a.name === m)?.online).length

  return (
    <div className={styles.composer}>
      <div className={styles.card}>
        <textarea
          aria-label="message"
          className={styles.input}
          rows={1}
          value={text}
          placeholder={`message ${room} as ${sendAs ?? name}…`}
          onChange={(e) => store.setDraft(room, e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== 'Enter' || e.shiftKey) return
            e.preventDefault()
            submit()
          }}
        />
      </div>
      <div className={styles.controls}>
        <label className={styles.markDone}>
          <input
            type="checkbox"
            aria-label="mark done"
            checked={done}
            onChange={(e) => setDone(e.target.checked)}
          />
          mark done
        </label>
        <span className={styles.preview}>
          delivers to {online}, queues for {members.length - online}
        </span>
        <button className={styles.send} onClick={submit}>
          send ⏎
        </button>
      </div>
    </div>
  )
}
