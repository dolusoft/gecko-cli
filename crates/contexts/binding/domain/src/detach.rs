//! `gecko detach`: what ending the binding reports.

use gecko_kernel::Outcome;

/// No token: nothing to end.
pub fn nothing_to_detach() -> Outcome {
    Outcome::done("There was no binding")
}

/// The token is gone; `ended` says whether the hub ended the binding too.
pub fn detached(ended: bool) -> Outcome {
    Outcome::done(if ended {
        "Detached"
    } else {
        "Detached here; the hub was not reached, so the tab keeps the binding until it ends it (CLI bindings in the Gecko profile)"
    })
}
