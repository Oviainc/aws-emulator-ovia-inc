// SPDX-License-Identifier: CC0-1.0

//! `MergePullRequestBySquash` over the wire, driven as awsJson1_1 requests through
//! the router that serves the real emulator.
//!
//! Two rules are under test.
//!
//! The first is the merge itself: squashing a pull request merges the CURRENT tip of
//! its source reference into the current tip of its destination reference against
//! the tree of those two tips' merge base — so a path only one side changed takes
//! that side's version, a path both sides changed the same way takes the agreed
//! version, and a path they changed differently is settled by `conflictDetailLevel`,
//! `conflictResolutionStrategy` and `conflictResolution` or else refused — and
//! writes the result as ONE commit whose ONLY parent is the destination tip, so the
//! source's history is collapsed rather than joined. The destination branch is left
//! at that commit and the pull request is closed, its target then recording what was
//! merged, who merged it, the merge commit and the squash strategy.
//!
//! The second is the condition a merge is allowed under: a pull request whose
//! approval rules state conditions its approvals do not meet cannot be merged at
//! all. A rule's document states, per `Approvers` statement, how many approvals it
//! needs and optionally the pool of identities whose approvals count towards it, so
//! the rule is met once every statement has that many approvals from its pool; a
//! pull request that meets every rule — or whose rules have been overridden — is
//! merged, and one that does not is refused with HTTP 400
//! `PullRequestApprovalRulesNotSatisfiedException` and nothing is written.
//!
//! The tables below span what the rules generalise over rather than one recorded
//! request: which side changed a path and in what way (content, mode, deletion, a
//! new file, a nested path), whether a side moved at all, the conflict detail level,
//! the conflict resolution strategies the model declares, a `conflictResolution`
//! entry, `keepEmptyFolders`, a pinned `sourceCommitId` and none; and, for the
//! approval rule, how many rules the pull request holds, how many approvals each
//! statement needs, how many statements one rule carries, whether a statement names
//! an approval pool and in which form (an exact identity, a wildcard, an identity
//! nobody holds), how many approvals the pull request holds and in which state,
//! whether the rules are overridden, and which merge operation is called.
//!
//! Each merged row's expectation is the tree this file computes from the rule, read
//! back through `GetFolder` as `(blobId, fileMode)` per path — content-addressed, so
//! the answer is checked against the content the rule says the merge produces — and
//! the pull request is read back through `GetPullRequest`, so the stored record
//! rather than the merge's own answer is what the assertions hold.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{
    base64, emulator, emulator_with_engine, error, git_blob_id, is_object_id, ok, string_at,
};
use crate::unreferenced_merge_commits::{
    commit, commit_of, parents_of, tree_of, Change, EXECUTABLE, GITKEEP, NORMAL,
};
use crate::wire::{caller_arn, DEFAULT_ACCESS_KEY_ID};

/// The two branches every pull request below is between: `main` is the repository's
/// default branch (it took the first commit) and the destination, `feature` is the
/// source.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// `MergeOptionTypeEnum`'s values for the two strategies driven here, and the
/// `PullRequestStatusEnum` values a pull request holds before and after a merge
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const SQUASH: &str = "SQUASH_MERGE";
const THREE_WAY: &str = "THREE_WAY_MERGE";
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The two merge operations the approval rule table drives.
const SQUASH_OPERATION: &str = "MergePullRequestBySquash";
const THREE_WAY_OPERATION: &str = "MergePullRequestByThreeWay";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The one sentence pair `PullRequestApprovalRulesNotSatisfiedException` carries,
/// whichever rule went unmet: the refusal, then where the rules that still have
/// requirements are reported. The wording CodeCommit answers that code with (the
/// model's documentation for the shape states the same refusal in other words).
const UNSATISFIED_MESSAGE: &str =
    "You cannot merge the pull request because the requirements for one or more approval rules \
     have not been met. For details about the approval rules that still have requirements, use \
     EvaluatePullRequestApprovalRules.";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// `ApprovalState`'s values, one reviewer's recorded state
