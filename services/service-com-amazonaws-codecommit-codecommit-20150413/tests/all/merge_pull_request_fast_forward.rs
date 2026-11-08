// SPDX-License-Identifier: CC0-1.0

//! `MergePullRequestByFastForward` over the wire, driven as awsJson1_1 requests
//! through the router that serves the real emulator.
//!
//! The rule under test is the fast-forward merge itself: it merges a pull request by
//! MOVING its destination reference rather than by writing a commit, so the only
//! thing that decides it is how the current tips of its two references are related
//! by ancestry. The merge is made exactly where the destination tip IS the merge
//! base of the two — there the destination branch is left at the SOURCE tip — and
//! every other relation (the source already in the destination's history, two
//! histories neither of which contains the other) is refused with HTTP 400
//! `ManualMergeRequiredException`, nothing being written, because a reference that
//! cannot advance cannot honour both sides. A merge that goes
//! through closes the pull request and records what was merged, who merged it, the
//! commit the destination branch then holds and the `FAST_FORWARD_MERGE` strategy.
//!
//! The tables below span what the rule generalises over rather than one recorded
//! request: which side moved, by how many commits, what kind of change each commit
//! carries (content, a new file, a nested path, a deletion, a file mode), whether the
//! request pins `sourceCommitId` and to which commit, which part of the request names
//! nothing mergeable, and whether the pull request's approval rules are met.
//!
//! Every merged row's expectation is computed here from the rule: the merge commit is
//! one of the two tips this file already knows (so a merge that WROTE a commit fails
//! the row), the destination branch is read back through `GetBranch`, its tree
//! through `GetFolder` as `(blobId, fileMode)` per path — content-addressed — and the
//! pull request through `GetPullRequest`, so the stored record rather than the
//! merge's own answer is what the assertions hold.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, emulator_with_engine, error, git_blob_id, ok, string_at};
use crate::unreferenced_merge_commits::{commit, commit_of, parents_of, tree_of, Change, EXECUTABLE, NORMAL};
use crate::wire::{caller_arn, fresh_client_request_token, DEFAULT_ACCESS_KEY_ID};

/// The two branches every pull request below is between: `main` is the destination
/// and `feature` the source.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// The operation under test, the `MergeOptionTypeEnum` value it records and the
/// `PullRequestStatusEnum` values a pull request holds before and after a merge
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const OPERATION: &str = "MergePullRequestByFastForward";
const FAST_FORWARD: &str = "FAST_FORWARD_MERGE";
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The one sentence `ManualMergeRequiredException` carries — the words the SERVICE
/// answers that class with on every merge path, which name no branch, no commit and
/// no repository (`src/state.rs`'s `MANUAL_MERGE_REQUIRED_MESSAGE`), rather than the
/// prose the model documents on the shape.
const MANUAL_MERGE_MESSAGE: &str =
    "Current branches cannot be merged, please try to merge using a Git client locally.";

/// The one sentence pair `PullRequestApprovalRulesNotSatisfiedException` carries,
/// whichever rule went unmet.
const UNSATISFIED_MESSAGE: &str =
    "You cannot merge the pull request because the requirements for one or more approval rules \
     have not been met. For details about the approval rules that still have requirements, use \
     EvaluatePullRequestApprovalRules.";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// `ApprovalState`'s value for an approval that counts
/// (`smithy_query(query="shape",shape="ApprovalState")`).
const APPROVE: &str = "APPROVE";

