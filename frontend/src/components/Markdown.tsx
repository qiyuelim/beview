// 轻量 Markdown 渲染器（无第三方依赖）：标题/粗斜体/行内代码/代码块/列表/引用/链接/段落。
import type { ReactNode } from 'react'
import '../markdown.css' // M8：.md 内容排版从 styles.css 抽出

// 行内解析：**粗** *斜* `码` [链接](url)
function inline(text: string): ReactNode[] {
  const out: ReactNode[] = []
  const re = /(\*\*[^*]+\*\*|\*[^*]+\*|`[^`]+`|\[[^\]]+\]\([^)]+\))/g
  let last = 0
  let i = 0
  let m: RegExpExecArray | null
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index))
    const tok = m[0]
    const key = i++
    if (tok.startsWith('**') && tok.endsWith('**')) out.push(<strong key={key}>{tok.slice(2, -2)}</strong>)
    else if (tok.startsWith('`') && tok.endsWith('`')) out.push(<code key={key}>{tok.slice(1, -1)}</code>)
    else if (tok.startsWith('*') && tok.endsWith('*')) out.push(<em key={key}>{tok.slice(1, -1)}</em>)
    else {
      const mm = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(tok)
      if (mm) {
        out.push(
          <a key={key} href={mm[2]} target="_blank" rel="noreferrer">
            {mm[1]}
          </a>,
        )
      } else {
        out.push(tok)
      }
    }
    last = m.index + tok.length
  }
  if (last < text.length) out.push(text.slice(last))
  return out
}

export default function Markdown({ text }: { text: string }) {
  const lines = text.split(/\r?\n/)
  const blocks: ReactNode[] = []
  let i = 0
  let k = 0

  while (i < lines.length) {
    const line = lines[i]
    const trim = line.trim()
    if (!trim) {
      i++
      continue
    }
    // 代码块
    if (trim.startsWith('```')) {
      const buf: string[] = []
      i++
      while (i < lines.length && !lines[i].trim().startsWith('```')) {
        buf.push(lines[i])
        i++
      }
      i++ // 跳过闭合围栏
      blocks.push(
        <pre key={k++}>
          <code>{buf.join('\n')}</code>
        </pre>,
      )
      continue
    }
    // 标题
    const h = /^(#{1,4})\s+(.*)$/.exec(trim)
    if (h) {
      const level = Math.min(h[1].length, 4)
      const Tag = (`h${level}`) as keyof JSX.IntrinsicElements
      blocks.push(<Tag key={k++}>{inline(h[2])}</Tag>)
      i++
      continue
    }
    // 引用
    if (/^>\s?/.test(trim)) {
      const buf: string[] = []
      while (i < lines.length && /^>\s?/.test(lines[i].trim())) {
        buf.push(lines[i].trim().replace(/^>\s?/, ''))
        i++
      }
      blocks.push(<blockquote key={k++}>{buf.join('\n')}</blockquote>)
      continue
    }
    // 列表
    const mu = /^[-*]\s+(.*)$/.exec(trim)
    const mo = /^\d+\.\s+(.*)$/.exec(trim)
    if (mu || mo) {
      const ordered = !!mo
      const items: ReactNode[] = []
      while (i < lines.length) {
        const l = lines[i].trim()
        if (!l) {
          i++
          break
        }
        const itemU = /^[-*]\s+(.*)$/.exec(l)
        const itemO = /^\d+\.\s+(.*)$/.exec(l)
        const hit = ordered ? itemO : itemU
        if (hit) {
          items.push(<li key={k++}>{inline(hit[1])}</li>)
          i++
        } else {
          break
        }
      }
      blocks.push(ordered ? <ol key={k++}>{items}</ol> : <ul key={k++}>{items}</ul>)
      continue
    }
    // 段落（聚合连续普通行）
    const buf: string[] = []
    while (i < lines.length) {
      const l = lines[i].trim()
      if (!l) break
      if (/^(#{1,4})\s|^```|^>\s?|^[-*]\s|^\d+\.\s/.test(l)) break
      buf.push(lines[i])
      i++
    }
    blocks.push(<p key={k++}>{inline(buf.join('\n'))}</p>)
  }

  return <div className="md">{blocks}</div>
}
