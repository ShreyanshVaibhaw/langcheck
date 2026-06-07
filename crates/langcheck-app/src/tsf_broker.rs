//! TSF broker IPC server.
//!
//! Runs on a dedicated thread, answering the TSF adapter's requests with the shared
//! engine ([`crate::engine`]). The broker is the only process that holds language
//! logic and persistence (`blueprint.md` §7.1, 11.4); the adapter only asks and
//! applies. The server enforces the kill switch: when LangCheck is disabled or
//! paused, every `Evaluate` is answered `Leave` (a liveness `Ping` is still
//! answered). The same-user, local-only pipe is provided by `langcheck-ipc`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use langcheck_core::ipc::{Request, Response};
use langcheck_core::{ConfidencePolicy, RankWeights};
use langcheck_ipc::PipeServer;
use langcheck_lexicon::{LexiconProvider, PersonalDictionary};

use crate::coordinator::SharedState;
use crate::engine;

/// Serve adapter requests until `shared` signals shutdown.
///
/// Fail-open: if the pipe cannot be created the adapter simply finds no broker and
/// (by its own contract) leaves typing untouched. Per-request engine work reuses
/// the immutable `lexicon` and `personal` dictionary.
///
/// When `log_requests` is set, each Evaluate prints a running **count** (never the
/// typed token — privacy) so the TSF adapter's detection can be confirmed live.
pub fn serve(
    shared: Arc<SharedState>,
    lexicon: Box<dyn LexiconProvider>,
    personal: PersonalDictionary,
    log_requests: bool,
) {
    let weights = RankWeights::default();
    let policy = ConfidencePolicy::default();
    let evaluations = AtomicU32::new(0);

    let server = match PipeServer::bind() {
        Ok(server) => server,
        Err(_) => return, // fail open: no broker pipe, adapter does nothing
    };

    while !shared.is_shutdown() {
        // Re-read the kill switches per connection: the global enable/pause AND the
        // TSF-adapter-specific switch must all be on to apply a correction.
        let active = shared.enabled() && !shared.paused() && shared.tsf_enabled();
        let outcome = server.serve_one(|request| {
            // Any adapter contact (the focus beacon OR a word eval) means TSF is
            // handling the foreground window — record it so the MVP keystroke path
            // stands down there. The beacon arrives on focus, before the first word,
            // so the keystroke path defers before it can fire (no race).
            if active && matches!(&request, Request::Active | Request::Evaluate { .. }) {
                shared.note_tsf_activity(shared.focus_id.load(Ordering::SeqCst));
            }
            if log_requests {
                if let Request::Evaluate { .. } = request {
                    let n = evaluations.fetch_add(1, Ordering::SeqCst) + 1;
                    // Privacy: count only — never the typed token (blueprint §12.1).
                    println!("broker: evaluate request #{n} received");
                }
            }
            match request {
                // Kill switch engaged: acknowledge an Evaluate but never correct.
                Request::Evaluate { .. } if !active => Response::Leave,
                // Ping/Active -> Pong; Evaluate -> shared engine decision.
                other => engine::evaluate_request(other, &*lexicon, &personal, &weights, &policy),
            }
        });
        // A transient client/IO error must not kill the broker; the instance was
        // already reset, so just wait for the next client.
        if outcome.is_err() {
            continue;
        }
    }
}