/// What the merge must answer for one row of the merge table.
#[derive(Debug, Clone)]
enum Expected {
    /// The merge goes through, which can only leave the destination branch at the
    /// SOURCE tip: the tree readable there afterwards.
    Merged(Vec<(&'static str, &'static str, &'static str)>),
    /// The modelled error code the request is refused with.
    Refused(&'static str),
}

/// What `sourceCommitId` the merge request names.
#[derive(Debug, Clone, Copy)]
enum Pin {
    /// The member is left out.
    Absent,
    /// The source branch's current tip.
    SourceTip,
    /// The base commit: a real commit, but not the source branch's tip.
    Base,
}

/// One row of the merge table: the base commit, the commits each side adds to it,
/// what the request pins, and what the merge must do.
struct Row {
    case: &'static str,
    base: Vec<(&'static str, &'static str)>,
    /// One entry per commit written on the destination branch after the base.
    destination: Vec<Vec<Change>>,
    /// One entry per commit written on the source branch after the base.
    source: Vec<Vec<Change>>,
    pin: Pin,
    expected: Expected,
}

impl Row {
    fn new(
        case: &'static str,
        base: Vec<(&'static str, &'static str)>,
        destination: Vec<Vec<Change>>,
        source: Vec<Vec<Change>>,
        expected: Expected,
    ) -> Self {
        Self {
            case,
            base,
            destination,
            source,
            pin: Pin::Absent,
            expected,
        }
    }

    fn pinning(mut self, pin: Pin) -> Self {
        self.pin = pin;
        self
    }
}

/// The merge table: rows differ along which side moved, by how many commits, what
/// each commit changes, and what the request pins the source tip to.
fn rows() -> Vec<Row> {
    vec![
        // The destination tip is the merge base and the source added one commit:
        // the reference advances to the source tip.
        Row::new(
            "source-one-commit-ahead",
            vec![("README.md", "# base\n")],
            vec![],
            vec![vec![Change::Put("src/only.txt", "source\n")]],
            Expected::Merged(vec![
                ("README.md", "# base\n", NORMAL),
                ("src/only.txt", "source\n", NORMAL),
            ]),
        ),
        // Several commits ahead rather than one: the advance is over the whole run
        // of them, so the tree is the last commit's.
        Row::new(
            "source-three-commits-ahead",
            vec![("a.txt", "one\n")],
            vec![],
            vec![
                vec![Change::Put("a.txt", "two\n")],
                vec![Change::Put("deep/a/b/c.txt", "nested\n")],
                vec![Change::PutMode("run.sh", "#!/bin/sh\n", EXECUTABLE)],
            ],
            Expected::Merged(vec![
                ("a.txt", "two\n", NORMAL),
                ("deep/a/b/c.txt", "nested\n", NORMAL),
                ("run.sh", "#!/bin/sh\n", EXECUTABLE),
            ]),
        ),
        // A run of commits that DELETES what the base held: a fast-forward carries
        // whatever the source's history did, deletions included.
        Row::new(
            "source-ahead-deleting-a-path",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![],
            vec![
                vec![Change::Put("b/d.txt", "another\n")],
                vec![Change::Delete("b/c.txt")],
            ],
            Expected::Merged(vec![
                ("a.txt", "one\n", NORMAL),
                ("b/d.txt", "another\n", NORMAL),
            ]),
        ),
        // The same advance with `sourceCommitId` naming the tip the branch holds:
        // the merge proceeds exactly as it does without it.
        Row::new(
            "source-ahead-with-the-source-tip-pinned",
            vec![("README.md", "# base\n")],
            vec![],
            vec![vec![Change::Put("src/only.txt", "source\n")]],
            Expected::Merged(vec![
                ("README.md", "# base\n", NORMAL),
                ("src/only.txt", "source\n", NORMAL),
            ]),
        )
        .pinning(Pin::SourceTip),
        // "Pass this value if you want an exception thrown if the current commit ID
        // of the tip of the source branch does not match this commit ID": the base
        // is a real commit, but not the tip.
        Row::new(
            "source-ahead-pinned-to-the-base",
            vec![("README.md", "# base\n")],
            vec![],
            vec![vec![Change::Put("src/only.txt", "source\n")]],
            Expected::Refused("TipOfSourceReferenceIsDifferentException"),
        )
        .pinning(Pin::Base),
        // Only the destination moved: the source tip is already in the
        // destination's history, so the destination tip is NOT the merge base and
        // the reference cannot advance \u2014 the merge is refused rather than reported
        // as the destination's own tip, exactly as `GetMergeOptions` offers no
        // fast-forward for this pair.
        //
        // (Neither side moving is not a row: two references holding the same tip
        // are two sides with nothing between them, so no pull request can be
        // opened over them at all \u2014 see `pull_request_creation`.)
        Row::new(
            "only-the-destination-moved",
            vec![("a.txt", "one\n")],
            vec![vec![Change::Put("d.txt", "destination\n")]],
            vec![],
            Expected::Refused("ManualMergeRequiredException"),
        ),
        // Both sides moved, on paths that do not even overlap: neither tip is the
        // other's ancestor, so no reference move can join them however mergeable
        // their content is.
        Row::new(
            "both-sides-moved-on-disjoint-paths",
            vec![("a.txt", "one\n")],
            vec![vec![Change::Put("d.txt", "destination\n")]],
            vec![vec![Change::Put("s.txt", "source\n")]],
            Expected::Refused("ManualMergeRequiredException"),
        ),
        // Both sides changed the same path: the same refusal, for the same reason —
        // the relation between the tips, not the content.
        Row::new(
            "both-sides-moved-on-one-path",
            vec![("a.txt", "one\n")],
            vec![vec![Change::Put("a.txt", "destination\n")]],
            vec![vec![Change::Put("a.txt", "source\n")]],
            Expected::Refused("ManualMergeRequiredException"),
        ),
    ]
}

/// A repository with `main` at the base commit plus the destination's commits and
/// `feature` branched from the base plus the source's, and one OPEN pull request
/// between them.
struct Opened {
    router: MultiServiceRouter,
    pull_request_id: String,
    base_commit: String,
    destination_tip: String,
    source_tip: String,
}

/// Open that repository and pull request on `router`.
async fn open_pull_request(
    router: MultiServiceRouter,
    repository: &str,
    title: &str,
    base: &[(&'static str, &'static str)],
    destination: &[Vec<Change>],
    source: &[Vec<Change>],
) -> Opened {
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base_changes: Vec<Change> = base
        .iter()
        .map(|(path, content)| Change::Put(path, content))
        .collect();
    let base_commit = commit(
        &router,
        repository,
        DESTINATION,
        None,
        &base_changes,
        "the base",
    )
    .await;
    ok(
        &router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base_commit,
        }),
    )
    .await;
    let mut destination_tip = base_commit.clone();
    for (index, changes) in destination.iter().enumerate() {
        destination_tip = commit(
            &router,
            repository,
            DESTINATION,
            Some(&destination_tip),
            changes,
            &format!("on the destination {index}"),
        )
        .await;
    }
    let mut source_tip = base_commit.clone();
    for (index, changes) in source.iter().enumerate() {
        source_tip = commit(
            &router,
            repository,
            SOURCE,
            Some(&source_tip),
            changes,
            &format!("on the source {index}"),
        )
        .await;
    }
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": title,
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
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
        router,
        base_commit,
        destination_tip,
        source_tip,
    }
}

