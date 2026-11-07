// `gecko`: the remote control of the Gecko web terminal (desktop CLI
// specification). Its own commands are login, logout, attach, detach, help
// and dev-hub; every other line goes to the bound tab.

import { readFileSync, writeFileSync } from 'node:fs'

import { tokenize } from '@dolusoft/command-protocol'

import type { OwnCommand, Parsed } from './args.ts'
import { ownOption, parseArgs } from './args.ts'
import { CliFailure, deviceLogin, resolveEnv, revokeToken } from './auth.ts'
import { openBrowser } from './browser.ts'
import { helpInput, requestHelp, unknownHelpTarget } from './help.ts'
import { deleteToken, readToken, writeDefaultEnv } from './config.ts'
import type { FormatOptions, Result } from './output.ts'
import {
  asciiJson,
  errorResult,
  escapeControls,
  exitCodeOf,
  exitOf,
  paint,
  plainText,
  write
} from './output.ts'
import type { Interrupts } from './run.ts'
import { runLine } from './run.ts'
import type { Session } from './session.ts'
import {
  capabilities,
  envOf,
  goneMessage,
  openSession,
  pair,
  toFailure
} from './session.ts'

const usage = `gecko: Gecko web terminalinin uzaktan kumandası

Kullanım:
  gecko [seçenekler] <komut satırı>     satır bağlı Gecko sekmesinde çalışır
  gecko [seçenekler] --line "<satır>"   satırı aynen yollar
  gecko [seçenekler] --line -           satırı stdin'den okur (ajanlar, PowerShell 5.1)

Kendi komutları:
  login [--env <ortam>]   cihaz girişi; ortamı sabitler
  logout                  yetkiyi iptal eder, token'ı siler
  attach [--new]          bir Gecko sekmesine bağlanır (tarayıcıda onay)
  detach                  bağlamayı keser
  help [ad alanı|komut]   bağlı sekmenin komutları (izne göre, sayfalı)
  dev-hub [--port <n>] [--web <köken>] [--auto-approve]
                          geliştirme hub'ını başlatır (yalnız localhost)

Seçenekler (komut adından önce):
  --json            tek JSON nesnesi, ASCII dışı karakterler \\uXXXX
  --env <ortam>     ortam (varsayılan: son login)
  --view <id>       hedef görünüm
  --out <dosya>     sonucu UTF-8 olarak dosyaya yazar
  --line <satır|->  satırın tamamı ya da stdin

Her kendi komutunda --help o komutun kullanımını basar.
CLI seçenekleri komut adından önce gelir: gecko --json <komut> …; tam çıktı için --json.

Çıkış kodları: 0 tamam · 1 komut hatası · 2 kullanım · 3 izin/onay yok ·
4 çalışmadı (yeniden denenebilir) · 5 sonuç bilinmiyor · 6 kimlik · 130 Ctrl+C
`

const ownUsage: Record<OwnCommand, string> = {
  login: `Kullanım: gecko login [--env <ortam>]

Cihaz girişi: kod basar, tarayıcıda onay sayfasını açar. Onaylanınca
ortamı varsayılan yapar.
`,
  logout: `Kullanım: gecko logout

CLI yetkisini iptal eder ve token'ı siler.
`,
  attach: `Kullanım: gecko attach [--new]

Bir Gecko sekmesine bağlanır; tarayıcıda onay ister. Bağlıysa yeni sekme
açmaz; --new yeni bağlama kurar.
`,
  detach: `Kullanım: gecko detach

Bağlamayı keser.
`,
  help: `Kullanım: gecko help [ad alanı|komut] [--cursor <n>] [--limit <n>]

Bağlı sekmenin komutları, izninize göre süzülmüş ve sayfalı. Komut adı
verilirse kullanımı ve girdisi; ad alanı verilirse (tasks) yalnız onun
komutları. --cursor önceki sayfanın sonunda yazan değerle devam eder.
`,
  'dev-hub': `Kullanım: gecko dev-hub [--port <n>] [--web <köken>] [--auto-approve] [--interval <sn>]

Geliştirme hub'ını başlatır, yalnız 127.0.0.1'de dinler.
  --port <n>        port (varsayılan 4400; 0 boş bir port)
  --web <köken>     /cli-pair ve /cli-login adreslerinin kökeni (frontendx)
  --auto-approve    giriş ve bağlamayı onaysız kabul eder (testler)
  --interval <sn>   cihaz girişinin yoklama aralığı
`
}