/// (`smithy_query(query="shape",shape="ApprovalState")`).
const APPROVE: &str = "APPROVE";
const REVOKE: &str = "REVOKE";

/// A five-line file, the base of the line-level rows below.
const FIVE_LINES: &str = "one\ntwo\nthree\nfour\nfive\n";

/// What the merge must answer for one row of the merge table.
#[derive(Debug, Clone)]
enum Expected {
    /// The merged tree: every path the result holds, with its content and mode.
    Tree(Vec<(&'static str, &'static str, &'static str)>),
    /// The modelled error code the request is refused with.
    Refused(&'static str),
}

/// One row of the merge table: a base commit, the change each side makes to it, the
/// merge request's conflict members, whether the request pins the source tip, and
/// what the merge must produce.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    base: Vec<(&'static str, &'static str)>,
    destination: Vec<Change>,
    source: Vec<Change>,
    /// `conflictDetailLevel` as sent; `None` leaves the member out, which is the
    /// documented `FILE_LEVEL` default.
    detail_level: Option<&'static str>,
    /// `conflictResolutionStrategy` as sent; `None` leaves it out, which is the
    /// documented `NONE` default.
    strategy: Option<&'static str>,
    /// `conflictResolution` as sent.
    resolution: Option<Value>,
    keep_empty_folders: bool,
    /// Does the request pin `sourceCommitId` to the source branch's current tip?
    pin_source_tip: bool,
    expected: Expected,
}

impl Row {
    fn new(
        case: &'static str,
        base: Vec<(&'static str, &'static str)>,
        destination: Vec<Change>,
        source: Vec<Change>,
        expected: Expected,
    ) -> Self {
        Self {
            case,
            base,
            destination,
            source,
            detail_level: None,
            strategy: None,
            resolution: None,
            keep_empty_folders: false,
            pin_source_tip: false,
            expected,
        }
    }

    fn with_detail_level(mut self, level: &'static str) -> Self {
        self.detail_level = Some(level);
        self
    }

    fn with_strategy(mut self, strategy: &'static str) -> Self {
        self.strategy = Some(strategy);
        self
    }

    fn with_resolution(mut self, resolution: Value) -> Self {
        self.resolution = Some(resolution);
        self
    }

    fn keeping_empty_folders(mut self) -> Self {
        self.keep_empty_folders = true;
        self
    }

    fn pinning_the_source_tip(mut self) -> Self {
        self.pin_source_tip = true;
        self
    }
}

/// The merge table: rows differ along which side changed what, how the request
/// settles a path both sides changed, whether folders emptied by the merge are kept,
/// whether the source tip is pinned, and whether a side moved at all.
fn rows() -> Vec<Row> {
    vec![
        // Each side adds a file the other does not touch: both land.
        Row::new(
            "disjoint-additions",
            vec![("README.md", "# base\n")],
            vec![Change::Put("dest/only.txt", "destination\n")],
            vec![Change::Put("src/only.txt", "source\n")],
            Expected::Tree(vec![
                ("README.md", "# base\n", NORMAL),
                ("dest/only.txt", "destination\n", NORMAL),
                ("src/only.txt", "source\n", NORMAL),
            ]),
        ),
        // The same merge with the source tip pinned: `sourceCommitId` names the tip
        // the branch holds, so the merge proceeds exactly as it does without it.
        Row::new(
            "disjoint-additions-with-the-source-tip-pinned",
            vec![("README.md", "# base\n")],
            vec![Change::Put("dest/only.txt", "destination\n")],
            vec![Change::Put("src/only.txt", "source\n")],
            Expected::Tree(vec![
                ("README.md", "# base\n", NORMAL),
                ("dest/only.txt", "destination\n", NORMAL),
                ("src/only.txt", "source\n", NORMAL),
            ]),
        )
        .pinning_the_source_tip(),
        // Only the source moved: the destination tip IS the merge base, and the
        // squash still writes a commit whose only parent is that tip.
        Row::new(
            "only-the-source-moved",
            vec![("a.txt", "one\n")],
            vec![],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "source\n", NORMAL)]),
        ),
        // Only the destination moved: the source tip is the base, so the result is
        // the destination's own tree.
        Row::new(
            "only-the-destination-moved",
            vec![("a.txt", "one\n"), ("keep.txt", "keep\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![],
            Expected::Tree(vec![
                ("a.txt", "destination\n", NORMAL),
                ("keep.txt", "keep\n", NORMAL),
            ]),
        ),
        // A nested path changed on one side only.
        Row::new(
            "one-sided-nested-change",
            vec![("deep/a/b/c.txt", "one\n")],
            vec![],
            vec![Change::Put("deep/a/b/c.txt", "source\n")],
            Expected::Tree(vec![("deep/a/b/c.txt", "source\n", NORMAL)]),
        ),
        // Both sides made the SAME change: an agreement, not a conflict.
        Row::new(
            "both-sides-agree",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "two\n")],
            vec![Change::Put("a.txt", "two\n")],
            Expected::Tree(vec![("a.txt", "two\n", NORMAL)]),
        ),
        // One side deleted a path the other left alone: the deletion is the change,
        // so it lands.
        Row::new(
            "one-sided-deletion",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![Change::Delete("b/c.txt")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "source\n", NORMAL)]),
        ),
        // The same deletion with `keepEmptyFolders`: "a .gitkeep file is created for
        // empty folders".
        Row::new(
            "one-sided-deletion-keeping-folders",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![Change::Delete("b/c.txt")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![
                ("a.txt", "source\n", NORMAL),
                ("b/.gitkeep", "", NORMAL),
            ]),
        )
        .keeping_empty_folders(),
        // A mode-only change on one side: the mode is part of the entry, so it
        // merges like content does.
        Row::new(
            "one-sided-mode-change",
            vec![("run.sh", "#!/bin/sh\n")],
            vec![Change::PutMode("run.sh", "#!/bin/sh\n", EXECUTABLE)],
            vec![Change::Put("other.txt", "source\n")],
            Expected::Tree(vec![
                ("other.txt", "source\n", NORMAL),
                ("run.sh", "#!/bin/sh\n", EXECUTABLE),
            ]),
        ),
        // Both sides changed one file differently at the default FILE_LEVEL detail:
        // "a not-mergeable result if the same file has differences in both
        // branches".
        Row::new(
            "file-level-conflict",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        ),
        // ACCEPT_SOURCE on that conflict: "which branch to use when resolving
        // conflicts".
        Row::new(
            "accept-source",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "source\n", NORMAL)]),
        )
        .with_strategy("ACCEPT_SOURCE"),
        // ACCEPT_DESTINATION on the same conflict.
        Row::new(
            "accept-destination",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "destination\n", NORMAL)]),
        )
        .with_strategy("ACCEPT_DESTINATION"),
        // LINE_LEVEL detail with the two sides changing DIFFERENT lines: not a
        // conflict, and the merged file carries both changes.
        Row::new(
            "line-level-disjoint-lines",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "ONE\ntwo\nthree\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nthree\nfour\nFIVE\n")],
            Expected::Tree(vec![("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n", NORMAL)]),
        )
        .with_detail_level("LINE_LEVEL"),
        // LINE_LEVEL detail with both sides changing the SAME line: "a conflict is
        // considered not mergeable if the same file in both branches has differences
        // on the same line".
        Row::new(
            "line-level-same-line",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "one\ntwo\nTHREE-DEST\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nTHREE-SRC\nfour\nfive\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_detail_level("LINE_LEVEL"),
        // AUTOMERGE with USE_NEW_CONTENT: the caller's own bytes settle the path.
        Row::new(
            "automerge-use-new-content",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "resolved by hand\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{
                "filePath": "a.txt",
                "replacementType": "USE_NEW_CONTENT",
                "content": base64(b"resolved by hand\n"),
            }],
        })),
        // AUTOMERGE with KEEP_BASE: the version the merge base held.
        Row::new(
            "automerge-keep-base",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Tree(vec![("a.txt", "one\n", NORMAL)]),
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_BASE" }],
        })),
        // A conflict resolution list under any other strategy names a branch and a
        // resolution at once, which the merge cannot honour.
        Row::new(
            "resolution-without-automerge",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("InvalidConflictResolutionException"),
        )
        .with_strategy("ACCEPT_SOURCE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_SOURCE" }],
        })),
    ]
}

