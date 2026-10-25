// SPDX-License-Identifier: CC0-1.0

//! The `clientRequestToken` a request may carry, over the wire, driven as awsJson1_1
//! requests through the router that serves the real emulator (the wire harness comes
//! from the sibling `repository_lifecycle` suite rather than a second copy).
//!
//! The rule under test: a `clientRequestToken` that is PRESENT in the request has to
//! carry between 1 and 256 characters — the bound the service's own
//! `InvalidClientRequestTokenException` text states for the member. A present token
//! outside it names no earlier request, so it is MALFORMED rather than absent: the
//! call is refused with HTTP 400 `InvalidClientRequestTokenException` and nothing is
//! persisted. A token inside it is accepted whatever characters it carries (the SDKs
//! prepopulate the member with a UUID, hyphens included) and is the idempotency key,
//! so repeating the same request under it answers the first call's resource instead
//! of creating a second one.
//!
//! The tables below span what that rule generalises over: the operation carrying the
//! member (every implemented one), and the token's size and character kind — the
//! empty string, one character, the 256-character bound, one character past it
//! counted in CHARACTERS rather than bytes, a UUID, non-ASCII text and punctuation.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::repository_lifecycle::{base64, emulator_with_engine, error, ok, string_at};

/// The upper bound of the member's documented range ("Client request tokens must be
/// between 1 and 256 ... characters"), which `src/state.rs` holds as
/// `CLIENT_REQUEST_TOKEN_MAX_LENGTH`.
const MAX_TOKEN_CHARACTERS: usize = 256;

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on `InvalidClientRequestTokenException`).
const CLIENT_ERROR_STATUS: u16 = 400;

const REPOSITORY: &str = "token-rules";

/// One repository with two diverged branches and one pull request already open on
/// them: everything the three operations carrying a `clientRequestToken` need.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    pull_request_id: String,
    before_commit: String,
    after_commit: String,
}

async fn fixture() -> Fixture {
    let (router, engine) = emulator_with_engine();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let destination = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "main",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\n"),
            "commitMessage": "the destination version",
        }),
    )
    .await;
    let before_commit = string_at(&destination, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "commitId": before_commit,
        }),
    )
    .await;
    let source = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\nbeta\n"),
            "parentCommitId": before_commit,
            "commitMessage": "the source version",
        }),
    )
    .await;
    let after_commit = string_at(&source, "/commitId");
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": "feature",
                "destinationReference": "main",
            }],
        }),
    )
    .await;
    Fixture {
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
        router,
        engine,
        before_commit,
        after_commit,
    }
}

impl Fixture {
    /// Every implemented operation carrying a `clientRequestToken`: the operation
    /// name, a body that succeeds without a token, and where its answer carries the
    /// id of the resource the call creates.
    fn carriers(&self) -> Vec<(&'static str, Value, &'static str)> {
        vec![
            (
                "CreatePullRequest",
                json!({
                    "title": "a second change to review",
                    "targets": [{
                        "repositoryName": REPOSITORY,
                        "sourceReference": "feature",
                        "destinationReference": "main",
                    }],
                }),
                "/pullRequest/pullRequestId",
            ),
            (
                "PostCommentForPullRequest",
                json!({
                    "pullRequestId": self.pull_request_id,
                    "repositoryName": REPOSITORY,
                    "beforeCommitId": self.before_commit,
                    "afterCommitId": self.after_commit,
                    "content": "a remark on the pull request",
                }),
                "/comment/commentId",
            ),
            (
                "PostCommentForComparedCommit",
                json!({
                    "repositoryName": REPOSITORY,
                    "beforeCommitId": self.before_commit,
                    "afterCommitId": self.after_commit,
                    "content": "a remark on the comparison",
                }),
                "/comment/commentId",
            ),
        ]
    }

    /// What this repository's stored state holds: its pull requests and its
    /// comments. A refused call must leave both exactly as it found them.
    fn stored(&self) -> (usize, usize) {
        let state = self.engine.state::<CodeCommitState>();
        let store = state.lock();
        let repository = store
            .repositories
            .get(REPOSITORY)
            .expect("the repository written above");
        (repository.pull_requests.len(), repository.comments.len())
    }
}

