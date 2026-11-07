// What the CLI prints (§6): the result on stdout, everything else on stderr.
// `--json` is one versioned object with every non-ASCII character escaped,
// so it survives any code page and PowerShell 5.1's redirection (DC-3).

/** The exit codes of §6.4. */
export const exitCodes = {
  ok: 0,
  failed: 1,
  usage: 2,
  denied: 3,
  notRun: 4,
  unknown: 5,
  auth: 6,
  interrupted: 130
} as const

const byCode: Record<string, number> = {
  failed: 1,
  'target-not-found': 1,
  timeout: 1,
  'id-conflict': 1,
  'invalid-args': 2,
  'unknown-command': 2,
  'too-large': 2,
  'unsupported-version': 2,
  usage: 2,
  'not-allowed': 3,
  'no-terminal-access': 3,
  'pair-timeout': 3,
  busy: 4,
  'not-delivered': 4,
  'tab-unreachable': 4,
  gone: 4,
  'clock-skew': 4,
  expired: 4,
  'no-binding': 4,
  'connection-lost': 4,
  unreachable: 4,
  'invalid-code': 4,
  'other-account': 4,
  unknown: 5,
  'unknown-outcome': 5,
  'not-logged-in': 6,
  unauthorized: 6,
  'token-expired': 6,
  revoked: 6,
  cancelled: 130,
  'cancelled-before-start': 130,
  'cancel-requested': 130
}

/** The exit code of an error code; 1 for one this table does not know. */
export function exitCodeOf(code: string): number {
  return byCode[code] ?? 1
}

/** One result, as `--json` writes it (§6.5). */
export interface Result {
  v: 1
  status: 'ok' | 'error' | 'unknown' | 'cancelled'
  code?: string
  message?: string
  reason?: string
  summary?: string
  data?: unknown
  view?: { id: string; title?: string }
  hidden?: boolean
  dataAgeMs?: number | null
  preview: boolean
  suggest?: string
  complete: boolean
  durationMs?: number
  untrusted: string[]
}

export function errorResult(
  code: string,
  message: string,
  extra: Partial<Result> = {}
): Result {
  return {
    v: 1,
    status:
      exitCodeOf(code) === 5
        ? 'unknown'
        : exitCodeOf(code) === 130
          ? 'cancelled'
          : 'error',
    code,
    message,
    preview: false,
    complete: exitCodeOf(code) !== 5,
    untrusted: [],
    ...extra
  }
}

/** The exit code of a result. */
export function exitOf(result: Result): number {
  if (result.status === 'ok') {
    return 0
  }
  if (result.status === 'cancelled') {
    return 130
  }
  return exitCodeOf(result.code ?? 'failed')
}

/** Lists the free-text fields that come from user input or data (§6.5). */
export function markUntrusted(result: Result, messageFromTab: boolean): Result {
  const untrusted: string[] = []
  if (result.summary !== undefined) untrusted.push('summary')
  if (result.data !== undefined) untrusted.push('data')
  // A message the CLI wrote is its own; one from the tab may quote input.
  if (result.message !== undefined && messageFromTab) untrusted.push('message')
  if (result.view?.title !== undefined) untrusted.push('view.title')
  return { ...result, untrusted }
}

/** JSON with every character outside ASCII as `\uXXXX`. */
export function asciiJson(value: unknown): string {
  return JSON.stringify(value).replace(
    /[\u007f-￿]/g,
    char => `\\u${char.charCodeAt(0).toString(16).padStart(4, '0')}`
  )
}

/**
 * Text that cannot drive the terminal: C0 and C1 control characters other
 * than line breaks and tabs are written as `\xHH` (§6.7).
 */
export function escapeControls(text: string): string {
  return text.replace(
    // eslint-disable-next-line no-control-regex
    /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g,
    char => `\\x${char.charCodeAt(0).toString(16).padStart(2, '0')}`
  )
}

/** The plain text of a result for stdout, empty for an error. */
export function plainText(result: Result, format: FormatOptions = {}): string {
  if (result.status !== 'ok') {
    return ''
  }
  const lines: string[] = []
  if (result.preview) {
    const head = `Önizleme: ${escapeControls(result.summary ?? '')}`
    lines.push(
      result.suggest
        ? `${head}. Çalıştırmak için: gecko ${escapeControls(result.suggest)}`
        : head
    )
  } else if (result.summary !== undefined) {
    lines.push(escapeControls(result.summary))
  }
  if (result.data !== undefined) {
    lines.push(formatData(result.data, format))
  }
  if (!result.complete) {
    lines.push('(sonuç eksik geldi; veri basılmadı)')
  }
  return lines.length > 0 ? `${lines.join('\n')}\n` : ''
}

/** The longest cell of a table, in code points. */
export const cellMax = 40
/** The longest value of a `key: value` line, in code points. */
const valueMax = 120
/** How many first rows give a table its columns. */
const columnRows = 20

/** Cuts text to `max` code points, the last one an ellipsis. */
function cut(text: string, max: number): string {
  const points = Array.from(text)
  return points.length <= max ? text : `${points.slice(0, max - 1).join('')}…`
}

