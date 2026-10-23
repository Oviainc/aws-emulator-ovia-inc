// SPDX-License-Identifier: CC0-1.0

//! The readers and assertion helpers the repository suites share, over the wire
//! harness this test target already has.
//!
//! Driving the service — building the emulator, signing a call, asserting a status,
//! reading a value out of a payload — is [`crate::repository_lifecycle`]'s harness
//! (itself signing through [`crate::wire`]), and this module adds nothing to it. What
//! lives here is what more than one repository suite needs: the `GetRepository`
//! metadata projection a stored repository is read back through, the timestamp reader
//! its date members are compared with, and the reader that answers a failed call's
//! error code AND the message text that came with it. One definition each, so two
//! suites cannot disagree about what "the stored repository", "that instant" or "that
//! error's message" means.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::ok;

/// The `repositoryMetadata` a `GetRepository` read reports for a name — the one
/// projection the repository suites read a stored repository through.
pub(crate) async fn metadata_of(router: &MultiServiceRouter, name: &str) -> Value {
    let got = ok(router, "GetRepository", &json!({ "repositoryName": name })).await;
    got.get("repositoryMetadata")
        .unwrap_or_else(|| panic!("no repositoryMetadata for {name:?} in {got}"))
        .clone()
}

/// A modelled timestamp member as epoch seconds. awsJson1_1 renders a timestamp as
/// an epoch-seconds number with a fractional part, so instants milliseconds apart
/// are distinguishable here.
pub(crate) fn seconds_at(metadata: &Value, member: &str) -> f64 {
    metadata
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no numeric {member} in {metadata}"))
}

/// The modeled error code AND the message text of a failed call by the default
/// caller, with the status asserted — for a suite whose subject is the message
/// rather than the code.
///
/// `crate::repository_lifecycle::error` answers the code alone, which is what a
/// suite checking WHICH error was raised needs. This is the same parse as
/// [`crate::wire::refusal_as`] — it IS that function, bound to the default
/// principal — so there is one definition of how an error's code and message are
/// read off the wire.
pub(crate) async fn error_with_message(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected_status: u16,
) -> (String, String) {
    crate::wire::refusal_as(
        router,
        crate::wire::DEFAULT_ACCESS_KEY_ID,
        operation,
        body,
        expected_status,
    )
    .await
}