/// A repository with `main` at the base commit plus the destination's changes and
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
    destination: &[Change],
    source: &[Change],
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
    let destination_tip = if destination.is_empty() {
        base_commit.clone()
    } else {
        commit(
            &router,
            repository,
            DESTINATION,
            Some(&base_commit),
            destination,
            "on the destination",
        )
        .await
    };
    let source_tip = if source.is_empty() {
        base_commit.clone()
    } else {
        commit(
            &router,
            repository,
            SOURCE,
            Some(&base_commit),
            source,
            "on the source",
        )
        .await
    };
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

/// The merge request for one row of the merge table.
fn merge_request(row: &Row, repository: &str, built: &Opened) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "pullRequestId": built.pull_request_id,
        "commitMessage": format!("merge for {}", row.case),
        "authorName": "Merge Tester",
        "email": "merge.tester@example.com",
    });
    if let Some(level) = row.detail_level {
        input["conflictDetailLevel"] = json!(level);
    }
    if let Some(strategy) = row.strategy {
        input["conflictResolutionStrategy"] = json!(strategy);
    }
    if let Some(resolution) = &row.resolution {
        input["conflictResolution"] = resolution.clone();
    }
    if row.keep_empty_folders {
        input["keepEmptyFolders"] = json!(true);
    }
    if row.pin_source_tip {
        input["sourceCommitId"] = json!(built.source_tip);
    }
    input
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

