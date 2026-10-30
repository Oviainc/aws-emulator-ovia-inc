// SPDX-License-Identifier: CC0-1.0

//! The TEXT CodeCommit puts on `ManualMergeRequiredException`, over the wire.
//!
//! The rule under test: every operation that refuses a merge because the merge
//! cannot be made automatically answers ONE fixed sentence — "Current branches
//! cannot be merged, please try to merge using a Git client locally." — whatever it
//! merges and whichever way the merge was asked for. The sentence interpolates
//! nothing: not the repository, not either reference, not a commit id, and not the
//! path the two sides disagree on. The status and the code come from the model
//! (`smithy_query(query="shape",shape="ManualMergeRequiredException")`: `errorKind`
//! client, `httpError` 400), and the refusal writes nothing.
//!
//! The table below spans what the rule generalises over rather than one recorded
//! request: WHICH operation refuses (all three pull-request merge strategies, all
//! three branch merge strategies, and the unreferenced merge, which asks for the
//! same merge without a target reference), and WHY it cannot be made automatically —
//! a path the two sides changed differently on the same line, and two histories
//! neither of which contains the other, which no reference move can join.
//!
//! The sibling merge suites own what a merge that GOES THROUGH produces; this suite
//! owns only the wording of this refusal.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, ok, string_at};
use crate::unreferenced_merge_commits::{commit, Change};
use crate::wire::fresh_client_request_token;

/// The status the model gives `ManualMergeRequiredException` (`errorKind: client`,
/// `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

const CODE: &str = "ManualMergeRequiredException";

/// The one sentence the service answers that code with.
const EXPECTED_MESSAGE: &str =
    "Current branches cannot be merged, please try to merge using a Git client locally.";

const REPOSITORY: &str = "manual-merge-wording";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// The path the two sides change differently, which is what makes the merge
/// unmergeable — and a value the message must NOT carry.
const CONFLICTING_PATH: &str = "shared/config.txt";

/// A repository whose two branches changed the same line of one file, plus the pull
/// request between them. Answers the two tips and the pull request id, so a row can
/// assert that the refusal moved neither.
struct Diverged {
    router: MultiServiceRouter,
    destination_tip: String,
    source_tip: String,
    pull_request_id: String,
}

async fn diverged() -> Diverged {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let base = commit(
        &router,
        REPOSITORY,
        DESTINATION,
        None,
        &[Change::Put(CONFLICTING_PATH, "root\n")],
        "the shared base",
    )
    .await;
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    let destination_tip = commit(
        &router,
        REPOSITORY,
        DESTINATION,
        Some(&base),
        &[Change::Put(CONFLICTING_PATH, "destination\n")],
        "the destination edit",
    )
    .await;
    let source_tip = commit(
        &router,
        REPOSITORY,
        SOURCE,
        Some(&base),
        &[Change::Put(CONFLICTING_PATH, "source\n")],
        "the source edit",
    )
    .await;
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": "a change that cannot be merged automatically",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    Diverged {
        router,
        destination_tip,
        source_tip,
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
    }
}

/// One row: the operation refused, and the request body built from the fixture.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: fn(&Diverged) -> Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a pull request merged by fast forward, with both branches moved",
            operation: "MergePullRequestByFastForward",
            body: |fixture| {
                json!({
                    "repositoryName": REPOSITORY,
                    "pullRequestId": fixture.pull_request_id,
                })
            },
        },
        Row {
            case: "a pull request merged by squash, with the line conflict unresolved",
            operation: "MergePullRequestBySquash",
            body: |fixture| {
                json!({
                    "repositoryName": REPOSITORY,
                    "pullRequestId": fixture.pull_request_id,
                })
            },
        },
        Row {
            case: "a pull request merged three way, with the line conflict unresolved",
            operation: "MergePullRequestByThreeWay",
            body: |fixture| {
                json!({
                    "repositoryName": REPOSITORY,
                    "pullRequestId": fixture.pull_request_id,
                })
            },
        },
        Row {
            case: "two branches merged by fast forward, with both moved",
            operation: "MergeBranchesByFastForward",
            body: |_| {
                json!({
                    "repositoryName": REPOSITORY,
                    "sourceCommitSpecifier": SOURCE,
                    "destinationCommitSpecifier": DESTINATION,
                    "targetBranch": DESTINATION,
                })
            },
        },
        Row {
            case: "two branches merged by squash, with the line conflict unresolved",
            operation: "MergeBranchesBySquash",
            body: |_| {
                json!({
                    "repositoryName": REPOSITORY,
                    "sourceCommitSpecifier": SOURCE,
                    "destinationCommitSpecifier": DESTINATION,
                    "targetBranch": DESTINATION,
                })
            },
        },
        Row {
            case: "two branches merged three way, with the line conflict unresolved",
            operation: "MergeBranchesByThreeWay",
            body: |_| {
                json!({
                    "repositoryName": REPOSITORY,
                    "sourceCommitSpecifier": SOURCE,
                    "destinationCommitSpecifier": DESTINATION,
                    "targetBranch": DESTINATION,
                })
            },
        },
        Row {
            case: "two branches merged three way by commit id rather than by name",
            operation: "MergeBranchesByThreeWay",
            body: |fixture| {
                json!({
                    "repositoryName": REPOSITORY,
                    "sourceCommitSpecifier": fixture.source_tip,
                    "destinationCommitSpecifier": fixture.destination_tip,
                    "targetBranch": DESTINATION,
                })
            },
        },
        Row {
            case: "an unreferenced three way merge of the two tips",
            operation: "CreateUnreferencedMergeCommit",
            body: |fixture| {
                json!({
                    "repositoryName": REPOSITORY,
                    "sourceCommitSpecifier": fixture.source_tip,
                    "destinationCommitSpecifier": fixture.destination_tip,
                    "mergeOption": "THREE_WAY_MERGE",
                })
            },
        },
        Row {
            case: "an unreferenced squash merge of the two tips",
            operation: "CreateUnreferencedMergeCommit",
            body: |_| {
                json!({
                    "repositoryName": REPOSITORY,
                    "sourceCommitSpecifier": SOURCE,
                    "destinationCommitSpecifier": DESTINATION,
                    "mergeOption": "SQUASH_MERGE",
                })
            },
        },
    ]
}

