// SPDX-License-Identifier: CC0-1.0

//! The header this service reports a response's correlation id in, and the id an
//! INVOCATION's answer reports there.
//!
//! Every answer AWS sends names the request it answered, in the `x-amzn-requestid` response
//! header — a success exactly as a failure — and that header is where an SDK reads the
//! `RequestId` of its response metadata from. A client that cannot read it has no
//! correlation handle on the call at all: an SDK's own error reporting and retry logging
//! both quote it, and a client that logs a failed call has nothing to log without it.
//!
//! For almost every operation that id is the RESPONSE's alone, minted per answer: nothing
//! else in the call holds one. An INVOCATION is the exception. The invocation Lambda
//! accepts has an id of its own ([`starting_invocation`]) which the run reports
//! as `context.aws_request_id`, writes into its `START`/`END`/`REPORT` log lines and the
//! log stream it writes them to, and carries in the error document a failed handler answers
//! with — and AWS reports THAT id on the invocation's response, so the client that invoked
//! a function can find the run it started in its logs, its report line or a support case.
//! Naming a second id there would break every correlation keyed on it.
//!
//! So the rule this service applies is: a response names the invocation the call it
//! answered started, and an answer that started none names a freshly minted id.
//! [`starting_invocation`] and [`serving_invocation`] are how the operation that started one
//! states which it is, and
//! [`naming_its_request`] is the one place both halves of the rule are applied — around
//! every modelled operation's whole run ([`super::StatedPerOperation`]), around the
//! response-streaming invocation's own entry, and around the answer to a request that named
//! no modelled operation. The id a run started off the response path (an asynchronous
//! delivery, a poller's batch) names nothing on this response, because the response it
//! would name was already sent.
//!
//! The framework's restJson1 renderers write the document and no such header (`vendor/` is
//! read-only), so this service reports it — through the TREE's statement of that rule
//! ([`service_common::query_error_envelope`]), which already words it for a protocol with
//! no envelope element for the id: the header alone, one id per response, and a response
//! that already names its request keeps the id it has. Nothing of that is restated here;
//! this module says WHERE this service applies it and WHICH id it reports.

use std::cell::RefCell;
use std::future::Future;

use aws_smithy_runtime_api::http::Response as RtResponse;
use service_common::query_error_envelope::with_stated_request_id_header;

pub use service_common::query_error_envelope::with_request_id_header;

tokio::task_local! {
    /// The invocation the request THIS task is serving started, recorded by the operation
    /// that started it ([`starting_invocation`]) and read back by [`naming_its_request`]
    /// when the answer is named.
    ///
    /// A task-local rather than a thread-local because an invoking operation awaits its run
    /// between minting the id and answering, so the answer may be named on another thread
    /// of the runtime than the one that recorded it. It is scoped around one request's whole
    /// run, so the slot an operation records into is always its own request's: a value
    /// cannot outlive the response it names, and work spawned off the response path holds no
    /// scope at all.
    static SERVED_INVOCATION: RefCell<Option<String>>;
}

/// State `request_id` as the invocation the request being served started, so this request's
/// answer names it.
///
/// For an operation that mints the id at the moment it decides to invoke,
/// [`starting_invocation`] does both in one call. This is the same statement for one that
/// already holds the id — it minted it while copying the run's inputs out of the store — and
/// states it once the invocation is really admitted, so a request that was refused instead
/// still names a response id of its own.
pub fn serving_invocation(request_id: &str) {
    let recorded = request_id.to_string();
    let _ = SERVED_INVOCATION.try_with(|slot| {
        *slot.borrow_mut() = Some(recorded);
    });
}

/// The id of an invocation the request being served STARTS: freshly minted
/// ([`crate::state::mint_request_id`]) and stated at once as the invocation this request's
/// answer names.
///
/// This is the ONE call an invoking operation makes for it, so the id the run reports as
/// `context.aws_request_id`, the id its `START`/`END`/`REPORT` lines and its error document
/// carry, and the id its response names cannot drift apart. An operation that starts no
/// invocation — a dry run, every control-plane call — calls nothing, and its answer is named
/// by a freshly minted response id.
///
/// Outside a request's scope (a delivery made off the response path) the id is still minted
/// and nothing is stated: there is no response of that invocation's own to name.
#[must_use]
pub fn starting_invocation() -> String {
    let request_id = crate::state::mint_request_id();
    serving_invocation(&request_id);
    request_id
}

/// Serve one request and name the request on the answer: the invocation the call started,
/// or a freshly minted id when it started none.
///
/// The whole run of the request is inside the scope — deserialize, validate, the handler and
/// the response — so an operation that starts an invocation can state it at the moment it
/// mints it rather than having to carry it back out through a modelled output no model
/// declares.
pub async fn naming_its_request<F>(served: F) -> RtResponse
where
    F: Future<Output = RtResponse> + Send,
{
    SERVED_INVOCATION
        .scope(RefCell::new(None), async move {
            let answered = served.await;
            match SERVED_INVOCATION.with(|slot| slot.borrow().clone()) {
                Some(request_id) => with_stated_request_id_header(answered, &request_id),
                None => with_request_id_header(answered),
            }
        })
        .await
}
