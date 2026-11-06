// The DC-3 matrix on this machine: the same arguments through Windows
// PowerShell 5.1, PowerShell 7 and cmd (or bash elsewhere) to a fake tab, by
// argv and by `--line -`, and the `--json` output redirected to a file.
// Needs `pnpm build`. Run: pnpm test:args

import { spawn } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { createConsole } from '@dolusoft/command-console-core'

import { startDevHub } from '../dist/hub/index.js'
import { connectTab, createTabGate } from '../dist/tab/index.js'

const root = fileURLToPath(new URL('..', import.meta.url))
const bin = join(root, 'bin', 'gecko.js')
const node = process.execPath
const expected = ['a b', '', 'x"y', 'c:\\yol\\', 'ığ']

const dir = mkdtempSync(join(tmpdir(), 'gecko-args-'))
const env = {
  ...process.env,
  GECKO_CONFIG_DIR: dir,
  GECKO_BROWSER: 'none',
  NO_COLOR: '1'
}

const hub = await startDevHub({
  port: 0,
  autoApprove: true,
  ratePerSecond: 1000,
  timings: { intervalMs: 50 }
})
let echoed
const commands = [
  {
    name: 'echo',
    danger: 'read',
    input: {
      type: 'object',
      properties: { text: { type: 'array', items: { type: 'string' } } }
    },
    run: request => {
      echoed = request.input?.text ?? []
      return { summary: 'ığüşöç ✓', data: echoed }
    }
  }
]
const tab = await connectTab({ hubUrl: hub.url, token: 'dev:u1', pageId: 'p1' })
const console_ = createConsole({
  gate: createTabGate(tab, 'p1'),
  now: Date.now,
  commands
})

// Async: the hub runs in this process and must keep answering.
const exited = child =>
  new Promise((resolve, reject) =>
    child.on('exit', code =>
      code === 0 ? resolve() : reject(new Error(`exit ${code}`))
    )
  )
await exited(
  spawn(node, [bin, 'login', '--env', hub.env], { env, stdio: 'ignore' })
)
// Pair through `gecko attach`: read the code, pair, wait for the end.
await new Promise((resolve, reject) => {
  const child = spawn(node, [bin, 'attach'], { env })
  let err = ''
  child.stderr.on('data', async data => {
    err += String(data)
    const match = /Kod: (\S+)/.exec(err)
    if (match && !child.paired) {
      child.paired = true
      const binding = await tab.pair(match[1])
      console_.attachRemote({ ...binding, send: reply => tab.sendToCli(reply) })
    }
  })
  child.on('exit', code => (code === 0 ? resolve() : reject(new Error(err))))
})

const args = expected.flatMap(value => ['--text', value])
const psQuote = value => `'${value.replace(/'/g, "''")}'`
const psArgs = args.map(psQuote).join(' ')
const line = `echo ${expected.map(v => `--text ${JSON.stringify(v)}`).join(' ')}`
const shells =
  process.platform === 'win32'
    ? [
        ['powershell.exe', ['-NoProfile', '-Command']],
        ['pwsh.exe', ['-NoProfile', '-Command']]
      ]
    : [['bash', ['-c']]]

const rows = []
const sh = (command, args_) =>
  new Promise(resolve => {
    const child = spawn(command, args_, { env })
    let output = ''
    child.stdout.on('data', data => (output += String(data)))
    child.stderr.on('data', data => (output += String(data)))
    child.on('error', () => resolve(false))
    child.on('exit', () => {
      last = output
      resolve(true)
    })
  })
let last = ''
const run = async (label, command, args_) => {
  echoed = undefined
  if (!(await sh(command, args_))) {
    rows.push([label, 'kabuk yok', ''])
    return
  }
  const ok = JSON.stringify(echoed) === JSON.stringify(expected)
  rows.push([
    label,
    ok ? 'doğru' : 'BOZUK',
    echoed === undefined
      ? `(sonuç:${last.trim().replace(/\s+/g, ' ').slice(0, 160)})`
      : JSON.stringify(echoed)
  ])
}
const readBack = file => {
  const bytes = readFileSync(file)
  const text =
    bytes[0] === 0xff && bytes[1] === 0xfe
      ? bytes.subarray(2).toString('utf16le')
      : bytes.toString('utf8')
  try {
    return JSON.parse(text.trim()).summary
  } catch {
    return null
  }
}

const lineFile = join(dir, 'line.txt')
writeFileSync(lineFile, line, 'utf8')
for (const [shell, pre] of shells) {
  const name = shell.replace(/\.exe$/, '')
  if (shell === 'bash') {
    const quoted = args.map(a => `'${a.replace(/'/g, "'\\''")}'`).join(' ')
    await run(`${name}, argv`, shell, [
      ...pre,
      `"${node}" "${bin}" --json echo ${quoted}`
    ])
    await run(`${name}, --line -`, shell, [
      ...pre,
      `"${node}" "${bin}" --json --line - < '${lineFile}'`
    ])
    continue
  }
  await run(`${name}, argv`, shell, [
    ...pre,
    `& '${node}' '${bin}' --json echo ${psArgs}`
  ])
  await run(`${name}, --line - (Get-Content | gecko)`, shell, [
    ...pre,
    `Get-Content -Raw -Encoding UTF8 '${lineFile}' | & '${node}' '${bin}' --json --line -`
  ])
  await run(`${name}, --line - ($OutputEncoding UTF-8)`, shell, [
    ...pre,
    `$OutputEncoding = [System.Text.UTF8Encoding]::new($false); Get-Content -Raw -Encoding UTF8 '${lineFile}' | & '${node}' '${bin}' --json --line -`
  ])
  // `--json` redirected with `>`: the file must read back as the result.
  const out = join(dir, `${name}.json`)
  await sh(shell, [
    ...pre,
    `& '${node}' '${bin}' --json echo --text 'a' > '${out}'`
  ])
  const summary = readBack(out)
  rows.push([
    `${name}, --json > dosya`,
    summary === 'ığüşöç ✓' ? 'doğru' : 'BOZUK',
    JSON.stringify(summary)
  ])
  const plain = join(dir, `${name}.txt`)
  await sh(shell, [
    ...pre,
    `& '${node}' '${bin}' --out '${plain}' echo --text 'a'`
  ])
  const text = readFileSync(plain, 'utf8')
  rows.push([
    `${name}, --out dosya`,
    text.startsWith('ığüşöç ✓') ? 'doğru' : 'BOZUK',
    JSON.stringify(text.split('\n')[0])
  ])
}
if (process.platform === 'win32') {
  const cmdFile = join(dir, 'args.cmd')
  writeFileSync(
    cmdFile,
    `@chcp 65001 >nul\r\n@"${node}" "${bin}" --json echo --text "a b" --text "" --text "x\\"y" --text "c:\\yol\\\\" --text "ığ"\r\n`,
    'utf8'
  )
  await run('cmd, .cmd dosyası', 'cmd.exe', ['/d', '/c', cmdFile])
}

process.stdout.write(
  '| Kabuk ve yol | Sonuç | Sekmeye varan |\n|---|---|---|\n'
)
for (const row of rows) process.stdout.write(`| ${row.join(' | ')} |\n`)

await tab.close()
await hub.close()
rmSync(dir, { recursive: true, force: true })
process.exit(0)