#[tokio::test]
async fn squashing_a_pull_request_writes_one_commit_on_the_destination_and_closes_the_request() {
    let repository = "squashed-pulls";
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
                let answered = error(
                    &built.router,
                    SQUASH_OPERATION,
                    &request,
                    CLIENT_ERROR_STATUS,
                )
                .await;
                assert_eq!(
                    answered, *code,
                    "case {case:?}: expected the refusal {code}, got {answered:?}"
                );
                // A refused merge writes nothing: the destination branch is where it
                // was, and the pull request is still open and unmerged.
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
                continue;
            }
            Expected::Tree(entries) => {
                let answered = ok(&built.router, SQUASH_OPERATION, &request).await;
                let merged = pull_request_of(&answered, case);

                // The pull request is CLOSED, and its target records the merge.
                assert_eq!(
                    merged.get("pullRequestStatus").and_then(Value::as_str),
                    Some(STATUS_CLOSED),
                    "case {case:?}: a merged pull request is CLOSED, got {merged}"
                );
                let target = target_of(&merged, case);
                let merge_commit = target
                    .pointer("/mergeMetadata/mergeCommitId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no mergeCommitId in {target}"))
                    .to_string();
                assert!(
                    is_object_id(&merge_commit),
                    "case {case:?}: expected a git object name as the merge commit, got \
                     {merge_commit:?}"
                );
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
                    Some(SQUASH),
                    "case {case:?}: expected the squash strategy recorded, got {target}"
                );
                assert_eq!(
                    target
                        .pointer("/mergeMetadata/mergedBy")
                        .and_then(Value::as_str),
                    Some(caller_arn(DEFAULT_ACCESS_KEY_ID).as_str()),
                    "case {case:?}: expected the calling principal as the merger, got {target}"
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
                    Some(built.base_commit.as_str()),
                    "case {case:?}: expected the merge base {}, got {target}",
                    built.base_commit
                );

                // The squash commit records the DESTINATION tip alone — the source's
                // history is collapsed, not joined — and the destination branch is
                // left at it.
                let written = commit_of(&built.router, repository, &merge_commit).await;
                let parents = parents_of(&written);
                assert_eq!(
                    parents,
                    vec![built.destination_tip.clone()],
                    "case {case:?}: expected the destination tip as the only parent, got \
                     {parents:?}"
                );
                let tip = branch_tip(&built.router, repository, DESTINATION).await;
                assert_eq!(
                    tip, merge_commit,
                    "case {case:?}: expected the destination branch at the merge commit \
                     {merge_commit}, got {tip:?}"
                );

                // The merged tree is the one the rule says, read back by path.
                let read = tree_of(&built.router, repository, DESTINATION).await;
                let expected = expected_tree(entries);
                assert_eq!(
                    read, expected,
                    "case {case:?}: expected the merged tree {expected:?}, got {read:?}"
                );
                for (path, (blob, _)) in &read {
                    if path.ends_with(GITKEEP) {
                        assert_eq!(
                            blob,
                            &git_blob_id(b""),
                            "case {case:?}: the kept-folder placeholder is the empty blob, got \
                             {blob:?}"
                        );
                    }
                }

                // The stored pull request is what the merge answered, so a later read
                // reports the merge rather than a prediction of one.
                let stored = stored_pull_request(&built.router, &built.pull_request_id).await;
                assert_eq!(
                    stored, merged,
                    "case {case:?}: expected the stored pull request to be the answered one \
                     {merged}, got {stored}"
                );

                // A merge that happened is not attempted twice: the identical call
                // answers the merge the pull request already carries, writing nothing
                // and moving no reference.
                let again = ok(&built.router, SQUASH_OPERATION, &request).await;
                assert_eq!(
                    pull_request_of(&again, case),
                    merged,
                    "case {case:?}: expected a repeated merge to answer the merge already \
                     recorded {merged}, got {again}"
                );
                let tip_again = branch_tip(&built.router, repository, DESTINATION).await;
                assert_eq!(
                    tip_again, merge_commit,
                    "case {case:?}: the repeated merge moved the destination branch to \
                     {tip_again:?}"
                );
            }
        }
    }
}

