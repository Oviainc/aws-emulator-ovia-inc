// SPDX-License-Identifier: CC0-1.0

//! REPEATING a merge of a pull request, over the wire.
//!
//! The rule under test: a merge of a pull request that already records a merge is the
//! idempotent re-read of the merge that happened, not a second attempt at it — HTTP 200
//! carrying the stored pull request with its `mergeMetadata` unchanged (the same
//! `mergeCommitId`, `mergedBy`, `mergeOption`), the destination reference left where the
//! merge put it and nothing written — while `PullRequestAlreadyClosedException` stays the
//! answer for a pull request closed WITHOUT a merge, which has no merge to report.
//!
//! The rows span what that rule generalises over: which of the three merge strategies made
//! the merge, which strategy the repeat asks for (the same one, and each of the others —
//! the merge a pull request carries is one fact, so the strategy the retry picks cannot
//! change the answer), how many times it is repeated, and whether the two branches were
//! fast-forwardable or had both moved apart.
//!
//! Every call goes through the shared wire harness in [`crate::repository_lifecycle`], so
//! these suites drive one emulator over one protocol.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, is_object_id, ok, string_at};
use crate::wire::fresh_client_request_token;

/// The three merge operations a pull request can be merged by, and the `mergeOption`
/// each records (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`).
const FAST_FORWARD: Strategy = Strategy {
    operation: "MergePullRequestByFastForward",
    merge_option: "FAST_FORWARD_MERGE",
};
const SQUASH: Strategy = Strategy {
    operation: "MergePullRequestBySquash",
    merge_option: "SQUASH_MERGE",
};
const THREE_WAY: Strategy = Strategy {
    operation: "MergePullRequestByThreeWay",
    merge_option: "THREE_WAY_MERGE",
};

/// One merge strategy: the operation that asks for it and the `mergeOption` a merge made
/// with it records.
#[derive(Debug, Clone, Copy)]
struct Strategy {
    operation: &'static str,
    merge_option: &'static str,
}

