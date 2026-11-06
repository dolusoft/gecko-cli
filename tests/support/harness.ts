// The contract harness: the development hub and the CLI as real processes,
// and a fake tab in this process that runs command-console-core's remote
// entry (`attachRemote`) in Node, as a Gecko tab runs it in the browser.

import type { ChildProcess } from 'node:child_process'
import { spawn } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import type {
  CommandDefinition,
  ConsoleOptions,
  CoreConsoleAPI
} from '@dolusoft/command-console-core'
import { agentTools, createConsole } from '@dolusoft/command-console-core'

import type { TabClient } from '../../src/tab/index.ts'
import { connectTab, createTabGate, serveHelp } from '../../src/tab/index.ts'
import { connectDev } from '../../src/transport/dev.ts'

const root = fileURLToPath(new URL('../..', import.meta.url))
const bin = join(root, 'bin', 'gecko.js')

export interface HubProcess {
  url: string
  env: string
  child: ChildProcess
  stderr: () => string
  stop(): Promise<void>
}

/** `gecko dev-hub --port 0`; resolves with the address it prints. */
export function startHub(
  extraEnv: Record<string, string> = {},
  args: string[] = []
): Promise<HubProcess> {
  const child = spawn(
    process.execPath,
    [bin, 'dev-hub', '--port', '0', ...args],
    {
      env: {
        ...process.env,
        GECKO_DEV_HUB_STDIN: '1',
        GECKO_DEV_HUB_RATE: '1000',
        ...extraEnv
      },
      stdio: ['pipe', 'pipe', 'pipe']
    }
  )
  let err = ''
  child.stderr!.on('data', data => (err += String(data)))
  return new Promise((resolve, reject) => {
    let out = ''
    child.stdout!.on('data', data => {
      out += String(data)
      const line = out.split('\n')[0]
      if (out.includes('\n')) {
        const url = line.trim()
        resolve({
          url,
          env: url.replace(/^http:\/\//, ''),
          child,
          stderr: () => err,
          stop: () =>
            new Promise<void>(done => {
              if (child.exitCode !== null) return done()
              child.once('exit', () => done())
              child.stdin!.end()
            })
        })
      }
    })
    child.once('exit', code => reject(new Error(`hub exited ${code}: ${err}`)))
  })
}

export interface CliRun {
  code: number
  stdout: string
  stderr: string
  /** The raw stdout bytes. */
  bytes: Buffer
}

export interface CliHandle {
  child: ChildProcess
  /** Resolves when the given text appears on stderr. */
  waitStderr(pattern: RegExp, timeoutMs?: number): Promise<RegExpMatchArray>
  /** The test hook for Ctrl+C on every platform. */
  interrupt(): void
  done: Promise<CliRun>
}

/** Starts `gecko <args>` with its own config directory. */
export function cli(
  args: string[],
  options: {
    configDir: string
    env?: Record<string, string>
    stdin?: string
  }
): CliHandle {
  const child = spawn(process.execPath, [bin, ...args], {
    env: {
      ...process.env,
      GECKO_CONFIG_DIR: options.configDir,
      GECKO_BROWSER: 'none',
      GECKO_TEST_INTERRUPT_STDIN: options.stdin === undefined ? '1' : '0',
      NO_COLOR: '1',
      ...options.env
    },
    stdio: ['pipe', 'pipe', 'pipe']
  })
  const out: Buffer[] = []
  let err = ''
  const waiters: { pattern: RegExp; resolve: (m: RegExpMatchArray) => void }[] =
    []
  child.stdout!.on('data', data => out.push(data as Buffer))
  child.stderr!.on('data', data => {
    err += String(data)
    for (const waiter of [...waiters]) {
      const match = err.match(waiter.pattern)
      if (match) {
        waiters.splice(waiters.indexOf(waiter), 1)
        waiter.resolve(match)
      }
    }
  })
  if (options.stdin !== undefined) {
    child.stdin!.end(options.stdin)
  }
  const done = new Promise<CliRun>(resolve => {
    child.once('close', code => {
      const bytes = Buffer.concat(out)
      resolve({
        code: code ?? -1,
        stdout: bytes.toString('utf8'),
        stderr: err,
        bytes
      })
    })
  })
  return {
    child,
    done,
    interrupt: () => child.stdin!.write('x'),
    waitStderr(pattern, timeoutMs = 10_000) {
      const match = err.match(pattern)
      if (match) return Promise.resolve(match)
      return new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error(`no ${pattern} on stderr: ${err}`)),
          timeoutMs
        )
        void done.then(run => {
          clearTimeout(timer)
          reject(
            new Error(
              `exited ${run.code} before ${pattern}: ${run.stdout} ${run.stderr}`
            )
          )
        })
        waiters.push({
          pattern,
          resolve: m => {
            clearTimeout(timer)
            resolve(m)
          }
        })
      })
    }
  }
}

