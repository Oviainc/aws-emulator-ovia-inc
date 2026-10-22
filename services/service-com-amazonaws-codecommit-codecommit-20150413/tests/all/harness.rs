// SPDX-License-Identifier: CC0-1.0

//! The readers the repository suites share, over the wire harness this test target
//! already has.
//!
//! Driving the service — building the emulator, signing a call, asserting a status,
//! reading a value out of a payload — is [`crate::repository_lifecycle`]'s harness
//! (itself signing through [`crate::wire`]), and this module adds nothing to it. What
//! lives here is the pair of readers more than one repository suite needs: the
//! `GetRepository` metadata projection a stored repository is read back through, and
//! the timestamp reader its date members are compared with. One definition each, so
//! two suites cannot disagree about what "the stored repository" or "that instant"
//! means.

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