class SigintInterrupts implements Interrupts {
  private _listeners = new Set<(count: number) => void>()
  private _count = 0
  constructor() {
    process.on('SIGINT', () => this.fire())
    // Test hook: Windows cannot deliver SIGINT to a child process; the
    // contract specs write "x" to stdin instead.
    if (process.env.GECKO_TEST_INTERRUPT_STDIN === '1') {
      process.stdin.on('data', data => {
        for (const char of String(data)) {
          if (char === 'x') this.fire()
        }
      })
    }
  }
  fire() {
    this._count++
    if (this._listeners.size === 0) {
      // Nothing waits on a request: stop now.
      process.exit(130)
    }
    for (const listener of [...this._listeners]) listener(this._count)
  }
  onInterrupt(listener: (count: number) => void) {
    this._listeners.add(listener)
    return () => this._listeners.delete(listener)
  }
  get signal(): AbortSignal {
    const controller = new AbortController()
    this.onInterrupt(() => controller.abort())
    return controller.signal
  }
}

const io = {
  info(text: string) {
    process.stderr.write(
      `${paint(process.stderr, 'info', escapeControls(text))}\n`
    )
  }
}

const helpFooter =
  'CLI seçenekleri komut adından önce gelir: gecko --json <komut> …; tam çıktı için --json.'

async function emit(
  result: Result,
  options: Parsed['options'],
  extra: FormatOptions = {}
): Promise<number> {
  const code = exitOf(result)
  const columns =
    options.out === undefined && process.stdout.isTTY
      ? process.stdout.columns
      : undefined
  const text = options.json
    ? `${asciiJson(result)}\n`
    : plainText(result, columns ? { columns, ...extra } : extra)
  if (options.out !== undefined) {
    writeFileSync(options.out, text, 'utf8')
  } else {
    await write(process.stdout, text)
  }
  if (!options.json && result.status !== 'ok') {
    const head = result.code ? `hata (${result.code})` : 'hata'
    await write(
      process.stderr,
      `${paint(process.stderr, 'error', head)}: ${escapeControls(result.message ?? '')}\n`
    )
    if (result.suggest) {
      await write(
        process.stderr,
        `Satır: gecko ${escapeControls(result.suggest)}\n`
      )
    }
  }
  return code
}

function failureResult(error: unknown): Result {
  const failure = toFailure(error)
  return errorResult(failure.code, failure.message, {
    ...(failure.reason ? { reason: failure.reason } : {})
  })
}

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = []
  for await (const chunk of process.stdin) {
    chunks.push(chunk as Buffer)
  }
  return Buffer.concat(chunks)
    .toString('utf8')
    .replace(/\r?\n$/, '')
}

async function sessionWithBinding(
  parsed: Parsed,
  interrupts: SigintInterrupts,
  allowPair: boolean
) {
  const session = await openSession(parsed.options.env)
  let binding = session.status.binding
  if (binding?.state === 'suspended') {
    throw new CliFailure(
      'tab-unreachable',
      'bağlı sekmeye şu an ulaşılamıyor (bağlantısı düştü); 60 sn içinde dönerse bağlama sürer'
    )
  }
  if (binding?.state === 'gone') {
    // No new tab on its own (§5.3): the CLI says why and offers attach.
    throw new CliFailure('gone', goneMessage(binding.reason), binding.reason)
  }
  if (!binding) {
    if (!allowPair) {
      throw new CliFailure('no-binding', 'bağlama yok: gecko attach')
    }
    binding = await pair(session, io, { open: true, signal: interrupts.signal })
    io.info('Bağlandı.')
  }
  const caps = await capabilities(session, binding, capabilitiesWaitMs())
  return { session, binding, caps }
}

