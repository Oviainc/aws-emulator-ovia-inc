// SPDX-License-Identifier: CC0-1.0

//! `MergePullRequestByThreeWay` over the wire, driven as awsJson1_1 requests through
//! the router that serves the real emulator.
//!
//! The rule under test: merging a pull request three-way merges the CURRENT tip of
//! its source reference into the current tip of its destination reference against the
//! tree of those two tips' merge base — so a path only one side changed takes that
//! side's version, a path both sides changed the same way takes the agreed version,
//! and a path they changed differently is settled by `conflictDetailLevel`,
//! `conflictResolutionStrategy` and `conflictResolution` or else refused — writes the
//! result as ONE commit whose parents are both tips, leaves the destination branch at
//! that commit, and closes the pull request, whose target then records what was
//! merged, who merged it, the merge commit and the strategy.
//!
//! The tables below span what the rule generalises over rather than one recorded
//! request: which side changed a path and in what way (content, mode, deletion, a new
//! file, a nested path), whether a side moved at all, the conflict detail level, the
//! conflict resolution strategies the model declares, a `conflictResolution` entry,
//! `keepEmptyFolders`, a pinned `sourceCommitId` and none, and each part of the
//! request that can name no mergeable pull request. Each row's expectation is the
//! merged tree this file computes from the rule, read back through `GetFolder` as
//! `(blobId, fileMode)` per path — content-addressed, so the answer is checked
//! against the content the rule says the merge produces — and the pull request is
//! read back through `GetPullRequest`, so the stored record rather than the merge's
//! own answer is what the assertions hold.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{
    base64, emulator, error, git_blob_id, is_object_id, ok, string_at,
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

/// `MergeOptionTypeEnum`'s value for this operation's strategy, and
/// `PullRequestStatusEnum`'s value a merged pull request holds
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const THREE_WAY: &str = "THREE_WAY_MERGE";
const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// A five-line file, the base of the line-level rows below.
const FIVE_LINES: &str = "one\ntwo\nthree\nfour\nfive\n";