/// The tree a row expects, as `path -> (blobId, fileMode)` — the blob id computed
/// from the content here rather than read back from the service.
fn expected_tree(
    entries: &[(&'static str, &'static str, &'static str)],
) -> BTreeMap<String, (String, String)> {
    entries
        .iter()
        .map(|(path, content, mode)| {
            (
                (*path).to_string(),
                (git_blob_id(content.as_bytes()), (*mode).to_string()),
            )
        })
        .collect()
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

/// The merge request for one row of the merge table.
fn merge_request(row: &Row, repository: &str, built: &Opened) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "pullRequestId": built.pull_request_id,
    });
    match row.pin {
        Pin::Absent => {}
        Pin::SourceTip => input["sourceCommitId"] = json!(built.source_tip),
        Pin::Base => input["sourceCommitId"] = json!(built.base_commit),
    }
    input
}

#[tokio::test]
async fn a_fast_forward_merge_moves_the_destination_reference_and_closes_the_pull_request() {
    let repository = "fast-forwarded-pulls";
    for row in rows() {
        let case = row.case;
        let built = open_pull_request(
            emulator(),
            repository,
            &format!("merge {case}"),
            &row.base,
            &row.destination,
            &row.source,
        )
        .await;
        let request = merge_request(&row, repository, &built);

        match &row.expected {
            Expected::Refused(code) => {
                let (answered, message) =
                    error_with_message(&built.router, OPERATION, &request, CLIENT_ERROR_STATUS)
                        .await;
                assert_eq!(
                    answered, *code,
                    "case {case:?}: expected the refusal {code}, got {answered:?}"
                );
                if *code == "ManualMergeRequiredException" {
                    assert_eq!(
                        message, MANUAL_MERGE_MESSAGE,
                        "case {case:?}: expected the modelled sentence, got {message:?}"
                    );
                }
                // A refused merge writes nothing: the destination branch is where
                // it was, and the pull request is still open and unmerged.
                let tip = branch_tip(&built.router, repository, DESTINATION).await;
                assert_eq!(
                    tip, built.destination_tip,
                    "case {case:?}: expected the destination branch left at {}, got {tip:?}",
                    built.destination_tip
                );
                let stored = stored_pull_request(&built.router, &built.pull_request_id).await;
                assert_eq!(
                    stored.get("pullRequestStatus").and_then(Value::as_str),
                    Some(STATUS_OPEN),
                    "case {case:?}: a refused merge leaves the pull request OPEN, got {stored}"
                );
                let target = target_of(&stored, case);
                assert_eq!(
                    target
                        .pointer("/mergeMetadata/isMerged")
                        .and_then(Value::as_bool),
                    Some(false),
                    "case {case:?}: a refused merge leaves the target unmerged, got {target}"
                );
            }
            Expected::Merged(entries) => {
                // Which commit the rule says the destination branch is left at, and
                // which commit is then the merge base: a fast-forward is made only
                // where the destination tip is the base, so the merge is the SOURCE
                // tip and the base is the destination's own.
                let (expected_commit, expected_base) = (&built.source_tip, &built.destination_tip);
                // The parents the landing commit had BEFORE the merge: a
                // fast-forward writes no commit, so they are unchanged by it.
                let parents_before =
                    parents_of(&commit_of(&built.router, repository, expected_commit).await);

                let answered = ok(&built.router, OPERATION, &request).await;
                let merged = pull_request_of(&answered, case);

                // The pull request is CLOSED and its target records the merge.
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
                    "case {case:?}: expected isMerged on the merged target, got {target}"
                );
                assert_eq!(
                    target
                        .pointer("/mergeMetadata/mergeOption")
                        .and_then(Value::as_str),
                    Some(FAST_FORWARD),
                    "case {case:?}: expected the fast-forward strategy recorded, got {target}"
                );
                assert_eq!(
                    target
                        .pointer("/mergeMetadata/mergedBy")
                        .and_then(Value::as_str),
                    Some(caller_arn(DEFAULT_ACCESS_KEY_ID).as_str()),
                    "case {case:?}: expected the calling principal as the merger, got {target}"
                );
                // No commit was written: the merge IS one of the two tips this file
                // already knows, and that commit still has the parents it had.
                assert_eq!(
                    target
                        .pointer("/mergeMetadata/mergeCommitId")
                        .and_then(Value::as_str),
                    Some(expected_commit.as_str()),
                    "case {case:?}: expected the merge to be the existing commit \
                     {expected_commit}, got {target}"
                );
                let parents_after =
                    parents_of(&commit_of(&built.router, repository, expected_commit).await);
                assert_eq!(
                    parents_after, parents_before,
                    "case {case:?}: expected the landing commit's parents left at \
                     {parents_before:?}, got {parents_after:?}"
                );
                assert_eq!(
                    target.get("sourceCommit").and_then(Value::as_str),
                    Some(built.source_tip.as_str()),
                    "case {case:?}: expected the merged source tip {}, got {target}",
                    built.source_tip
                );
                assert_eq!(
                    target.get("destinationCommit").and_then(Value::as_str),
                    Some(built.destination_tip.as_str()),
                    "case {case:?}: expected the merged destination tip {}, got {target}",
                    built.destination_tip
                );
                assert_eq!(
                    target.get("mergeBase").and_then(Value::as_str),
                    Some(expected_base.as_str()),
                    "case {case:?}: expected the merge base {expected_base}, got {target}"
                );

                // The destination branch holds that commit, and the merged content
                // is readable there.
                let tip = branch_tip(&built.router, repository, DESTINATION).await;
                assert_eq!(
                    &tip, expected_commit,
                    "case {case:?}: expected the destination branch at {expected_commit}, got \
                     {tip:?}"
                );
                let read = tree_of(&built.router, repository, DESTINATION).await;
                let expected = expected_tree(entries);
                assert_eq!(
                    read, expected,
                    "case {case:?}: expected the merged tree {expected:?}, got {read:?}"
                );
                // The source branch is left where it was: a fast-forward moves the
                // destination reference alone.
                let source_tip = branch_tip(&built.router, repository, SOURCE).await;
                assert_eq!(
                    source_tip, built.source_tip,
                    "case {case:?}: expected the source branch left at {}, got {source_tip:?}",
                    built.source_tip
                );

                // The stored pull request is what the merge answered, so a later
                // read reports the merge rather than a prediction of one.
                let stored = stored_pull_request(&built.router, &built.pull_request_id).await;
                assert_eq!(
                    stored, merged,
                    "case {case:?}: expected the stored pull request to be the answered one \
                     {merged}, got {stored}"
                );

                // A merge that happened is not attempted twice: the identical call
                // answers the merge the pull request already carries, writing
                // nothing and moving no reference.
                let again = ok(&built.router, OPERATION, &request).await;
                assert_eq!(
                    pull_request_of(&again, case),
                    merged,
                    "case {case:?}: expected a repeated merge to answer the merge already \
                     recorded {merged}, got {again}"
                );
                let tip_again = branch_tip(&built.router, repository, DESTINATION).await;
                assert_eq!(
                    &tip_again, expected_commit,
                    "case {case:?}: the repeated merge moved the destination branch to \
                     {tip_again:?}"
                );
            }
        }
    }
}