#[tokio::test]
async fn every_merge_that_cannot_be_made_automatically_is_refused_in_the_same_words() {
    let fixture = diverged().await;

    for row in rows() {
        let case = row.case;
        let (code, message) = error_with_message(
            &fixture.router,
            row.operation,
            &(row.body)(&fixture),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, CODE,
            "case {case:?}: expected the modelled {CODE}, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, EXPECTED_MESSAGE,
            "case {case:?}: expected the exact sentence AWS words this class with, got {message:?}"
        );
        // The sentence is fixed: nothing of the request or of the stored state is in
        // it, so a client matching it matches on every repository.
        for named in [
            REPOSITORY,
            DESTINATION,
            SOURCE,
            CONFLICTING_PATH,
            fixture.destination_tip.as_str(),
            fixture.source_tip.as_str(),
            fixture.pull_request_id.as_str(),
        ] {
            assert!(
                !message.contains(named),
                "case {case:?}: the message must interpolate nothing, but {message:?} carries \
                 {named:?}"
            );
        }
    }

    // Every refusal above wrote nothing: both branches are where they were and the
    // pull request is still open.
    for (branch, tip) in [
        (DESTINATION, &fixture.destination_tip),
        (SOURCE, &fixture.source_tip),
    ] {
        let read = ok(
            &fixture.router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": branch }),
        )
        .await;
        assert_eq!(
            &string_at(&read, "/branch/commitId"),
            tip,
            "case {branch:?}: a refused merge must leave the branch where it was, got {read}"
        );
    }
    let read = ok(
        &fixture.router,
        "GetPullRequest",
        &json!({ "pullRequestId": fixture.pull_request_id }),
    )
    .await;
    assert_eq!(
        string_at(&read, "/pullRequest/pullRequestStatus"),
        "OPEN",
        "a refused merge must leave the pull request open, got {read}"
    );
}

#[tokio::test]
async fn a_merge_that_can_be_made_is_still_served() {
    // The positive control for the table above: with the conflict resolved — the
    // source taking the destination's version of the line — the same three-way merge
    // between the same two branches goes through, so the rows above are refusals
    // about the merge rather than about the request.
    let fixture = diverged().await;
    commit(
        &fixture.router,
        REPOSITORY,
        SOURCE,
        Some(&fixture.source_tip),
        &[Change::Put(CONFLICTING_PATH, "destination\n")],
        "resolve by taking the destination version",
    )
    .await;
    let merged = ok(
        &fixture.router,
        "MergeBranchesByThreeWay",
        &json!({
            "repositoryName": REPOSITORY,
            "sourceCommitSpecifier": SOURCE,
            "destinationCommitSpecifier": DESTINATION,
            "targetBranch": DESTINATION,
        }),
    )
    .await;
    let commit_id = string_at(&merged, "/commitId");
    let read = ok(
        &fixture.router,
        "GetBranch",
        &json!({ "repositoryName": REPOSITORY, "branchName": DESTINATION }),
    )
    .await;
    assert_eq!(
        string_at(&read, "/branch/commitId"),
        commit_id,
        "the resolved merge must advance the target branch to the merge commit, got {read}"
    );
}