/// What the merge must answer for one row.
#[derive(Debug, Clone)]
enum Expected {
    /// The merged tree: every path the result holds, with its content and mode.
    Tree(Vec<(&'static str, &'static str, &'static str)>),
    /// The modelled error code the request is refused with.
    Refused(&'static str),
}

/// One row: a base commit, the change each side makes to it, the merge request's
/// conflict members, whether the request pins the source tip, and what the merge must
/// produce.
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

/// The table: rows differ along which side changed what, how the request settles a
/// path both sides changed, whether folders emptied by the merge are kept, whether
/// the source tip is pinned, and whether a side moved at all.
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
        // Only the source moved: the destination tip IS the merge base, and a
        // three-way merge still records both tips as the merge commit's parents.
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
        // NONE is the default, named explicitly: conflicts are still refused.
        Row::new(
            "strategy-none-named",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_strategy("NONE"),
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
        // One path, held as a FILE by one side and as a FOLDER by the other: a git
        // tree holds one or the other under a name, so no strategy settles it.
        Row::new(
            "file-against-folder",
            vec![("z.txt", "z\n")],
            vec![Change::Put("x", "a file\n")],
            vec![Change::Put("x/y.txt", "a folder\n")],
            Expected::Refused("ManualMergeRequiredException"),
        )
        .with_strategy("AUTOMERGE"),
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

async fn opened(repository: &str, row: &Row) -> Opened {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base_changes: Vec<Change> = row
        .base
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
    let destination_tip = if row.destination.is_empty() {
        base_commit.clone()
    } else {
        commit(
            &router,
            repository,
            DESTINATION,
            Some(&base_commit),
            &row.destination,
            "on the destination",
        )
        .await
    };
    let source_tip = if row.source.is_empty() {
        base_commit.clone()
    } else {
        commit(
            &router,
            repository,
            SOURCE,
            Some(&base_commit),
            &row.source,
            "on the source",
        )
        .await
    };
    let created = ok(
        &router,
        "CreatePullRequest",
        &json!({
            "title": format!("merge {}", row.case),
            "targets": [{
                "repositoryName": repository,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
        }),
    )
    .await;
    Opened {
        router,
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
        base_commit,
        destination_tip,
        source_tip,
    }
}

/// The merge request for one row.
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
async fn merging_a_pull_request_three_way_writes_the_merge_and_closes_the_request() {
    let repository = "merged-pulls";
    for row in rows() {
        let case = row.case;
        let built = opened(repository, &row).await;
        let request = merge_request(&row, repository, &built);

        match &row.expected {
            Expected::Refused(code) => {
                let answered = error(
                    &built.router,
                    "MergePullRequestByThreeWay",
                    &request,
                    400,
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
                let stored =
                    stored_pull_request(&built.router, &built.pull_request_id).await;
                assert_eq!(
                    stored.get("pullRequestStatus").and_then(Value::as_str),
                    Some(STATUS_OPEN),
                    "case {case:?}: a refused merge leaves the pull request OPEN, got {stored}"
                );
                let target = target_of(&stored, case);
                assert_eq!(
                    target.pointer("/mergeMetadata/isMerged").and_then(Value::as_bool),
                    Some(false),
                    "case {case:?}: a refused merge leaves the target unmerged, got {target}"
                );
                continue;
            }
            Expected::Tree(entries) => {
                let answered = ok(&built.router, "MergePullRequestByThreeWay", &request).await;
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
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no mergeCommitId in {target}")
                    })
                    .to_string();
                assert!(
                    is_object_id(&merge_commit),
                    "case {case:?}: expected a git object name as the merge commit, got \
                     {merge_commit:?}"
                );
                assert_eq!(
                    target.pointer("/mergeMetadata/isMerged").and_then(Value::as_bool),
                    Some(true),
                    "case {case:?}: expected isMerged on the merged target, got {target}"
                );
                assert_eq!(
                    target.pointer("/mergeMetadata/mergeOption").and_then(Value::as_str),
                    Some(THREE_WAY),
                    "case {case:?}: expected the three-way strategy recorded, got {target}"
                );
                assert_eq!(
                    target.pointer("/mergeMetadata/mergedBy").and_then(Value::as_str),
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

                // The merge commit records BOTH tips, the destination first, and the
                // destination branch is left at it.
                let written = commit_of(&built.router, repository, &merge_commit).await;
                let parents = parents_of(&written);
                assert_eq!(
                    parents,
                    vec![built.destination_tip.clone(), built.source_tip.clone()],
                    "case {case:?}: expected the destination and source tips as parents, got \
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
                let stored =
                    stored_pull_request(&built.router, &built.pull_request_id).await;
                assert_eq!(
                    stored, merged,
                    "case {case:?}: expected the stored pull request to be the answered one \
                     {merged}, got {stored}"
                );

                // "Pull request status can only change from OPEN to CLOSED": a second
                // merge of the same pull request is refused and writes nothing.
                let again =
                    error(&built.router, "MergePullRequestByThreeWay", &request, 400).await;
                assert_eq!(
                    again, "PullRequestAlreadyClosedException",
                    "case {case:?}: expected a closed pull request to refuse a second merge, got \
                     {again:?}"
                );
                let tip_again = branch_tip(&built.router, repository, DESTINATION).await;
                assert_eq!(
                    tip_again, merge_commit,
                    "case {case:?}: the refused second merge moved the destination branch to \
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
    /// What to do to the repository before the merge, if anything.
    prepare: Prepare,
    /// The `repositoryName` the merge names; `None` sends the repository the pull
    /// request is in.
    repository: Option<&'static str>,
    /// The `pullRequestId` the merge names; `None` sends the created one.
    pull_request_id: Option<&'static str>,
    /// The `sourceCommitId` the merge names, if any. `Base` pins the base commit,
    /// which is NOT the source branch's tip.
    source_commit_id: SourceCommitId,
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
    /// Open a pull request in a SECOND repository and merge against this one.
    PullRequestInAnotherRepository,
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
async fn a_merge_that_names_no_mergeable_pull_request_is_refused_and_writes_nothing() {
    // Rows differ along which part of the request names nothing mergeable: the id's
    // form, the id itself, the repository, the pinned source tip, and a reference
    // the repository no longer holds.
    let refusals = vec![
        Refusal {
            case: "a pull request id that is not an id",
            prepare: Prepare::Nothing,
            repository: None,
            pull_request_id: Some("not-an-id"),
            source_commit_id: SourceCommitId::Absent,
            code: "InvalidPullRequestIdException",
        },
        Refusal {
            case: "a blank pull request id",
            prepare: Prepare::Nothing,
            repository: None,
            pull_request_id: Some("   "),
            source_commit_id: SourceCommitId::Absent,
            code: "PullRequestIdRequiredException",
        },
        Refusal {
            case: "a pull request id no pull request was minted as",
            prepare: Prepare::Nothing,
            repository: None,
            pull_request_id: Some("987654"),
            source_commit_id: SourceCommitId::Absent,
            code: "PullRequestDoesNotExistException",
        },
        Refusal {
            case: "a repository that does not exist",
            prepare: Prepare::Nothing,
            repository: Some("no-such-repository"),
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            code: "RepositoryDoesNotExistException",
        },
        Refusal {
            case: "a pull request held by another repository",
            prepare: Prepare::PullRequestInAnotherRepository,
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            code: "RepositoryNotAssociatedWithPullRequestException",
        },
        Refusal {
            case: "a source commit id that is not a commit id",
            prepare: Prepare::Nothing,
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Malformed,
            code: "InvalidCommitIdException",
        },
        Refusal {
            case: "a source commit id that is not the tip of the source branch",
            prepare: Prepare::Nothing,
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Base,
            code: "TipOfSourceReferenceIsDifferentException",
        },
        Refusal {
            case: "a source reference the repository no longer holds",
            prepare: Prepare::DeleteSourceBranch,
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            code: "TipOfSourceReferenceIsDifferentException",
        },
        Refusal {
            case: "a destination reference the repository no longer holds",
            prepare: Prepare::DeleteDestinationBranch,
            repository: None,
            pull_request_id: None,
            source_commit_id: SourceCommitId::Absent,
            code: "ConcurrentReferenceUpdateException",
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        // Every row is driven against one shape of repository: a base commit, a
        // destination branch that moved, and a source branch that moved. The
        // destination is a branch of its own so the rows that delete it are not
        // refused for deleting the DEFAULT branch.
        let router = emulator();
        let repository = "refused-pulls";
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
            Prepare::PullRequestInAnotherRepository => {
                // The merge below names THIS repository while the pull request it
                // asks about lives in the other one.
                let other = "other-pulls";
                ok(
                    &router,
                    "CreateRepository",
                    &json!({ "repositoryName": other }),
                )
                .await;
                let other_base = commit(
                    &router,
                    other,
                    DESTINATION,
                    None,
                    &[Change::Put("a.txt", "one\n")],
                    "the base",
                )
                .await;
                ok(
                    &router,
                    "CreateBranch",
                    &json!({
                        "repositoryName": other,
                        "branchName": SOURCE,
                        "commitId": other_base,
                    }),
                )
                .await;
                commit(
                    &router,
                    other,
                    SOURCE,
                    Some(&other_base),
                    &[Change::Put("s.txt", "source\n")],
                    "on the source",
                )
                .await;
                let elsewhere = ok(
                    &router,
                    "CreatePullRequest",
                    &json!({
                        "title": case,
                        "targets": [{
                            "repositoryName": other,
                            "sourceReference": SOURCE,
                            "destinationReference": DESTINATION,
                        }],
                    }),
                )
                .await;
                let elsewhere_id = string_at(&elsewhere, "/pullRequest/pullRequestId");
                let answered = error(
                    &router,
                    "MergePullRequestByThreeWay",
                    &json!({
                        "repositoryName": repository,
                        "pullRequestId": elsewhere_id,
                    }),
                    400,
                )
                .await;
                assert_eq!(
                    answered, refusal.code,
                    "case {case:?}: expected the refusal {}, got {answered:?}",
                    refusal.code
                );
                continue;
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
        let answered = error(&router, "MergePullRequestByThreeWay", &request, 400).await;
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
        let target = target_of(&stored, case);
        assert_eq!(
            target
                .pointer("/mergeMetadata/isMerged")
                .and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a refused merge leaves the target unmerged, got {target}"
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
