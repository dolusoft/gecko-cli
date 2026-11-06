// The contract harness: the development hub and the CLI as real processes,
// and a fake tab in this process that runs command-console-core's remote
// entry (`attachRemote`) in Node, as a Gecko tab runs it in the browser.

import type { ChildProcess } from 'node:child_process'
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import type {
  CommandDefinition,
  ConsoleOptions,
  CoreConsoleAPI
} from '@dolusoft/command-console-core'
import { createConsole } from '@dolusoft/command-console-core'

import type { TabClient } from '../../src/tab/index.ts'
import { connectTab, createTabGate } from '../../src/tab/index.ts'

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
      env: { ...process.env, GECKO_DEV_HUB_STDIN: '1', ...extraEnv },
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
  // The gate follows the current connection, as the shell's hub client does.
  const listeners = new Set<(raw: unknown) => void>()
  const wire = (client: TabClient) => {
    client.onCliMessage(raw => {
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
  const attach = (binding: {
    bindingId: string
    grantId: string
    approvedAt: number
  }) =>
    console.attachRemote({
      ...binding,
      send: reply => {
        if (options.dropReplies?.(reply)) return
        return tab.sendToCli(reply).then(() => undefined)
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
    console,
    pageId,
    busyAttempts,
    async pair(code) {
      const binding = await tab.pair(code)
      attach(binding)
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