/// Tokens outside the member's range. Rows differ along size and character kind,
/// including a token whose 257 CHARACTERS are more than 257 bytes, so the bound is
/// seen to be counted in characters.
fn malformed_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("the explicitly empty string", String::new()),
        (
            "one character past the bound",
            "x".repeat(MAX_TOKEN_CHARACTERS + 1),
        ),
        (
            "one non-ASCII character past the bound",
            "é".repeat(MAX_TOKEN_CHARACTERS + 1),
        ),
        ("far past the bound", "9".repeat(4 * MAX_TOKEN_CHARACTERS)),
    ]
}

/// Tokens inside it. Rows differ along size and character kind: the one-character
/// minimum, the 256-character maximum, the UUID form an SDK prepopulates the member
/// with, non-ASCII text, and punctuation.
fn accepted_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("one character", "a".to_string()),
        (
            "an SDK-generated UUID, hyphens included",
            "f47ac10b-58cc-4372-a567-0e02b2c3d479".to_string(),
        ),
        (
            "the character at the bound",
            "x".repeat(MAX_TOKEN_CHARACTERS),
        ),
        ("non-ASCII characters", "идемпотентный-ключ".to_string()),
        ("punctuation and spaces", "token 7 / for:retry".to_string()),
    ]
}

#[tokio::test]
async fn a_present_token_outside_the_bound_is_refused_and_persists_nothing() {
    let fixture = fixture().await;
    let before = fixture.stored();

    for (operation, body, _) in fixture.carriers() {
        for (label, token) in malformed_tokens() {
            let case = format!("{operation} with {label}");
            let mut sent = body.clone();
            sent["clientRequestToken"] = json!(token);
            let code = error(&fixture.router, operation, &sent, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "InvalidClientRequestTokenException",
                "case {case:?}: expected InvalidClientRequestTokenException, got {code:?}"
            );
            assert_eq!(
                fixture.stored(),
                before,
                "case {case:?}: a refused call must persist nothing, \
                 expected {before:?} stored (pull requests, comments)"
            );
        }
    }
}

#[tokio::test]
async fn a_present_token_inside_the_bound_is_accepted_and_deduplicates_a_repeat() {
    // One emulator per operation: a comment's idempotency token is held per
    // repository across the comment-posting operations, so two operations sharing a
    // store would see each other's tokens as replays of a different request.
    let carrier_count = fixture().await.carriers().len();
    for index in 0..carrier_count {
        let fixture = fixture().await;
        let (operation, body, id_pointer) = fixture
            .carriers()
            .into_iter()
            .nth(index)
            .expect("the carrier at this index");
        for (label, token) in accepted_tokens() {
            let case = format!("{operation} with {label}");
            let mut sent = body.clone();
            sent["clientRequestToken"] = json!(token);

            let first = ok(&fixture.router, operation, &sent).await;
            let first_id = string_at(&first, id_pointer);
            // The token echoed back is the one the request carried, not a
            // generated one.
            let echoed = first
                .pointer(&format!(
                    "{}/clientRequestToken",
                    id_pointer.rsplit_once('/').expect("a nested pointer").0
                ))
                .and_then(Value::as_str);
            assert_eq!(
                echoed,
                Some(token.as_str()),
                "case {case:?}: the stored resource must report the token sent, got {echoed:?}"
            );

            // The SAME request under the SAME token answers the first call's
            // resource rather than creating a second one.
            let repeated = ok(&fixture.router, operation, &sent).await;
            let repeated_id = string_at(&repeated, id_pointer);
            assert_eq!(
                repeated_id, first_id,
                "case {case:?}: a repeat under the same token must answer the first resource, \
                 got {repeated_id:?} after {first_id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_leaving_the_token_out_creates_a_resource_of_its_own() {
    let fixture = fixture().await;

    // The member is optional: leaving it out names no earlier request, so two such
    // calls are two resources — the behaviour an empty token must NOT be read as.
    for (operation, body, id_pointer) in fixture.carriers() {
        let first = string_at(&ok(&fixture.router, operation, &body).await, id_pointer);
        let second = string_at(&ok(&fixture.router, operation, &body).await, id_pointer);
        assert_ne!(
            first, second,
            "case {operation:?}: two untokened calls must create two resources, got {first:?} twice"
        );
    }
}
