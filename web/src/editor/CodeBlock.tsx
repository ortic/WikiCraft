// Code block with language picker and live Mermaid/diagram preview.

import { useEffect, useId, useState } from 'react'
import CodeBlockLowlight from '@tiptap/extension-code-block-lowlight'
import { NodeViewContent, NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from '@tiptap/react'
import { useEditable } from './useEditable'

const LANGUAGES = ['', 'bash', 'c', 'cpp', 'csharp', 'css', 'diff', 'go', 'graphql', 'html', 'java', 'javascript', 'json', 'kotlin', 'markdown', 'mermaid', 'php', 'plaintext', 'python', 'ruby', 'rust', 'scss', 'shell', 'sql', 'swift', 'typescript', 'xml', 'yaml']

let mermaidPromise: Promise<typeof import('mermaid').default> | null = null
function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then((m) => {
      const dark = document.documentElement.dataset.theme === 'dark' || (!document.documentElement.dataset.theme && matchMedia('(prefers-color-scheme: dark)').matches)
      m.default.initialize({ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default' })
      return m.default
    })
  }
  return mermaidPromise
}

function MermaidPreview({ code }: { code: string }) {
  const id = useId().replace(/[^a-zA-Z0-9]/g, '')
  const [svg, setSvg] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    const t = setTimeout(async () => {
      if (!code.trim()) return
      try {
        const mermaid = await loadMermaid()
        const { svg } = await mermaid.render(`mmd-${id}-${Date.now()}`, code)
        if (!cancelled) {
          setSvg(svg)
          setError(null)
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message.split('\n')[0] : 'Invalid diagram')
      }
    }, 300)
    return () => {
      cancelled = true
      clearTimeout(t)
    }
  }, [code, id])
  return (
    <div className="diagram-preview" contentEditable={false}>
      {error ? <div className="error-text small">Diagram error: {error}</div> : svg ? <div dangerouslySetInnerHTML={{ __html: svg }} /> : <div className="spinner" />}
    </div>
  )
}

function CodeBlockView({ node, updateAttributes, editor }: ReactNodeViewProps) {
  const language = (node.attrs.language as string | null) ?? ''
  const isDiagram = language === 'mermaid'
  const lineNumbers = !!node.attrs.lineNumbers
  const wrap = !!node.attrs.wrap
  const lines = Math.max(1, node.textContent.split('\n').length)
  const [copied, setCopied] = useState(false)
  const editable = useEditable(editor)
  const copy = async () => {
    try {
      await navigator.clipboard?.writeText(node.textContent)
      setCopied(true)
      setTimeout(() => setCopied(false), 1200)
    } catch {
      /* clipboard refused */
    }
  }
  return (
    <NodeViewWrapper className="code-block">
      <div className="code-toolbar" contentEditable={false}>
        {editable ? (
          <select value={language} onChange={(e) => updateAttributes({ language: e.target.value || null })} aria-label="Code language">
            {LANGUAGES.map((l) => (
              <option key={l} value={l}>
                {l || 'auto'}
              </option>
            ))}
          </select>
        ) : (
          <span>{language}</span>
        )}
        {editable && !isDiagram && (
          <>
            <button className={lineNumbers ? 'on' : ''} title="Line numbers" aria-pressed={lineNumbers} onClick={() => updateAttributes({ lineNumbers: !lineNumbers })}>
              1 2 3
            </button>
            <button className={wrap ? 'on' : ''} title="Wrap long lines" aria-pressed={wrap} onClick={() => updateAttributes({ wrap: !wrap })}>
              Wrap
            </button>
          </>
        )}
        <button className="copy-code" onClick={copy}>
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre className={`${lineNumbers ? 'with-lines' : ''} ${wrap ? 'wrap' : ''}`}>
        {lineNumbers && (
          <span className="code-gutter" contentEditable={false} aria-hidden>
            {Array.from({ length: lines }, (_, i) => (
              <span key={i}>{i + 1}</span>
            ))}
          </span>
        )}
        <NodeViewContent<'code'> as="code" />
      </pre>
      {isDiagram && <MermaidPreview code={node.textContent} />}
    </NodeViewWrapper>
  )
}

export const CodeBlock = CodeBlockLowlight.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      lineNumbers: { default: false, rendered: false },
      wrap: { default: false, rendered: false },
    }
  },
  addNodeView() {
    return ReactNodeViewRenderer(CodeBlockView)
  },
})