/// One row of the refusal table: what the request names, and the modelled error it
/// is answered with.
struct Refusal {
    case: &'static str,
    /// The `repositoryName` the merge names; `None` sends the repository the pull
    /// request is in.
    repository: Option<&'static str>,
    /// The `pullRequestId` the merge names; `None` sends the created one.
    pull_request_id: Option<&'static str>,
    /// The `sourceCommitId` the merge names, if any.
    source_commit_id: SourceCommitId,
    /// What to do to the repository before the merge, if anything.
    prepare: Prepare,
    code: &'static str,
}

#[derive(Debug, Clone, Copy)]
enum Prepare {
    Nothing,
    /// Delete the source branch, so its reference holds no tip to merge.
    DeleteSourceBranch,
    /// Delete the destination branch, so the merge has no reference to be applied
    /// to.
    DeleteDestinationBranch,
}

#[derive(Debug, Clone, Copy)]
enum SourceCommitId {
    Absent,
    /// A value that is not a git object name at all.
    Malformed,
    /// The base commit: a real commit, but not the source branch's tip.
    Base,
}

#[tokio::test]
async fn a_squash_merge_that_names_no_mergeable_pull_request_is_refused_and_writes_nothing() {
    // Rows differ along which part of the request names nothing mergeable: the id's
    // form, the id itself, the repository, the pinned source tip, and a reference
    // the repository no longer holds.
    let refusals = vec![
        Refusal {
            case: "a pull request id that is not an id",
            repository: None,
            pull_request_id: Some("not-an-id"),
            source_commit_id: SourceCommitId::Absent,
            prepare: Prepare::Nothing,
            code: "InvalidPullRequestIdException",
        },
        Refusal {
            case: "a pull request id of nothing but spaces",
            repository: None,
            pull_request_id: Some("   "),
            source_commit_id: SourceCommitId::Absent,
            prepare: Prepare::Nothing,
            code: "InvalidPullRequestIdException",
        },
        Refusal {
            case: "a pull request id no pull request was minted as",
            repository: None,
            pull_request_id: Some("987654"),
            source_commit_id: SourceCommitId::Absent,
            prepare: Prepare::Nothing,
            code: "PullRequestDoesNotExistException",
        },
        Refusal {
            case: "a repository that does not exist",
            repository: Some("no-such-repository"),
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            prepare: Prepare::Nothing,
            code: "RepositoryDoesNotExistException",
        },
        Refusal {
            case: "a source commit id that is not a commit id",
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Malformed,
            prepare: Prepare::Nothing,
            code: "InvalidCommitIdException",
        },
        Refusal {
            case: "a source commit id that is not the tip of the source branch",
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Base,
            prepare: Prepare::Nothing,
            code: "TipOfSourceReferenceIsDifferentException",
        },
        Refusal {
            case: "a source reference the repository no longer holds",
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            prepare: Prepare::DeleteSourceBranch,
            code: "TipOfSourceReferenceIsDifferentException",
        },
        Refusal {
            case: "a destination reference the repository no longer holds",
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            prepare: Prepare::DeleteDestinationBranch,
            code: "ConcurrentReferenceUpdateException",
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        // Every row is driven against one shape of repository: a base commit, a
        // destination branch that moved, and a source branch that moved. The
        // destination is not the repository's default branch, so the row that
        // deletes it is not refused for deleting the DEFAULT branch.
        let router = emulator();
        let repository = "refused-squashes";
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        )
        .await;
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
        let destination_tip = commit(
            &router,
            repository,
            DESTINATION,
            Some(&base_commit),
            &[Change::Put("d.txt", "destination\n")],
            "on the destination",
        )
        .await;
        commit(
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
            }),
        )
        .await;
        let created_id = string_at(&created, "/pullRequest/pullRequestId");

        match refusal.prepare {
            Prepare::Nothing => {}
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
        match refusal.source_commit_id {
            SourceCommitId::Absent => {}
            SourceCommitId::Malformed => {
                request["sourceCommitId"] = json!("not-a-commit-id");
            }
            SourceCommitId::Base => {
                request["sourceCommitId"] = json!(base_commit);
            }
        }
        let answered = error(&router, SQUASH_OPERATION, &request, CLIENT_ERROR_STATUS).await;
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
                tip, destination_tip,
                "case {case:?}: expected the destination branch left at {destination_tip}, got \
                 {tip:?}"
            );
        }
    }
}

