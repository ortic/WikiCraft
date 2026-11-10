import { useEffect, useState } from 'react'
import type { Editor } from '@tiptap/core'

/** Whether the editor is editable, kept current: pages turn editable after their first sync,
 *  and node views don't re-render on their own when that happens. */
export function useEditable(editor: Editor): boolean {
  const [editable, setEditable] = useState(editor.isEditable)
  useEffect(() => {
    const update = () => setEditable(editor.isEditable)
    update()
    editor.on('update', update)
    editor.on('transaction', update)
    return () => {
      editor.off('update', update)
      editor.off('transaction', update)
    }
  }, [editor])
  return editable
}
