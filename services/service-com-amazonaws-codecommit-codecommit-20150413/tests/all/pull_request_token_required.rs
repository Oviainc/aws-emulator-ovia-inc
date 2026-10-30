// SPDX-License-Identifier: CC0-1.0

//! The `clientRequestToken` `CreatePullRequest` REQUIRES on the wire.
//!
//! The rule under test: `CreatePullRequest` will not open a pull request for a
//! request whose body carries no `clientRequestToken` at all. Such a request is
//! refused with the modelled `ClientRequestTokenRequiredException`
//! (`smithy_query(query="shape",shape="ClientRequestTokenRequiredException")`:
//! `errorKind` client, `httpError` 400) and the fixed sentence "A client request
//! token is required.", before any pull request id is allocated — so the account's
//! pull requests are exactly what they were. An AWS SDK fills the member in from the
//! model's `@idempotencyToken` before sending, which is why a caller using one never
//! sees this; a caller that signs its own request does.
//!
//! The rows below span what the rule generalises over rather than one recorded
//! request: the rest of the request the tokenless body carries — the title's text and
//! length, whether a description is sent, each form the two references take (short
//! name or fully qualified) and whether the destination is named at all — none of
//! which makes a tokenless request acceptable. A token that is PRESENT and holds no
//! character is a DIFFERENT failure (it is malformed rather than absent) and is
//! asserted here too, against the suite that owns the member's bounds
//! (`tests/all/client_request_token_validation.rs`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, ok, string_at};

/// The status the model gives both token refusals.
const CLIENT_ERROR_STATUS: u16 = 400;

const CODE: &str = "ClientRequestTokenRequiredException";

/// The fixed sentence the service answers that code with.
const EXPECTED_MESSAGE: &str = "A client request token is required.";

const REPOSITORY: &str = "pull-request-token-required";

/// A repository with two diverged branches, ready for a pull request between them.
async fn diverged() -> MultiServiceRouter {
    let router = emulator();
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
    let base = string_at(&destination, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "commitId": base,
        }),
    )
    .await;
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": "feature",
            "filePath": "README.md",
            "fileContent": base64(b"alpha\nbeta\n"),
            "parentCommitId": base,
            "commitMessage": "the source version",
        }),
    )
    .await;
    router
}

/// The pull request ids the repository holds.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok(
        router,
        "ListPullRequests",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    listed
        .get("pullRequestIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The tokenless bodies. Rows differ along everything else the request carries.
fn tokenless_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "both references by short name",
            json!({
                "title": "a change to review",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "feature",
                    "destinationReference": "main",
                }],
            }),
        ),
        (
            "both references fully qualified",
            json!({
                "title": "a change to review",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "refs/heads/feature",
                    "destinationReference": "refs/heads/main",
                }],
            }),
        ),
        (
            "the destination left to the default branch",
            json!({
                "title": "a change to review",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "feature",
                }],
            }),
        ),
        (
            "a description as well",
            json!({
                "title": "a change to review",
                "description": "what it changes, and why",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "feature",
                    "destinationReference": "main",
                }],
            }),
        ),
        (
            "a one-character title",
            json!({
                "title": "t",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "feature",
                    "destinationReference": "main",
                }],
            }),
        ),
        (
            "a title at the modelled maximum of 150 characters",
            json!({
                "title": "t".repeat(150),
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "feature",
                    "destinationReference": "main",
                }],
            }),
        ),
        (
            "a unicode title",
            json!({
                "title": "revisión obligatoria ⚙",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": "feature",
                    "destinationReference": "main",
                }],
            }),
        ),
    ]
}

#[tokio::test]
async fn a_create_pull_request_carrying_no_token_is_refused_and_opens_nothing() {
    let router = diverged().await;

    for (case, body) in tokenless_bodies() {
        let (code, message) =
            error_with_message(&router, "CreatePullRequest", &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, CODE,
            "case {case:?}: expected the modelled {CODE}, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, EXPECTED_MESSAGE,
            "case {case:?}: expected the exact sentence AWS words this class with, got {message:?}"
        );
        assert!(
            listed_ids(&router).await.is_empty(),
            "case {case:?}: a refused create must open no pull request"
        );
    }

    // A token that is PRESENT and empty names no earlier request, so it is malformed
    // rather than absent: a different code, and still nothing opened.
    let mut empty_token = tokenless_bodies()
        .into_iter()
        .next()
        .expect("the first row")
        .1;
    empty_token["clientRequestToken"] = json!("");
    let (code, message) = error_with_message(
        &router,
        "CreatePullRequest",
        &empty_token,
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "InvalidClientRequestTokenException",
        "case \"an empty token\": a present token that names nothing is malformed rather than \
         absent, got {code:?} saying {message:?}"
    );
    assert!(
        listed_ids(&router).await.is_empty(),
        "case \"an empty token\": a refused create must open no pull request"
    );

    // The positive control: the same bodies WITH a token each open a pull request, so
    // the rows above are refusals about the token and nothing else.
    let mut opened: Vec<String> = Vec::new();
    for (index, (case, mut body)) in tokenless_bodies().into_iter().enumerate() {
        body["clientRequestToken"] = json!(format!("token-{index}"));
        let created = ok(&router, "CreatePullRequest", &body).await;
        let id = string_at(&created, "/pullRequest/pullRequestId");
        assert_eq!(
            string_at(&created, "/pullRequest/clientRequestToken"),
            format!("token-{index}"),
            "case {case:?}: the stored pull request reports the token sent, got {created}"
        );
        opened.push(id);
    }
    let mut listed = listed_ids(&router).await;
    listed.sort();
    opened.sort();
    assert_eq!(
        listed, opened,
        "the repository must hold exactly the pull requests the tokened calls opened"
    );
}