/// One row of the refusal table: what the request names, what was done to the
/// repository first, and the modelled error it is answered with.
struct Refusal {
    case: &'static str,
    /// The `repositoryName` the merge names; `None` sends the repository the pull
    /// request is in.
    repository: Option<&'static str>,
    /// The `pullRequestId` the merge names; `None` sends the created one.
    pull_request_id: Option<&'static str>,
    source_commit_id: Option<&'static str>,
    prepare: Prepare,
    code: &'static str,
}

#[derive(Debug, Clone, Copy)]
enum Prepare {
    Nothing,
    /// Delete the source branch, so its reference holds no tip to merge.
    DeleteSourceBranch,
    /// Delete the destination branch, so the merge has no reference to advance.
    DeleteDestinationBranch,
    /// Open a second repository and name THAT one in the merge, so the pull
    /// request exists but belongs elsewhere.
    AnotherRepository,
}

/// The second repository `Prepare::AnotherRepository` names.
const OTHER_REPOSITORY: &str = "another-repository";

#[tokio::test]
async fn a_fast_forward_merge_that_names_no_mergeable_pull_request_is_refused_and_writes_nothing() {
    // Rows differ along which part of the request names nothing mergeable: the id's
    // form, the id itself, the repository, the pinned source tip, and each of the
    // two references the repository may no longer hold.
    let refusals = vec![
        Refusal {
            case: "a pull request id that is not an id",
            repository: None,
            pull_request_id: Some("not-an-id"),
            source_commit_id: None,
            prepare: Prepare::Nothing,
            code: "InvalidPullRequestIdException",
        },
        Refusal {
            case: "a pull request id of nothing but spaces",
            repository: None,
            pull_request_id: Some("   "),
            source_commit_id: None,
            prepare: Prepare::Nothing,
            code: "InvalidPullRequestIdException",
        },
        Refusal {
            case: "a pull request id no pull request was minted as",
            repository: None,
            pull_request_id: Some("987654"),
            source_commit_id: None,
            prepare: Prepare::Nothing,
            code: "PullRequestDoesNotExistException",
        },
        Refusal {
            case: "a repository that does not exist",
            repository: Some("no-such-repository"),
            pull_request_id: None,
            source_commit_id: None,
            prepare: Prepare::Nothing,
            code: "RepositoryDoesNotExistException",
        },
        Refusal {
            case: "a repository the pull request is not in",
            repository: Some(OTHER_REPOSITORY),
            pull_request_id: None,
            source_commit_id: None,
            prepare: Prepare::AnotherRepository,
            code: "RepositoryNotAssociatedWithPullRequestException",
        },
        Refusal {
            case: "a source commit id that is not a commit id",
            repository: None,
            pull_request_id: None,
            source_commit_id: Some("not-a-commit-id"),
            prepare: Prepare::Nothing,
            code: "InvalidCommitIdException",
        },
        Refusal {
            case: "a source reference the repository no longer holds",
            repository: None,
            pull_request_id: None,
            source_commit_id: None,
            prepare: Prepare::DeleteSourceBranch,
            code: "ReferenceDoesNotExistException",
        },
        Refusal {
            case: "a destination reference the repository no longer holds",
            repository: None,
            pull_request_id: None,
            source_commit_id: None,
            prepare: Prepare::DeleteDestinationBranch,
            code: "ReferenceDoesNotExistException",
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        // Every row is driven against one shape of repository: a base commit and a
        // source branch one commit ahead of it, so the merge itself would be a
        // fast-forward and only what the row names refuses it. Neither branch is
        // the repository's default, so the rows that delete one are not refused for
        // deleting the DEFAULT branch.
        let router = emulator();
        let repository = "refused-fast-forwards";
        for name in [repository, OTHER_REPOSITORY] {
            ok(&router, "CreateRepository", &json!({ "repositoryName": name })).await;
        }
        let base_commit = commit(
            &router,
            repository,
            "trunk",
            None,
            &[Change::Put("a.txt", "one\n")],
            "the base",
        )
        .await;
        for branch in [DESTINATION, SOURCE] {
            ok(
                &router,
                "CreateBranch",
                &json!({
                    "repositoryName": repository,
                    "branchName": branch,
                    "commitId": base_commit,
                }),
            )
            .await;
        }
        let source_tip = commit(
            &router,
            repository,
            SOURCE,
            Some(&base_commit),
            &[Change::Put("s.txt", "source\n")],
            "on the source",
        )
        .await;
        let created = ok(
            &router,
            "CreatePullRequest",
            &json!({
                "title": case,
                "targets": [{
                    "repositoryName": repository,
                    "sourceReference": SOURCE,
                    "destinationReference": DESTINATION,
                }],
                "clientRequestToken": fresh_client_request_token(),
            }),
        )
        .await;
        let created_id = string_at(&created, "/pullRequest/pullRequestId");

        match refusal.prepare {
            Prepare::Nothing | Prepare::AnotherRepository => {}
            Prepare::DeleteSourceBranch => {
                ok(
                    &router,
                    "DeleteBranch",
                    &json!({ "repositoryName": repository, "branchName": SOURCE }),
                )
                .await;
            }
            Prepare::DeleteDestinationBranch => {
                ok(
                    &router,
                    "DeleteBranch",
                    &json!({ "repositoryName": repository, "branchName": DESTINATION }),
                )
                .await;
            }
        }

        let mut request = json!({
            "repositoryName": refusal.repository.unwrap_or(repository),
            "pullRequestId": refusal.pull_request_id.unwrap_or(&created_id),
        });
        if let Some(source_commit_id) = refusal.source_commit_id {
            request["sourceCommitId"] = json!(source_commit_id);
        }
        let answered = error(&router, OPERATION, &request, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            answered, refusal.code,
            "case {case:?}: expected the refusal {}, got {answered:?}",
            refusal.code
        );

        // Nothing was written: the pull request is still open and unmerged, and a
        // destination branch that still exists is where it was.
        let stored = stored_pull_request(&router, &created_id).await;
        assert_eq!(
            stored.get("pullRequestStatus").and_then(Value::as_str),
            Some(STATUS_OPEN),
            "case {case:?}: a refused merge leaves the pull request OPEN, got {stored}"
        );
        if !matches!(refusal.prepare, Prepare::DeleteDestinationBranch) {
            let tip = branch_tip(&router, repository, DESTINATION).await;
            assert_eq!(
                tip, base_commit,
                "case {case:?}: expected the destination branch left at {base_commit}, got {tip:?}"
            );
        }
        if !matches!(refusal.prepare, Prepare::DeleteSourceBranch) {
            let tip = branch_tip(&router, repository, SOURCE).await;
            assert_eq!(
                tip, source_tip,
                "case {case:?}: expected the source branch left at {source_tip}, got {tip:?}"
            );
        }
    }
}

/// One `Approvers` statement of an approval rule document: how many approvals it
/// needs.
struct Statement {
    approvals_needed: u32,
}

/// One row of the approval rule table: the rules the pull request holds, how many
/// reviewers approved it, and whether the fast-forward merge is allowed.
struct ApprovalRow {
    case: &'static str,
    /// One entry per approval rule applied to the pull request, each a list of the
    /// statements its document carries.
    rules: Vec<Vec<Statement>>,
    /// How many distinct reviewers have an APPROVE state recorded on it.
    approvals: usize,
    /// Does the merge go through? Derived from the rule: every statement of every
    /// rule needs that many approvals.
    merged: bool,
}

/// One approval rule document, as the compact JSON text the service stores.
fn document(statements: &[Statement]) -> String {
    let statements: Vec<Value> = statements
        .iter()
        .map(|statement| {
            json!({
                "Type": STATEMENT_TYPE,
                "NumberOfApprovalsNeeded": statement.approvals_needed,
            })
        })
        .collect();
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": statements,
    }))
    .expect("a JSON document")
}