function flat(value: unknown): string {
  if (value === null || value === undefined) {
    return ''
  }
  return escapeControls(
    typeof value === 'string' ? value : JSON.stringify(value)
  ).replace(/\s+/g, ' ')
}

function isRow(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isRows(value: unknown): value is Record<string, unknown>[] {
  return Array.isArray(value) && value.length > 0 && value.every(isRow)
}

export interface FormatOptions {
  /** The terminal's width; the table drops the columns that do not fit. */
  columns?: number
}

/**
 * `data` for a reader (§6): an array of rows, or an object's arrays of rows
 * (`items`), as tables; an object's `columns` + `rows` grid (`dataview.rows`)
 * as one table headed by `columns`; other fields as `key: value`. `--json`
 * keeps it all.
 */
export function formatData(data: unknown, options: FormatOptions = {}): string {
  if (isRows(data)) {
    return table(data, options.columns)
  }
  if (Array.isArray(data)) {
    return data.length === 0
      ? '(boş)'
      : data.map(item => `- ${cut(flat(item), valueMax)}`).join('\n')
  }
  if (isRow(data)) {
    const fields: string[] = []
    const tables: string[] = []
    const gridded = isGrid(data)
    if (gridded) {
      tables.push(grid(data.columns, data.rows, options.columns))
    }
    for (const [key, value] of Object.entries(data)) {
      if (gridded && (key === 'columns' || key === 'rows')) {
        continue
      }
      if (isRows(value)) {
        tables.push(`${escapeControls(key)}:\n${table(value, options.columns)}`)
      } else if (Array.isArray(value) && value.length === 0) {
        fields.push(`${escapeControls(key)}: (boş)`)
      } else {
        fields.push(`${escapeControls(key)}: ${cut(flat(value), valueMax)}`)
      }
    }
    return [...fields, ...tables].join('\n')
  }
  if (typeof data === 'string') {
    return escapeControls(data)
  }
  return flat(data)
}

/** `{ columns: string[], rows: unknown[][] }`: a table sent as a grid. */
function isGrid(
  value: Record<string, unknown>
): value is { columns: string[]; rows: unknown[][] } {
  return (
    Array.isArray(value.columns) &&
    value.columns.every(column => typeof column === 'string') &&
    Array.isArray(value.rows) &&
    value.rows.every(row => Array.isArray(row))
  )
}

function table(data: Record<string, unknown>[], columnsMax?: number): string {
  const columns: string[] = []
  for (const row of data.slice(0, columnRows)) {
    for (const key of Object.keys(row)) {
      if (!columns.includes(key)) columns.push(key)
    }
  }
  return grid(
    columns,
    data.map(row => columns.map(column => row[column])),
    columnsMax
  )
}

function grid(
  columns: string[],
  cells: unknown[][],
  columnsMax?: number
): string {
  const heads = columns.map(column => cut(escapeControls(column), cellMax))
  const rows = cells.map(row =>
    columns.map((_, i) => cut(flat(row[i]), cellMax))
  )
  const widths = columns.map((_, i) =>
    Math.max(width(heads[i]), ...rows.map(row => width(row[i])))
  )
  // Keep the first columns that fit the terminal; the rest is in --json.
  let shown = columns.length
  if (columnsMax !== undefined && columnsMax > 0) {
    let used = 0
    shown = 0
    for (const w of widths) {
      const next = used + (shown > 0 ? 2 : 0) + w
      if (next > columnsMax && shown > 0) break
      used = next
      shown++
    }
  }
  const line = (cells: string[]) =>
    cells
      .slice(0, shown)
      .map((text, i) => text + ' '.repeat(widths[i] - width(text)))
      .join('  ')
      .trimEnd()
  const lines = [
    line(heads),
    line(widths.map(w => '-'.repeat(w))),
    ...rows.map(line)
  ]
  if (shown < columns.length) {
    lines.push(
      `(+${columns.length - shown} sütun sığmadı: ${columns.slice(shown).join(', ')}; tamamı için --json)`
    )
  }
  return lines.join('\n')
}

function width(text: string): number {
  return Array.from(text).length
}

/** Whether a stream may be coloured (TTY, no `NO_COLOR`). */
export function colorOn(stream: NodeJS.WriteStream): boolean {
  return Boolean(stream.isTTY) && !('NO_COLOR' in process.env)
}

export function paint(
  stream: NodeJS.WriteStream,
  tone: 'error' | 'info' | 'ok',
  text: string
): string {
  if (!colorOn(stream)) {
    return text
  }
  const code = tone === 'error' ? 31 : tone === 'ok' ? 32 : 2
  return `\u001b[${code}m${text}\u001b[0m`
}

/** Writes and resolves once the stream took it (stdout drains before exit). */
export function write(stream: NodeJS.WritableStream, text: string) {
  return new Promise<void>((resolve, reject) => {
    if (text === '') {
      resolve()
      return
    }
    stream.write(text, error => (error ? reject(error) : resolve()))
  })
}