async function own(
  command: OwnCommand,
  parsed: Extract<Parsed, { kind: 'own' }>,
  interrupts: SigintInterrupts
): Promise<number> {
  const options = parsed.options
  const done = (summary: string): Result => ({
    v: 1,
    status: 'ok',
    summary,
    preview: false,
    complete: true,
    untrusted: []
  })
  switch (command) {
    case 'dev-hub': {
      const { startDevHub } = await import('../hub/hub.ts')
      const port = Number(ownOption(parsed.args, 'port') ?? 4400)
      const interval = ownOption(parsed.args, 'interval')
      const web = ownOption(parsed.args, 'web')
      const hub = await startDevHub({
        port,
        autoApprove: ownOption(parsed.args, 'auto-approve') === true,
        // The origin of frontendx for /cli-pair and /cli-login addresses.
        ...(typeof web === 'string'
          ? { webOrigin: web.replace(/\/$/, '') }
          : {}),
        ...(process.env.GECKO_DEV_HUB_RATE
          ? { ratePerSecond: Number(process.env.GECKO_DEV_HUB_RATE) }
          : {}),
        ...(process.env.GECKO_DEV_HUB_BURST
          ? { rateBurst: Number(process.env.GECKO_DEV_HUB_BURST) }
          : {}),
        ...(process.env.GECKO_DEV_HUB_USERS
          ? { users: JSON.parse(process.env.GECKO_DEV_HUB_USERS) as [] }
          : {}),
        timings: {
          // Tests shorten the windows of §5 and §8 (JSON of DevHubTimings).
          ...(process.env.GECKO_DEV_HUB_TIMINGS
            ? (JSON.parse(process.env.GECKO_DEV_HUB_TIMINGS) as object)
            : {}),
          ...(interval ? { intervalMs: Number(interval) * 1000 } : {})
        },
        log: line => process.stderr.write(`${line}\n`)
      })
      // The first stdout line is the address; tests read it.
      await write(process.stdout, `${hub.url}\n`)
      io.info(`ortam: gecko login --env ${hub.env}`)
      await new Promise<void>(resolve => {
        const stop = () => resolve()
        process.once('SIGINT', stop)
        process.once('SIGTERM', stop)
        process.stdin.on('end', () => {
          if (process.env.GECKO_DEV_HUB_STDIN === '1') stop()
        })
        if (process.env.GECKO_DEV_HUB_STDIN === '1') process.stdin.resume()
      })
      await hub.close()
      return 0
    }
    case 'login': {
      const envOption = ownOption(parsed.args, 'env')
      const env = typeof envOption === 'string' ? envOption : envOf(options.env)
      resolveEnv(env)
      const token = await deviceLogin(
        env,
        start => {
          const opened = openBrowser(start.verificationUriComplete)
          io.info(
            opened
              ? `Kod: ${start.userCode}  ·  tarayıcıda onay sayfası açıldı`
              : `Kod: ${start.userCode}  ·  şu adresi açın: ${start.verificationUri} (kodu girin)`
          )
        },
        interrupts.signal
      )
      writeDefaultEnv(env)
      return emit(done(`Onaylandı: ${token.user.name} (${env})`), options)
    }
    case 'logout': {
      const env = envOf(options.env)
      const token = readToken(env)
      if (token) {
        await revokeToken(token)
      }
      deleteToken(env)
      return emit(done(`Çıkış yapıldı (${env})`), options)
    }
    case 'attach': {
      const session = await openSession(options.env)
      try {
        const current = session.status.binding
        const fresh = ownOption(parsed.args, 'new') === true
        if (current?.state === 'active' && !fresh) {
          await capabilities(session, current, capabilitiesWaitMs())
          return emit(
            done(
              `Zaten bağlı (sayfa ${current.pageId}). Yeni sekme için: gecko attach --new`
            ),
            options
          )
        }
        const binding = await pair(session, io, {
          open: true,
          signal: interrupts.signal
        })
        await capabilities(session, binding, capabilitiesWaitMs())
        return emit(done(`Bağlandı (sayfa ${binding.pageId})`), options)
      } finally {
        await close(session)
      }
    }
    case 'detach': {
      const session = await openSession(options.env)
      try {
        await session.transport.detach()
        return emit(done('Bağlama kesildi'), options)
      } finally {
        await close(session)
      }
    }
    case 'help': {
      const { session, caps } = await sessionWithBinding(
        parsed,
        interrupts,
        true
      )
      try {
        const input = helpInput(caps, parsed.args)
        const unknown = unknownHelpTarget(caps, input)
        if (unknown) return await emit(unknown, options)
        const result = await requestHelp({
          transport: session.transport,
          input,
          marginMs: process.env.GECKO_DELIVERY_MARGIN_MS
            ? Number(process.env.GECKO_DELIVERY_MARGIN_MS)
            : undefined
        })
        // Only the list (a page of commands) carries the CLI-options note.
        const list =
          typeof result.data === 'object' &&
          result.data !== null &&
          'commands' in result.data
        return await emit(result, options, list ? { footer: helpFooter } : {})
      } finally {
        await close(session)
      }
    }
  }
}