/// One `Approvers` statement of an approval rule document: how many approvals it
/// needs and, where it names one, the pool of identities whose approvals count.
struct Statement {
    approvals_needed: u32,
    pool: Vec<String>,
}

/// One row of the approval rule table: the rules the pull request holds, the
/// approval state each reviewer has recorded on it, whether its rules are
/// overridden, which merge operation is called, and whether the merge is allowed.
struct ApprovalRow {
    case: &'static str,
    /// One entry per approval rule applied to the pull request, each a list of the
    /// statements its document carries.
    rules: Vec<Vec<Statement>>,
    /// `(reviewer identity, approval state)` pairs the stored pull request holds.
    approvals: Vec<(String, &'static str)>,
    overridden: bool,
    operation: &'static str,
    /// Does the merge go through? Derived from the rule: every statement of every
    /// rule needs that many approvals from its pool, unless the rules are
    /// overridden.
    merged: bool,
}

/// An identity ARN in the emulator's account: an IAM user of that name.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// An identity ARN for one session of an assumed role.
fn assumed_role_arn(role: &str, session: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role}/{session}",
        engine::emulator_account_id()
    )
}

/// One approval rule document, as the compact JSON text the service stores.
fn document(statements: &[Statement]) -> String {
    let statements: Vec<Value> = statements
        .iter()
        .map(|statement| {
            let mut value = json!({
                "Type": STATEMENT_TYPE,
                "NumberOfApprovalsNeeded": statement.approvals_needed,
            });
            if !statement.pool.is_empty() {
                value["ApprovalPoolMembers"] = json!(statement.pool);
            }
            value
        })
        .collect();
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": statements,
    }))
    .expect("a JSON document")
}

