// SPDX-License-Identifier: CC0-1.0

//! The readers and assertion helpers the repository suites share, over the wire
//! harness this test target already has.
//!
//! Driving the service — building the emulator, signing a call, asserting a status,
//! reading a value out of a payload — is [`crate::repository_lifecycle`]'s harness
//! (itself signing through [`crate::wire`]), and this module adds nothing to it. What
//! lives here is what more than one repository suite needs: the `GetRepository`
//! metadata projection a stored repository is read back through, the timestamp reader
//! its date members are compared with, the reader that answers a failed call's
//! error code AND the message text that came with it, and the wording CodeCommit
//! refuses a `pullRequestId` or a `revisionId` naming nothing with. One definition
//! each, so two suites cannot disagree about what "the stored repository", "that
//! instant", "that error's message" or "that refusal's words" means.

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

/// `InvalidPullRequestIdException`'s message for a pull request id that is not one,
/// as CodeCommit words it: the offending value named in the sentence, then the
/// operation a caller lists the valid ids with.
pub(crate) fn invalid_pull_request_id(pull_request_id: &str) -> String {
    format!(
        "The following pull request ID is not valid: {pull_request_id}. To view a list of valid \
         pull request IDs for this repository, use ListPullRequests."
    )
}

/// `InvalidPullRequestIdException`'s message for a pull request id that was carried
/// but holds no characters: the same sentence, with the clause that says the value
/// was empty or null.
pub(crate) fn empty_pull_request_id(pull_request_id: &str) -> String {
    format!(
        "The following pull request ID is empty or null and therefore not valid: \
         {pull_request_id}. To view a list of valid pull request IDs for this repository, use \
         ListPullRequests."
    )
}

/// `InvalidRevisionIdException`'s message for a revision id that was carried but
/// holds no characters. A revision id's wording names no value.
pub(crate) const EMPTY_REVISION_ID: &str =
    "A revision ID is required, but the value is either empty or null.";

/// `InvalidRevisionIdException`'s message for a revision id that is not the FORM one
/// takes: the requirement, then the length a revision id holds, then where the
/// current one is read from.
pub(crate) const INVALID_REVISION_ID: &str =
    "A revision ID is required, but the value is not valid. Revision IDs are 64 characters in \
     length. To retrieve a valid revision ID, use GetPullRequest.";

/// `PullRequestDoesNotExistException`'s message: a FIXED sentence pair that names no
/// value — not the id the request asked for, and not a repository — and points a
/// caller at the operation that lists the ids that do exist.
pub(crate) const PULL_REQUEST_DOES_NOT_EXIST: &str =
    "The pull request ID does not exist. Verify that you provided a valid pull request ID, and \
     then try your operation again. To get a list of pull request IDs, use ListPullRequests.";
