//! Ctrl+C, counted. With `GECKO_TEST_INTERRUPT_STDIN=1` every `x` written to stdin
//! counts as one too: Windows cannot send SIGINT to a child process, so the tests use
//! this (docs/test-hooks.md). The binary's side of the kernel's `Interrupts` port.

use gecko_kernel::Interrupts;
use tokio::io::AsyncReadExt;
use tokio::sync::watch;

/// How many interrupts arrived so far.
#[derive(Debug, Clone)]
pub struct CtrlC {
    count: watch::Receiver<u32>,
    seen: u32,
}

impl CtrlC {
    /// Starts listening; call inside the runtime. `stdin_hook` is
    /// `GECKO_TEST_INTERRUPT_STDIN=1`.
    pub fn listen(stdin_hook: bool) -> CtrlC {
        let (tx, count) = watch::channel(0u32);
        let ctrl_c = tx.clone();
        tokio::spawn(async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                ctrl_c.send_modify(|n| *n += 1);
            }
        });
        if stdin_hook {
            tokio::spawn(async move {
                let mut stdin = tokio::io::stdin();
                let mut buffer = [0u8; 64];
                while let Ok(read) = stdin.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    let presses = buffer[..read].iter().filter(|b| **b == b'x').count();
                    for _ in 0..presses {
                        tx.send_modify(|n| *n += 1);
                    }
                }
            });
        }
        CtrlC { count, seen: 0 }
    }
}

impl Interrupts for CtrlC {
    async fn next(&mut self) {
        let seen = self.seen;
        match self.count.wait_for(|n| *n > seen).await {
            // One at a time: two quick presses resolve two waits.
            Ok(_) => self.seen = seen + 1,
            // Nothing can interrupt any more.
            Err(_) => std::future::pending::<()>().await,
        }
    }
}