export function tempConfig(): { dir: string; remove(): void } {
  const dir = mkdtempSync(join(tmpdir(), 'gecko-cli-test-'))
  return { dir, remove: () => rmSync(dir, { recursive: true, force: true }) }
}

/** Approves a device code as the fake user's login page does. */
export async function approveDevice(
  hubUrl: string,
  userCode: string,
  options: { user?: string; approve?: boolean } = {}
) {
  return fetch(`${hubUrl}/oauth/device/verify`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer dev:${options.user ?? 'u1'}`
    },
    body: JSON.stringify({
      user_code: userCode,
      approve: options.approve ?? true
    })
  })
}

/** `gecko login` with the user's approval; the config directory keeps the token. */
export async function login(hub: HubProcess, configDir: string, user = 'u1') {
  const run = cli(['login', '--env', hub.env], { configDir })
  const [, code] = await run.waitStderr(/Kod: (\S+)/)
  await approveDevice(hub.url, code, { user })
  const result = await run.done
  if (result.code !== 0) {
    throw new Error(`login failed: ${result.stderr}`)
  }
  return result
}

export interface FakeTab {
  tab: TabClient
  console: CoreConsoleAPI
  pageId: string
  /** Uses the code and attaches the console, as the pairing page does. */
  pair(code: string): Promise<void>
  /** Binds without attaching the console: a page that predates the CLI. */
  pairOnly(code: string): Promise<void>
  /** Hears CLI messages but does not hand them to the console. */
  deaf: boolean
  /** Every CLI message the hub routed to this tab. */
  heard: unknown[]
  /** Waits for a plan and approves it after its first second. */
  approveNext(): Promise<void>
  rejectNext(): Promise<void>
  /** Drops the connection (Wi-Fi); `reconnect` says hello again. */
  drop(): Promise<void>
  reconnect(): Promise<void>
  close(): Promise<void>
  busyAttempts: unknown[]
}

export interface FakeTabOptions {
  hubUrl: string
  user?: string
  pageId?: string
  commands: CommandDefinition[]
  /** The tab's clock against the hub's, in ms. */
  clockOffsetMs?: number
  status?: () => { hidden: boolean; dataAgeMs?: number | null }
  /** Replies the tab never sends (a lost `received`). */
  dropReplies?: (reply: { kind: string }) => boolean
  extra?: Partial<ConsoleOptions>
}

let pages = 0

export async function fakeTab(options: FakeTabOptions): Promise<FakeTab> {
  const pageId = options.pageId ?? `page-${process.pid}-${++pages}`
  const token = `dev:${options.user ?? 'u1'}`
  let tab = await connectTab({ hubUrl: options.hubUrl, token, pageId })
  const busyAttempts: unknown[] = []
  const heard: unknown[] = []
  const self = { deaf: false }
  // The gate follows the current connection, as the shell's hub client does.
  const listeners = new Set<(raw: unknown) => void>()
  const wire = (client: TabClient) => {
    client.onCliMessage(raw => {
      heard.push(raw)
      if (self.deaf) return
      for (const listener of [...listeners]) listener(raw)
    })
    client.onCliBusy(event => busyAttempts.push(event))
    client.onBinding(event => {
      if (event.state === 'gone') {
        console.detachRemote()
      }
    })
  }
  const gate = {
    ...createTabGate(tab, pageId),
    subscribe(listener: (raw: unknown) => void) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    }
  }
  const console = createConsole({
    gate,
    now: () => Date.now() + (options.clockOffsetMs ?? 0),
    commands: options.commands,
    ...options.extra
  })
  wire(tab)
  // `gecko help`: the agent help tool, as the shell serves it.
  const helpTool = agentTools(console, { context: () => ({}) })[1]
  const serve = (client: TabClient) =>
    serveHelp(client, {
      help: input => helpTool.execute(input),
      bindingId: () =>
        self.deaf ? undefined : console.getState().remote?.bindingId,
      now: () => Date.now() + (options.clockOffsetMs ?? 0)
    })
  serve(tab)
  const attach = (binding: {
    bindingId: string
    grantId: string
    approvedAt: number
  }) =>
    console.attachRemote({
      ...binding,
      send: reply => {
        if (options.dropReplies?.(reply)) return
        // A reply after a drop has no connection to go on: lost, as in a tab.
        return tab.sendToCli(reply).then(
          () => undefined,
          () => undefined
        )
      },
      ...(options.status ? { status: options.status } : {})
    })
  const waitPlan = async () => {
    for (let i = 0; i < 500; i++) {
      const approval = console.getState().remote?.approval
      if (approval) return approval
      await new Promise(r => setTimeout(r, 20))
    }
    throw new Error('no approval plan')
  }
  return {
    get tab() {
      return tab
    },
    get deaf() {
      return self.deaf
    },
    set deaf(value: boolean) {
      self.deaf = value
    },
    console,
    pageId,
    busyAttempts,
    heard,
    async pair(code) {
      const binding = await tab.pair(code)
      attach(binding)
    },
    async pairOnly(code) {
      await tab.pair(code)
    },
    async approveNext() {
      const plan = await waitPlan()
      const wait = plan.acceptsAt - (Date.now() + (options.clockOffsetMs ?? 0))
      if (wait > 0) await new Promise(r => setTimeout(r, wait + 20))
      if (!console.approve(plan.requestId)) throw new Error('approve refused')
    },
    async rejectNext() {
      const plan = await waitPlan()
      console.reject(plan.requestId)
    },
    async drop() {
      await tab.close()
      console.suspendRemote()
    },
    async reconnect() {
      tab = await connectTab({ hubUrl: options.hubUrl, token, pageId })
      wire(tab)
      serve(tab)
      // `connectTab` said hello; a binding back within 60 s is active again.
      if (tab.binding && console.getState().remote === null) {
        attach(tab.binding)
      }
    },
    async close() {
      await tab.close().catch(() => {})
      console.dispose?.()
    }
  }
}

/** Starts an attach, pairs the fake tab with the printed code. */
export async function attach(
  hub: HubProcess,
  configDir: string,
  tab: FakeTab,
  args: string[] = []
) {
  const run = cli(['attach', ...args], { configDir })
  const [, code] = await run.waitStderr(/Kod: (\S+)/)
  await tab.pair(code)
  const result = await run.done
  if (result.code !== 0) throw new Error(`attach failed: ${result.stderr}`)
  void hub
  return result
}

/** `gecko --json <args>`: the exit code and the parsed object. */
export async function jsonRun(
  args: string[],
  configDir: string,
  env: Record<string, string> = {}
) {
  const run = await cli(['--json', ...args], { configDir, env }).done
  let result: Record<string, unknown>
  try {
    result = JSON.parse(run.stdout) as Record<string, unknown>
  } catch {
    throw new Error(`not JSON (exit ${run.code}): ${run.stdout} ${run.stderr}`)
  }
  return { ...run, result }
}

/** The stored token of a config directory. */
export function storedToken(configDir: string, env: string) {
  const file = join(
    configDir,
    `token-${env.replace(/[^A-Za-z0-9.-]/g, '_')}.json`
  )
  return JSON.parse(readFileSync(file, 'utf8')) as {
    accessToken: string
    refreshToken: string
    grantId: string
  }
}

/** A raw CLI connection with the stored token, for the hub-level checks. */
export function rawCli(hub: HubProcess, configDir: string) {
  return connectDev({
    host: hub.env,
    token: storedToken(configDir, hub.env).accessToken
  })
}

export const pause = (ms: number) => new Promise(r => setTimeout(r, ms))