fn statement(approvals_needed: u32, pool: &[String]) -> Statement {
    Statement {
        approvals_needed,
        pool: pool.to_vec(),
    }
}

/// Record the approval states and the override this row names on the STORED pull
/// request. A row's approvers include identities no caller of this emulator can sign
/// as (an assumed-role session, a user nobody holds), so the state a merge reads is
/// seeded here, exactly as the sibling approval-rule suite seeds a closed pull
/// request. The sibling `pull_request_approval_state` suite drives the approvals an
/// API caller can record through `UpdatePullRequestApprovalState`, and
/// `pull_request_approval_rule_override` the override it can record through
/// `OverridePullRequestApprovalRules`, instead.
fn record_approvals(engine: &Arc<engine::InMemoryEngine>, pull_request_id: &str, row: &ApprovalRow) {
    let state = engine.state::<CodeCommitState>();
    let mut store = state.lock();
    let id: u64 = pull_request_id.parse().expect("the id the service minted");
    let pull_request = store
        .pull_request_mut(id)
        .expect("the pull request opened above");
    for (reviewer, approval_state) in &row.approvals {
        pull_request
            .approvals
            .insert(reviewer.clone(), (*approval_state).to_string());
    }
    pull_request.approval_rules_overridden_by = row
        .overridden
        .then(|| user_arn("Overrider"));
}

