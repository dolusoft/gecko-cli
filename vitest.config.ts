import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.spec.ts'],
    // The contract specs start the hub and the CLI as real processes.
    testTimeout: 20_000,
    hookTimeout: 20_000
  }
})
