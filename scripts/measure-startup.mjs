// The CLI's cold start: `gecko help` without a login (no network) and
// `gecko --version`, as wall time of a new process. Target < 300 ms for
// `help` (the brief of gecko-cli). Needs `pnpm build`.

import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const bin = fileURLToPath(new URL('../bin/gecko.js', import.meta.url))
const dir = mkdtempSync(join(tmpdir(), 'gecko-start-'))
const env = { ...process.env, GECKO_CONFIG_DIR: dir }
const runs = Number(process.argv[2] ?? 15)

const measure = args => {
  // One warm-up run fills the OS file cache, as a second call does.
  spawnSync(process.execPath, [bin, ...args], { env })
  const times = []
  for (let i = 0; i < runs; i++) {
    const start = process.hrtime.bigint()
    spawnSync(process.execPath, [bin, ...args], { env })
    times.push(Number(process.hrtime.bigint() - start) / 1e6)
  }
  times.sort((a, b) => a - b)
  return {
    median: times[Math.floor(times.length / 2)],
    p90: times[Math.floor(times.length * 0.9)]
  }
}

const baseline = (() => {
  const times = []
  for (let i = 0; i < runs; i++) {
    const start = process.hrtime.bigint()
    spawnSync(process.execPath, ['-e', '0'])
    times.push(Number(process.hrtime.bigint() - start) / 1e6)
  }
  times.sort((a, b) => a - b)
  return times[Math.floor(times.length / 2)]
})()

const help = measure(['help'])
const version = measure(['--version'])
const round = value => Math.round(value)
process.stdout.write(
  `node -e 0 (taban): ${round(baseline)} ms\n` +
    `gecko help (girişsiz): medyan ${round(help.median)} ms, p90 ${round(help.p90)} ms\n` +
    `gecko --version: medyan ${round(version.median)} ms, p90 ${round(version.p90)} ms\n`
)
rmSync(dir, { recursive: true, force: true })
process.exitCode = help.median < 300 ? 0 : 1