/// Record `count` reviewers' approvals on the STORED pull request.
///
/// The approvers are identities no caller of this emulator signs as, so the state a
/// merge reads is seeded here, exactly as the sibling merge suites seed it; the
/// `pull_request_approval_state` suite drives the approvals an API caller can record
/// through `UpdatePullRequestApprovalState` instead.
fn record_approvals(engine: &Arc<engine::InMemoryEngine>, pull_request_id: &str, count: usize) {
    let state = engine.state::<CodeCommitState>();
    let mut store = state.lock();
    let id: u64 = pull_request_id.parse().expect("the id the service minted");
    let pull_request = store
        .pull_request_mut(id)
        .expect("the pull request opened above");
    for index in 0..count {
        pull_request.approvals.insert(
            format!("arn:aws:iam::{}:user/Reviewer{index}", engine::emulator_account_id()),
            APPROVE.to_string(),
        );
    }
}

#[tokio::test]
async fn a_fast_forward_merge_waits_for_the_pull_request_approval_rules_to_be_satisfied() {
    // Rows differ along how many rules the pull request holds, how many approvals
    // each statement needs, how many statements one rule carries, and how many
    // reviewers approved.
    let rows = vec![
        ApprovalRow {
            case: "no approval rule at all",
            rules: vec![],
            approvals: 0,
            merged: true,
        },
        ApprovalRow {
            case: "one rule needing one approval and nobody approved",
            rules: vec![vec![Statement {
                approvals_needed: 1,
            }]],
            approvals: 0,
            merged: false,
        },
        ApprovalRow {
            case: "one rule needing one approval and one reviewer approved",
            rules: vec![vec![Statement {
                approvals_needed: 1,
            }]],
            approvals: 1,
            merged: true,
        },
        ApprovalRow {
            case: "one rule needing two approvals and one reviewer approved",
            rules: vec![vec![Statement {
                approvals_needed: 2,
            }]],
            approvals: 1,
            merged: false,
        },
        ApprovalRow {
            case: "two statements of one rule, one of them unmet",
            rules: vec![vec![
                Statement {
                    approvals_needed: 1,
                },
                Statement {
                    approvals_needed: 3,
                },
            ]],
            approvals: 2,
            merged: false,
        },
        ApprovalRow {
            case: "two rules, both met",
            rules: vec![
                vec![Statement {
                    approvals_needed: 1,
                }],
                vec![Statement {
                    approvals_needed: 2,
                }],
            ],
            approvals: 2,
            merged: true,
        },
    ];

    let repository = "reviewed-fast-forwards";
    for row in rows {
        let case = row.case;
        let (router, engine) = emulator_with_engine();
        // A source branch one commit ahead of the destination: the merge is a
        // fast-forward, so only the approval rules decide it.
        let built = open_pull_request(
            router,
            repository,
            case,
            &[("README.md", "# base\n")],
            &[],
            &[vec![Change::Put("src.txt", "source\n")]],
        )
        .await;
        for (index, statements) in row.rules.iter().enumerate() {
            ok(
                &built.router,
                "CreatePullRequestApprovalRule",
                &json!({
                    "pullRequestId": built.pull_request_id,
                    "approvalRuleName": format!("rule-{index}"),
                    "approvalRuleContent": document(statements),
                }),
            )
            .await;
        }
        record_approvals(&engine, &built.pull_request_id, row.approvals);

        let request = json!({
            "repositoryName": repository,
            "pullRequestId": built.pull_request_id,
        });

        if row.merged {
            let answered = ok(&built.router, OPERATION, &request).await;
            let merged = pull_request_of(&answered, case);
            assert_eq!(
                merged.get("pullRequestStatus").and_then(Value::as_str),
                Some(STATUS_CLOSED),
                "case {case:?}: expected the satisfied pull request merged and CLOSED, got {merged}"
            );
            let tip = branch_tip(&built.router, repository, DESTINATION).await;
            assert_eq!(
                tip, built.source_tip,
                "case {case:?}: expected the destination branch advanced to {}, got {tip:?}",
                built.source_tip
            );
        } else {
            let (code, message) =
                error_with_message(&built.router, OPERATION, &request, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, "PullRequestApprovalRulesNotSatisfiedException",
                "case {case:?}: expected the unmet approval rules refused, got {code:?}"
            );
            assert_eq!(
                message, UNSATISFIED_MESSAGE,
                "case {case:?}: expected the modelled sentence, got {message:?}"
            );
            // Nothing was written: the pull request is still open, and the
            // destination branch is where it was.
            let stored = stored_pull_request(&built.router, &built.pull_request_id).await;
            assert_eq!(
                stored.get("pullRequestStatus").and_then(Value::as_str),
                Some(STATUS_OPEN),
                "case {case:?}: a refused merge leaves the pull request OPEN, got {stored}"
            );
            let tip = branch_tip(&built.router, repository, DESTINATION).await;
            assert_eq!(
                tip, built.destination_tip,
                "case {case:?}: expected the destination branch left at {}, got {tip:?}",
                built.destination_tip
            );
        }
    }
}
