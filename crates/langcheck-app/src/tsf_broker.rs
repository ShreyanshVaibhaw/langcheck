//! TSF broker IPC server.
//!
//! Runs on a dedicated thread, answering the TSF adapter's requests with the shared
//! engine ([`crate::engine`]). The broker is the only process that holds language
//! logic and persistence (`blueprint.md` §7.1, 11.4); the adapter only asks and
//! applies. The server enforces the kill switch: when LangCheck is disabled or
//! paused, every `Evaluate` is answered `Leave` (a liveness `Ping` is still
//! answered). The same-user, local-only pipe is provided by
//! `langcheck-windows::ipc`.

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
pub fn serve(
    shared: Arc<SharedState>,
    lexicon: Box<dyn LexiconProvider>,
    personal: PersonalDictionary,
) {
    let weights = RankWeights::default();
    let policy = ConfidencePolicy::default();

    let server = match PipeServer::bind() {
        Ok(server) => server,
        Err(_) => return, // fail open: no broker pipe, adapter does nothing
    };

    while !shared.is_shutdown() {
        // Re-read the kill switch per connection.
        let active = shared.enabled() && !shared.paused();
        let outcome = server.serve_one(|request| match request {
            Request::Ping => Response::Pong,
            request @ Request::Evaluate { .. } if active => {
                engine::evaluate_request(request, &*lexicon, &personal, &weights, &policy)
            }
            // Kill switch engaged: acknowledge but never correct.
            Request::Evaluate { .. } => Response::Leave,
        });
        // A transient client/IO error must not kill the broker; the instance was
        // already reset, so just wait for the next client.
        if outcome.is_err() {
            continue;
        }
    }
}
