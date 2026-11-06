// Opens an address in the default browser when there is a desktop; without
// one (SSH, a server) the caller prints the address and the code instead.

import { spawn } from 'node:child_process'

/** Whether this session can show a browser. `GECKO_BROWSER=none` says no. */
export function hasDesktop(): boolean {
  if (process.env.GECKO_BROWSER === 'none') {
    return false
  }
  if (process.env.SSH_CONNECTION || process.env.SSH_TTY) {
    return false
  }
  if (process.platform === 'win32' || process.platform === 'darwin') {
    return true
  }
  return Boolean(process.env.DISPLAY || process.env.WAYLAND_DISPLAY)
}

/** Starts the default browser on `url`; `false` when it could not. */
export function openBrowser(url: string): boolean {
  if (!hasDesktop()) {
    return false
  }
  // The address is built by the CLI (an origin and a code), never typed.
  const [command, args] =
    process.platform === 'win32'
      ? ['rundll32', ['url.dll,FileProtocolHandler', url]]
      : process.platform === 'darwin'
        ? ['open', [url]]
        : ['xdg-open', [url]]
  try {
    const child = spawn(command, args as string[], {
      detached: true,
      stdio: 'ignore',
      windowsHide: true
    })
    child.on('error', () => {})
    child.unref()
    return true
  } catch {
    return false
  }
}
