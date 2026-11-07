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
  /** The process exit code; set on an error in `--json` (UX U9). */
  exitCode?: number
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
  const more = continuation(result.data, format.line)
  if (more !== undefined) lines.push(more)
  if (format.footer !== undefined) lines.push(format.footer)
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
  /** The line that ran; a page's `nextCursor` turns it into the next one. */
  line?: string
  /** A last line for the reader (the help list's note on CLI options). */
  footer?: string
}

/** "devamı: gecko <line> --cursor N" for a page that has a `nextCursor`. */
function continuation(
  data: unknown,
  line: string | undefined
): string | undefined {
  if (line === undefined || !isRow(data)) return undefined
  const next = data.nextCursor
  if (typeof next !== 'string' && typeof next !== 'number') return undefined
  const base = line
    .replace(/\s--cursor(?:=|\s+)(?:"[^"]*"|'[^']*'|\S+)/g, '')
    .trim()
  return `devamı: gecko ${escapeControls(base)} --cursor ${escapeControls(String(next))}`
}

interface SchemaField {
  type?: unknown
  items?: unknown
  enum?: unknown
  minimum?: unknown
  maximum?: unknown
  minItems?: unknown
  maxItems?: unknown
  description?: unknown
}

type Schema = { properties: Record<string, SchemaField>; required?: unknown }

function isSchema(value: unknown): value is Schema {
  return (
    isRow(value) &&
    isRow(value.properties) &&
    Object.values(value.properties).every(isRow)
  )
}

/** Splits text into lines of at most `max` code points at spaces. */
function wrap(text: string, max: number): string[] {
  const out: string[] = []
  let current = ''
  for (const word of text.split(/\s+/).filter(Boolean)) {
    if (current !== '' && width(current) + 1 + width(word) > max) {
      out.push(current)
      current = word
    } else {
      current = current === '' ? word : `${current} ${word}`
    }
  }
  if (current !== '') out.push(current)
  return out
}

/**
 * A command's `input` schema as one option per line: `--ad <tip>`, whether it
 * is required, enum/min/max/maxItems, then the description wrapped to the
 * terminal. An array option may repeat (`--source <string>…`).
 */
function schemaLines(schema: Schema, columns: number | undefined): string {
  const required = new Set(
    Array.isArray(schema.required) ? (schema.required as unknown[]) : []
  )
  const lines: string[] = []
  for (const [name, field] of Object.entries(schema.properties)) {
    const items = isRow(field.items) ? (field.items as SchemaField) : undefined
    const array = field.type === 'array'
    const rawType = array ? items?.type : field.type
    const type = Array.isArray(rawType)
      ? rawType.join('|')
      : typeof rawType === 'string'
        ? rawType
        : array
          ? 'string'
          : 'any'
    const parts = [
      `--${escapeControls(name)} <${escapeControls(type)}>${array ? '…' : ''}`,
      required.has(name) ? 'zorunlu' : 'isteğe bağlı'
    ]
    const enumValues = Array.isArray(field.enum)
      ? field.enum
      : Array.isArray(items?.enum)
        ? items.enum
        : undefined
    if (enumValues) {
      parts.push(
        `enum: ${enumValues.map(v => escapeControls(String(v))).join('|')}`
      )
    }
    if (field.minimum !== undefined) parts.push(`min ${String(field.minimum)}`)
    if (field.maximum !== undefined) parts.push(`max ${String(field.maximum)}`)
    if (field.minItems !== undefined) {
      parts.push(`minItems ${String(field.minItems)}`)
    }
    if (field.maxItems !== undefined) {
      parts.push(`maxItems ${String(field.maxItems)}`)
    }
    lines.push(`  ${parts.join(' · ')}`)
    if (typeof field.description === 'string' && field.description !== '') {
      const text = escapeControls(field.description)
      const wrapped =
        columns !== undefined && columns > 10
          ? wrap(text, Math.max(20, columns - 6))
          : [text.replace(/\s+/g, ' ')]
      for (const row of wrapped) lines.push(`      ${row}`)
    }
  }
  return lines.length > 0 ? lines.join('\n') : '  (alan yok)'
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
      if (key === 'input' && isSchema(value)) {
        fields.push(`input:\n${schemaLines(value, options.columns)}`)
      } else if (isRows(value)) {
        tables.push(`${escapeControls(key)}:\n${table(value, options.columns)}`)
      } else if (Array.isArray(value) && value.length === 0) {
        fields.push(`${escapeControls(key)}: (boş)`)
      } else {
        fields.push(
          `${escapeControls(key)}: ${cut(flat(shown(key, value)), valueMax)}`
        )
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

/**
 * Rows under a header (UX U6): columns empty in every row are hidden and
 * named; id columns are never cut, and when they take more than half of the
 * terminal the rows go vertical (`field  value` blocks); other cells are cut
 * at `cellMax`; the columns that do not fit are named.
 */
function grid(
  allColumns: string[],
  allCells: unknown[][],
  columnsMax?: number
): string {
  const kept = allColumns
    .map((_, i) => i)
    .filter(i => allCells.some(row => !isEmpty(row[i])))
  const hidden = allColumns.filter((_, i) => !kept.includes(i))
  const columns = kept.map(i => allColumns[i])
  const values = allCells.map(row =>
    kept.map(i => shown(allColumns[i], row[i]))
  )
  const ids = columns.map((column, i) =>
    isIdColumn(
      column,
      values.map(row => row[i])
    )
  )
  const note =
    hidden.length > 0
      ? [`(boş sütunlar gizlendi: ${hidden.map(escapeControls).join(', ')})`]
      : []
  if (columns.length === 0) {
    return ['(boş)', ...note].join('\n')
  }
  const texts = values.map(row =>
    row.map((value, i) => (ids[i] ? cell(value) : cut(cell(value), cellMax)))
  )
  const heads = columns.map((column, i) =>
    ids[i] ? escapeControls(column) : cut(escapeControls(column), cellMax)
  )
  const widths = columns.map((_, i) =>
    Math.max(width(heads[i]), ...texts.map(row => width(row[i])))
  )
  if (columnsMax !== undefined && columnsMax > 0) {
    const idWidth = widths
      .filter((_, i) => ids[i])
      .reduce((sum, w, n) => sum + w + (n > 0 ? 2 : 0), 0)
    if (idWidth > columnsMax / 2) {
      return [...vertical(heads, texts), ...note].join('\n')
    }
  }
  // Keep the first columns that fit the terminal; the rest is in --json.
  let count = columns.length
  if (columnsMax !== undefined && columnsMax > 0) {
    let used = 0
    count = 0
    for (const w of widths) {
      const next = used + (count > 0 ? 2 : 0) + w
      if (next > columnsMax && count > 0) break
      used = next
      count++
    }
  }
  const line = (cells: string[]) =>
    cells
      .slice(0, count)
      .map((text, i) => text + ' '.repeat(widths[i] - width(text)))
      .join('  ')
      .trimEnd()
  const lines = [
    line(heads),
    line(widths.map(w => '-'.repeat(w))),
    ...texts.map(line)
  ]
  if (count < columns.length) {
    lines.push(
      `(+${columns.length - count} sütun sığmadı: ${columns.slice(count).join(', ')}; tamamı için --json)`
    )
  }
  return [...lines, ...note].join('\n')
}

/** Each row as a block of `field  value` lines, a blank line between. */
function vertical(heads: string[], rows: string[][]): string[] {
  const pad = Math.max(...heads.map(width))
  const lines: string[] = []
  rows.forEach((row, n) => {
    if (n > 0) lines.push('')
    row.forEach((text, i) => {
      if (text === '') return
      lines.push(`${heads[i]}${' '.repeat(pad - width(heads[i]))}  ${text}`)
    })
  })
  return lines
}

/** Duration fields by name (UX U6); `startMs` and `endMs` are epochs. */
export const durationFields = new Set([
  'durationMs',
  'runningMs',
  'ageMs',
  'snapshotAgeMs',
  'dataAgeMs',
  'elapsedMs',
  'uptimeMs'
])

/** 454558943 → "5 g 6 sa": the two largest units from the first one. */
export function humanDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`
  const units: [string, number][] = [
    ['g', 86_400_000],
    ['sa', 3_600_000],
    ['dk', 60_000],
    ['sn', 1000]
  ]
  let rest = Math.floor(ms)
  const first = units.findIndex(([, size]) => rest >= size)
  const parts: string[] = []
  for (const [name, size] of units.slice(first, first + 2)) {
    const count = Math.floor(rest / size)
    rest -= count * size
    if (count > 0) parts.push(`${count} ${name}`)
  }
  return parts.join(' ')
}

/** A field's value for a reader: a known duration in words, else as is. */
function shown(key: string, value: unknown): unknown {
  return durationFields.has(key) &&
    typeof value === 'number' &&
    Number.isFinite(value) &&
    value >= 0
    ? humanDuration(value)
    : value
}

/**
 * A table cell (UX U6): an array is `[n]`, or its members' `title`, `name`
 * or `value` joined when every member has one; an object is `{…}`.
 */
function cell(value: unknown): string {
  if (Array.isArray(value)) {
    for (const key of ['title', 'name', 'value']) {
      const named =
        value.length > 0 &&
        value.every(
          item =>
            isRow(item) &&
            (typeof item[key] === 'string' || typeof item[key] === 'number')
        )
      if (named) {
        return flat(
          value
            .map(item => String((item as Record<string, unknown>)[key]))
            .join(', ')
        )
      }
    }
    return `[${value.length}]`
  }
  if (isRow(value)) return '{…}'
  return flat(value)
}

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** `id`, `workId`, `…Id` or a column of UUIDs: never cut (UX U6). */
function isIdColumn(name: string, values: unknown[]): boolean {
  if (name === 'id' || /Id$/.test(name)) return true
  const present = values.filter(v => typeof v === 'string' && v !== '')
  return present.length > 0 && present.every(v => uuid.test(v as string))
}

/** `null`, `""` and a missing field are empty; `0` and `false` are not. */
function isEmpty(value: unknown): boolean {
  return value === null || value === undefined || value === ''
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
