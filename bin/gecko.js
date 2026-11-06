#!/usr/bin/env node
// The `gecko` command. stdout and stderr have drained when main resolves.
import { main } from '../dist/cli/main.js'

const code = await main(process.argv.slice(2))
process.exit(code)