/// The `PullRequestStatusEnum` values a pull request holds
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The status every error asserted here models (`errorKind: client`, `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The sentence `PullRequestAlreadyClosedException` carries on a MERGE, written out
/// here rather than read from the service, so the refusal is checked against the
/// words CodeCommit answers a merge of a closed pull request with — which are not
/// the ones it answers an attempt to update its status with.
const ALREADY_CLOSED: &str =
    "The specified pull request is marked as Closed. A closed pull request cannot be merged.";

const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// One repository with two branches and one open pull request between them.
struct Opened {
    router: MultiServiceRouter,
    pull_request_id: String,
    /// The tip the destination branch holds before any merge.
    destination_tip: String,
}

/// Open a pull request from [`SOURCE`] to [`DESTINATION`] in `repository`.
///
/// `diverged` decides whether the destination branch has moved on since the source branch
/// left it: two branches that each carry a commit the other does not cannot be joined by
/// moving a reference, which is the case a fast-forward cannot serve and the two merging
/// strategies settle with a merge commit. The two sides touch different paths, so the
/// content never conflicts.
async fn open_pull_request(repository: &str, diverged: bool) -> Opened {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let seeded = ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "filePath": "README.md",
            "fileContent": base64(b"the released version\n"),
            "commitMessage": "base",
        }),
    )
    .await;
    let base = string_at(&seeded, "/commitId");
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    ok(
        &router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "filePath": "feature.txt",
            "fileContent": base64(b"the proposed change\n"),
            "parentCommitId": base,
            "commitMessage": "proposed change",
        }),
    )
    .await;
    let mut destination_tip = base.clone();
    if diverged {
        let moved_on = ok(
            &router,
            "PutFile",
            &json!({
                "repositoryName": repository,
                "branchName": DESTINATION,
                "filePath": "CHANGELOG.md",
                "fileContent": base64(b"released since\n"),
                "parentCommitId": base,
                "commitMessage": "the destination moves on",
            }),
        )
        .await;
        destination_tip = string_at(&moved_on, "/commitId");
    }
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": "a change to land",
            "targets": [{
                "repositoryName": repository,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    Opened {
        router,
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
        destination_tip,
    }
}

/// The merge request one strategy takes: the pull request it merges, plus the commit
/// message the two strategies that WRITE a commit carry.
fn merge_request(strategy: Strategy, repository: &str, pull_request_id: &str) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "pullRequestId": pull_request_id,
    });
    if strategy.operation != FAST_FORWARD.operation {
        input["commitMessage"] = json!("land the change");
    }
    input
}

fn pull_request_of(payload: &Value, case: &str) -> Value {
    payload
        .get("pullRequest")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no pullRequest in {payload}"))
}

fn target_of(pull_request: &Value, case: &str) -> Value {
    pull_request
        .get("pullRequestTargets")
        .and_then(Value::as_array)
        .and_then(|targets| targets.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no pullRequestTargets in {pull_request}"))
}

async fn stored_pull_request(router: &MultiServiceRouter, pull_request_id: &str) -> Value {
    let payload = ok(
        router,
        "GetPullRequest",
        &json!({ "pullRequestId": pull_request_id }),
    )
    .await;
    pull_request_of(&payload, "the stored pull request")
}

async fn branch_tip(router: &MultiServiceRouter, repository: &str, branch: &str) -> String {
    let payload = ok(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": branch }),
    )
    .await;
    string_at(&payload, "/branch/commitId")
}

/// One row: how the pull request was merged, which strategies the merge is then asked for
/// again, and whether the two branches had moved apart.
struct Row {
    case: &'static str,
    repository: &'static str,
    diverged: bool,
    merged_with: Strategy,
    repeated_with: &'static [Strategy],
}

#[tokio::test]
async fn a_merge_of_an_already_merged_pull_request_answers_the_merge_that_happened() {
    let rows = [
        Row {
            case: "a fast-forward merge asked for twice more",
            repository: "repeat-fast-forward",
            diverged: false,
            merged_with: FAST_FORWARD,
            repeated_with: &[FAST_FORWARD, FAST_FORWARD],
        },
        Row {
            case: "a fast-forward merge retried with the two writing strategies",
            repository: "repeat-fast-forward-crosswise",
            diverged: false,
            merged_with: FAST_FORWARD,
            repeated_with: &[SQUASH, THREE_WAY],
        },
        Row {
            case: "a squash merge of a fast-forwardable pull request asked for again",
            repository: "repeat-squash-fast-forwardable",
            diverged: false,
            merged_with: SQUASH,
            repeated_with: &[SQUASH],
        },
        Row {
            case: "a squash merge of two branches that moved apart, retried every way",
            repository: "repeat-squash-diverged",
            diverged: true,
            merged_with: SQUASH,
            repeated_with: &[SQUASH, FAST_FORWARD, THREE_WAY],
        },
        Row {
            case: "a three-way merge of two branches that moved apart, asked for again",
            repository: "repeat-three-way-diverged",
            diverged: true,
            merged_with: THREE_WAY,
            repeated_with: &[THREE_WAY, SQUASH],
        },
        Row {
            case: "a three-way merge of a fast-forwardable pull request, retried by \
                   fast-forward",
            repository: "repeat-three-way-fast-forwardable",
            diverged: false,
            merged_with: THREE_WAY,
            repeated_with: &[FAST_FORWARD],
        },
    ];

    for row in rows {
        let case = row.case;
        let built = open_pull_request(row.repository, row.diverged).await;

        // The merge that happens: it closes the pull request and records what was
        // merged, with which strategy.
        let answered = ok(
            &built.router,
            row.merged_with.operation,
            &merge_request(row.merged_with, row.repository, &built.pull_request_id),
        )
        .await;
        let merged = pull_request_of(&answered, case);
        assert_eq!(
            merged.get("pullRequestStatus").and_then(Value::as_str),
            Some(STATUS_CLOSED),
            "case {case:?}: a merged pull request is CLOSED, got {merged}"
        );
        let target = target_of(&merged, case);
        assert_eq!(
            target
                .pointer("/mergeMetadata/isMerged")
                .and_then(Value::as_bool),
            Some(true),
            "case {case:?}: the merge is recorded on the target, got {target}"
        );
        assert_eq!(
            target
                .pointer("/mergeMetadata/mergeOption")
                .and_then(Value::as_str),
            Some(row.merged_with.merge_option),
            "case {case:?}: expected the strategy {}, got {target}",
            row.merged_with.merge_option
        );
        let merge_commit = string_at(&target, "/mergeMetadata/mergeCommitId");
        assert!(
            is_object_id(&merge_commit),
            "case {case:?}: expected a commit id for the merge, got {merge_commit:?}"
        );
        let landed = branch_tip(&built.router, row.repository, DESTINATION).await;
        assert_eq!(
            landed, merge_commit,
            "case {case:?}: the destination branch holds the merge {merge_commit}, got {landed:?}"
        );
        assert_ne!(
            landed, built.destination_tip,
            "case {case:?}: the merge advances the destination branch past {}",
            built.destination_tip
        );

        // Every repeat of that merge, whichever strategy asks for it, answers the merge
        // the pull request already carries and writes nothing.
        for (attempt, strategy) in row.repeated_with.iter().enumerate() {
            let again = ok(
                &built.router,
                strategy.operation,
                &merge_request(*strategy, row.repository, &built.pull_request_id),
            )
            .await;
            let repeated = pull_request_of(&again, case);
            assert_eq!(
                repeated, merged,
                "case {case:?}: repeat {attempt} by {} answers the merge already recorded \
                 {merged}, got {repeated}",
                strategy.operation
            );
            let tip = branch_tip(&built.router, row.repository, DESTINATION).await;
            assert_eq!(
                tip, merge_commit,
                "case {case:?}: repeat {attempt} by {} left the destination branch at \
                 {merge_commit}, got {tip:?}",
                strategy.operation
            );
            let stored = stored_pull_request(&built.router, &built.pull_request_id).await;
            assert_eq!(
                stored, merged,
                "case {case:?}: repeat {attempt} by {} changed the stored pull request to \
                 {stored}",
                strategy.operation
            );
        }
    }
}

#[tokio::test]
async fn a_pull_request_closed_without_a_merge_refuses_every_merge_strategy() {
    // Rows differ along the strategy the merge of the closed pull request asks for: a
    // pull request closed by its author carries no merge to report, whichever strategy
    // the caller names.
    for strategy in [FAST_FORWARD, SQUASH, THREE_WAY] {
        let case = strategy.operation;
        let repository = "closed-without-a-merge";
        let built = open_pull_request(repository, true).await;
        let closed = ok(
            &built.router,
            "UpdatePullRequestStatus",
            &json!({
                "pullRequestId": built.pull_request_id,
                "pullRequestStatus": STATUS_CLOSED,
            }),
        )
        .await;
        assert_eq!(
            string_at(&closed, "/pullRequest/pullRequestStatus"),
            STATUS_CLOSED,
            "case {case:?}: the pull request is closed before the merge, got {closed}"
        );

        let (code, message) = error_with_message(
            &built.router,
            strategy.operation,
            &merge_request(strategy, repository, &built.pull_request_id),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestAlreadyClosedException",
            "case {case:?}: expected PullRequestAlreadyClosedException, got {code:?}"
        );
        assert_eq!(
            message, ALREADY_CLOSED,
            "case {case:?}: expected {ALREADY_CLOSED:?}, got {message:?}"
        );

        // The refusal wrote nothing: the destination branch is where it was and the
        // target still records no merge.
        let tip = branch_tip(&built.router, repository, DESTINATION).await;
        assert_eq!(
            tip, built.destination_tip,
            "case {case:?}: expected the destination branch left at {}, got {tip:?}",
            built.destination_tip
        );
        let stored = stored_pull_request(&built.router, &built.pull_request_id).await;
        let target = target_of(&stored, case);
        assert_eq!(
            target
                .pointer("/mergeMetadata/isMerged")
                .and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a refused merge leaves the target unmerged, got {target}"
        );
        assert_ne!(
            stored.get("pullRequestStatus").and_then(Value::as_str),
            Some(STATUS_OPEN),
            "case {case:?}: the closed pull request is not re-opened, got {stored}"
        );
    }
}