#[tokio::test]
async fn a_pull_request_is_merged_only_once_its_approval_rules_are_satisfied() {
    let reviewer = user_arn("Reviewer");
    let other_reviewer = user_arn("OtherReviewer");
    let session = assumed_role_arn("Reviewers", "session-7");
    let pool_of_reviewer = vec![reviewer.clone()];
    let pool_of_nobody = vec![user_arn("NobodyHoldsThis")];
    let pool_of_sessions = vec![assumed_role_arn("Reviewers", "*")];
    let no_pool: Vec<String> = Vec::new();

    let rows = vec![
        ApprovalRow {
            case: "no approval rule at all",
            rules: vec![],
            approvals: vec![],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        ApprovalRow {
            case: "one rule needing one approval and nobody approved",
            rules: vec![vec![statement(1, &no_pool)]],
            approvals: vec![],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "one rule needing one approval and one reviewer approved",
            rules: vec![vec![statement(1, &no_pool)]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        ApprovalRow {
            case: "one rule needing two approvals and one reviewer approved",
            rules: vec![vec![statement(2, &no_pool)]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "one rule needing two approvals and two reviewers approved",
            rules: vec![vec![statement(2, &no_pool)]],
            approvals: vec![
                (reviewer.clone(), APPROVE),
                (other_reviewer.clone(), APPROVE),
            ],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        ApprovalRow {
            case: "an approval the reviewer took back",
            rules: vec![vec![statement(1, &no_pool)]],
            approvals: vec![(reviewer.clone(), REVOKE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "a pool that names the approving reviewer",
            rules: vec![vec![statement(1, &pool_of_reviewer)]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        ApprovalRow {
            case: "a pool the approving reviewer is not in",
            rules: vec![vec![statement(1, &pool_of_nobody)]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "a wildcard pool over every session of an assumed role",
            rules: vec![vec![statement(1, &pool_of_sessions)]],
            approvals: vec![(session.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        ApprovalRow {
            case: "a wildcard pool and an approver outside the role",
            rules: vec![vec![statement(1, &pool_of_sessions)]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "two statements of one rule, only one of them met",
            rules: vec![vec![
                statement(1, &pool_of_reviewer),
                statement(1, &pool_of_nobody),
            ]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "two rules, one met and one not",
            rules: vec![
                vec![statement(1, &no_pool)],
                vec![statement(1, &pool_of_nobody)],
            ],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "two rules, both met",
            rules: vec![
                vec![statement(1, &no_pool)],
                vec![statement(1, &pool_of_reviewer)],
            ],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        ApprovalRow {
            case: "an unmet rule whose approval rules are overridden",
            rules: vec![vec![statement(3, &no_pool)]],
            approvals: vec![],
            overridden: true,
            operation: SQUASH_OPERATION,
            merged: true,
        },
        // The same condition holds for the other merge strategy the model declares
        // for a pull request, so it is driven here too.
        ApprovalRow {
            case: "an unmet rule merged three-way",
            rules: vec![vec![statement(1, &no_pool)]],
            approvals: vec![],
            overridden: false,
            operation: THREE_WAY_OPERATION,
            merged: false,
        },
        ApprovalRow {
            case: "a met rule merged three-way",
            rules: vec![vec![statement(1, &pool_of_reviewer)]],
            approvals: vec![(reviewer.clone(), APPROVE)],
            overridden: false,
            operation: THREE_WAY_OPERATION,
            merged: true,
        },
    ];

    let repository = "reviewed-pulls";
    for row in rows {
        let case = row.case;
        let (router, engine) = emulator_with_engine();
        let built = open_pull_request(
            router,
            repository,
            case,
            &[("README.md", "# base\n")],
            &[Change::Put("dest.txt", "destination\n")],
            &[Change::Put("src.txt", "source\n")],
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
        record_approvals(&engine, &built.pull_request_id, &row);

        let request = json!({
            "repositoryName": repository,
            "pullRequestId": built.pull_request_id,
            "commitMessage": format!("merge for {case}"),
            "authorName": "Merge Tester",
            "email": "merge.tester@example.com",
        });

        if row.merged {
            let answered = ok(&built.router, row.operation, &request).await;
            let merged = pull_request_of(&answered, case);
            assert_eq!(
                merged.get("pullRequestStatus").and_then(Value::as_str),
                Some(STATUS_CLOSED),
                "case {case:?}: expected the satisfied pull request merged and CLOSED, got \
                 {merged}"
            );
            let target = target_of(&merged, case);
            let expected_option = if row.operation == SQUASH_OPERATION {
                SQUASH
            } else {
                THREE_WAY
            };
            assert_eq!(
                target
                    .pointer("/mergeMetadata/mergeOption")
                    .and_then(Value::as_str),
                Some(expected_option),
                "case {case:?}: expected {expected_option} recorded on the target, got {target}"
            );
            // The merge is readable where it was written: the destination branch is
            // at the merge commit the target names.
            let merge_commit = target
                .pointer("/mergeMetadata/mergeCommitId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no mergeCommitId in {target}"))
                .to_string();
            let tip = branch_tip(&built.router, repository, DESTINATION).await;
            assert_eq!(
                tip, merge_commit,
                "case {case:?}: expected the destination branch at {merge_commit}, got {tip:?}"
            );
        } else {
            let (code, message) = error_with_message(
                &built.router,
                row.operation,
                &request,
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "PullRequestApprovalRulesNotSatisfiedException",
                "case {case:?}: expected the unmet approval rules refused, got {code:?}"
            );
            assert_eq!(
                message, UNSATISFIED_MESSAGE,
                "case {case:?}: expected the modelled sentence, got {message:?}"
            );
            // Nothing was written: the pull request is still open and unmerged, and
            // the destination branch is where it was.
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
            let tip = branch_tip(&built.router, repository, DESTINATION).await;
            assert_eq!(
                tip, built.destination_tip,
                "case {case:?}: expected the destination branch left at {}, got {tip:?}",
                built.destination_tip
            );
        }
    }
}