async function close(session: Session) {
  await session.transport.close().catch(() => {})
}

async function lineCommand(
  parsed: Parsed,
  line: string,
  interrupts: SigintInterrupts
): Promise<number> {
  const { session, caps } = await sessionWithBinding(parsed, interrupts, true)
  try {
    const result = await runLine({
      transport: session.transport,
      capabilities: caps,
      line,
      view: parsed.options.view,
      io,
      interrupts,
      marginMs: process.env.GECKO_DELIVERY_MARGIN_MS
        ? Number(process.env.GECKO_DELIVERY_MARGIN_MS)
        : undefined,
      cancelWaitMs: process.env.GECKO_CANCEL_WAIT_MS
        ? Number(process.env.GECKO_CANCEL_WAIT_MS)
        : undefined
    })
    if (result.view?.title && !parsed.options.json) {
      io.info(`Görünüm: ${result.view.title}`)
    }
    return await emit(result, parsed.options, { line })
  } finally {
    await close(session)
  }
}

async function version(): Promise<string> {
  const pkg = JSON.parse(
    readFileSync(new URL('../../package.json', import.meta.url), 'utf8')
  ) as { version: string }
  return pkg.version
}

export async function main(argv: readonly string[]): Promise<number> {
  const parsed = parseArgs(argv)
  const options = parsed.options
  try {
    if (options.version) {
      await write(process.stdout, `${await version()}\n`)
      return 0
    }
    if (parsed.kind === 'usage') {
      await write(
        process.stdout,
        parsed.command ? ownUsage[parsed.command] : usage
      )
      return 0
    }
    if (parsed.kind === 'error') {
      return emit(errorResult('usage', parsed.message), options)
    }
    const interrupts = new SigintInterrupts()
    if (parsed.kind === 'own') {
      if (parsed.command === 'help' && !hasLogin(options.env)) {
        // Without a login there is no tab to ask: the CLI's own usage.
        await write(process.stdout, usage)
        io.info('Gecko komutları için önce: gecko login --env <ortam>')
        return 0
      }
      return await own(parsed.command, parsed, interrupts)
    }
    const line = parsed.line === '-' ? await readStdin() : parsed.line
    // The tab's `help` command is not open to the CLI (not-allowed): a
    // `help …` line takes the help-request path, as `gecko help …` does.
    const tokens = tokenize(line).tokens.map(token => token.value)
    if (tokens[0] === 'help') {
      return await own(
        'help',
        { kind: 'own', command: 'help', options, args: tokens.slice(1) },
        interrupts
      )
    }
    return await lineCommand(parsed, line, interrupts)
  } catch (error) {
    const result = failureResult(error)
    if (exitCodeOf(result.code ?? '') === 130) {
      return emit(
        errorResult('cancelled', 'iptal edildi', { status: 'cancelled' }),
        options
      )
    }
    return emit(result, options)
  }
}

function hasLogin(env: string | undefined): boolean {
  try {
    return readToken(envOf(env)) !== undefined
  } catch {
    return false
  }
}

/** How long the tab's capability list is awaited after pairing. */
function capabilitiesWaitMs(): number {
  return Number(process.env.GECKO_CAPABILITIES_WAIT_MS ?? 5000)
}
