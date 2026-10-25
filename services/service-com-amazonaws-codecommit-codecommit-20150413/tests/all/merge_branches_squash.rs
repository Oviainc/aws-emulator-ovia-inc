// SPDX-License-Identifier: CC0-1.0

//! `MergeBranchesBySquash` over the wire, driven as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! The rule under test: the operation merges the source commit into the destination
//! commit — three-way, against the tree of the two commits' merge base, so a path only
//! one side changed takes that side's version, a path both sides changed the same way
//! takes the agreed version, and a path they changed differently is settled by
//! `conflictDetailLevel`, `conflictResolutionStrategy` and `conflictResolution` or else
//! refused — writes the result as ONE commit whose only parent is the destination tip,
//! so the source history is collapsed while the tree stays the tree a three-way merge of
//! those two tips produces, and leaves the branch the merge is applied to (the
//! `targetBranch`, or the branch the destination specifier names) at that commit.
//!
//! The tables below span what the rule generalises over rather than one recorded
//! request: which side changed a path and in what way (content, mode, deletion, a new
//! file, a nested path), the conflict detail level, every conflict resolution strategy
//! the model declares, a `conflictResolution` entry, `keepEmptyFolders`, every
//! commit-specifier form for either side, an explicit target branch and none, and each
//! part of the request that can name no mergeable pair or no valid target. Each row's
//! expectation is the merged tree this file computes from the rule, read back through
//! `GetFolder` as `(blobId, fileMode)` per path — content-addressed, so the answer is
//! checked against the content the rule says the merge produces — and against the tree
//! `CreateUnreferencedMergeCommit` writes for the SAME pair of tips under
//! `THREE_WAY_MERGE`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::ops::types::UserInfo;
use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    Branch, FileEntry, Repository, FILE_MODE_NORMAL,
};
use service_com_amazonaws_codecommit_codecommit_20150413::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::repository_lifecycle::{
    base64, emulator, emulator_with_engine, error, git_blob_id, is_object_id, ok, string_at, REGION,
};
use crate::unreferenced_merge_commits::{
    commit, commit_of, parents_of, tree_of, Change, EXECUTABLE, GITKEEP, NORMAL,
};

/// The destination branch of every merge below — the repository's default branch,
/// which is what `HEAD` names — and the branch the source commits are written on.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// A five-line file, the base of the line-level rows below.
const FIVE_LINES: &str = "one\ntwo\nthree\nfour\nfive\n";

/// How a request names one side of the merge.
#[derive(Debug, Clone, Copy)]
enum Specifier {
    /// The branch name.
    Branch,
    /// The fully qualified reference the branch lives at.
    Reference,
    /// `HEAD`, which names the repository's default branch.
    Head,
    /// The commit id of that side's tip, which pins one commit instead of
    /// following the branch.
    CommitId,
}

/// One row: a base commit, the change each side makes to it, the merge request's
/// conflict members, how the request names the two sides and the target branch, and
/// the tree the merge must produce.
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
    source_as: Specifier,
    destination_as: Specifier,
    /// Does the request name `targetBranch` itself? Without it the merge is applied
    /// to the branch the destination specifier names, which a commit id does not.
    explicit_target: bool,
    /// The merged tree, as the rule says it looks: every path, its content and its
    /// mode.
    expected: Vec<(&'static str, &'static str, &'static str)>,
}

impl Row {
    /// A row with the defaults: no conflict members, no kept folders, both sides
    /// named by branch and no target branch of its own.
    fn new(
        case: &'static str,
        base: Vec<(&'static str, &'static str)>,
        destination: Vec<Change>,
        source: Vec<Change>,
        expected: Vec<(&'static str, &'static str, &'static str)>,
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
            source_as: Specifier::Branch,
            destination_as: Specifier::Branch,
            explicit_target: false,
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

    /// Name the two sides the given way, and the target branch explicitly where the
    /// row asks for it.
    fn named(mut self, source: Specifier, destination: Specifier, explicit_target: bool) -> Self {
        self.source_as = source;
        self.destination_as = destination;
        self.explicit_target = explicit_target;
        self
    }
}

/// The table: every row is one merge whose result the rule fixes.
fn rows() -> Vec<Row> {
    vec![
        // Each side adds a file the other does not touch: both land, and the squash
        // commit lands on the destination branch.
        Row::new(
            "disjoint-additions",
            vec![("README.md", "# base\n")],
            vec![Change::Put("dest/only.txt", "destination\n")],
            vec![Change::Put("src/only.txt", "source\n")],
            vec![
                ("README.md", "# base\n", NORMAL),
                ("dest/only.txt", "destination\n", NORMAL),
                ("src/only.txt", "source\n", NORMAL),
            ],
        ),
        // Only the source changed a path, and both sides are named by fully
        // qualified reference.
        Row::new(
            "one-sided-source-change-by-reference",
            vec![("a.txt", "one\n"), ("keep.txt", "keep\n")],
            vec![],
            vec![Change::Put("a.txt", "source\n")],
            vec![
                ("a.txt", "source\n", NORMAL),
                ("keep.txt", "keep\n", NORMAL),
            ],
        )
        .named(Specifier::Reference, Specifier::Reference, false),
        // Only the destination changed a path, and it changed a NESTED one; `HEAD`
        // names the destination.
        Row::new(
            "one-sided-nested-destination-change",
            vec![("deep/a/b/c.txt", "one\n")],
            vec![Change::Put("deep/a/b/c.txt", "destination\n")],
            vec![Change::Put("other.txt", "source\n")],
            vec![
                ("deep/a/b/c.txt", "destination\n", NORMAL),
                ("other.txt", "source\n", NORMAL),
            ],
        )
        .named(Specifier::Branch, Specifier::Head, false),
        // Both sides made the SAME change: an agreement, not a conflict. The source
        // is named by commit id and the target branch explicitly.
        Row::new(
            "both-sides-agree",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "two\n")],
            vec![Change::Put("a.txt", "two\n")],
            vec![("a.txt", "two\n", NORMAL)],
        )
        .named(Specifier::CommitId, Specifier::Branch, true),
        // A destination named by COMMIT ID, which names no branch, beside an
        // explicit target branch: the merge is applied to the branch the request
        // names.
        Row::new(
            "commit-id-destination-with-explicit-target",
            vec![("a.txt", "one\n")],
            vec![Change::Put("b.txt", "destination\n")],
            vec![Change::Put("c.txt", "source\n")],
            vec![
                ("a.txt", "one\n", NORMAL),
                ("b.txt", "destination\n", NORMAL),
                ("c.txt", "source\n", NORMAL),
            ],
        )
        .named(Specifier::Branch, Specifier::CommitId, true),
        // One side deleted a path the other left alone: the deletion is the change,
        // so it lands.
        Row::new(
            "one-sided-deletion",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![Change::Delete("b/c.txt")],
            vec![Change::Put("a.txt", "source\n")],
            vec![("a.txt", "source\n", NORMAL)],
        ),
        // The same deletion with `keepEmptyFolders`: "a .gitkeep file is created for
        // empty folders".
        Row::new(
            "one-sided-deletion-keeping-folders",
            vec![("a.txt", "one\n"), ("b/c.txt", "nested\n")],
            vec![Change::Delete("b/c.txt")],
            vec![Change::Put("a.txt", "source\n")],
            vec![("a.txt", "source\n", NORMAL), ("b/.gitkeep", "", NORMAL)],
        )
        .keeping_empty_folders(),
        // A mode-only change on one side: the mode is part of the entry, so it
        // merges like content does.
        Row::new(
            "one-sided-mode-change",
            vec![("run.sh", "#!/bin/sh\n")],
            vec![Change::Put("other.txt", "destination\n")],
            vec![Change::PutMode("run.sh", "#!/bin/sh\n", EXECUTABLE)],
            vec![
                ("other.txt", "destination\n", NORMAL),
                ("run.sh", "#!/bin/sh\n", EXECUTABLE),
            ],
        ),
        // LINE_LEVEL detail with the two sides changing DIFFERENT lines: not a
        // conflict, and the merged file carries both changes.
        Row::new(
            "line-level-disjoint-lines",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "ONE\ntwo\nthree\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nthree\nfour\nFIVE\n")],
            vec![("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n", NORMAL)],
        )
        .with_detail_level("LINE_LEVEL"),
        // ACCEPT_SOURCE on a file both sides changed: "which branch to use when
        // resolving conflicts".
        Row::new(
            "accept-source",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            vec![("a.txt", "source\n", NORMAL)],
        )
        .with_strategy("ACCEPT_SOURCE"),
        // ACCEPT_DESTINATION on the same conflict.
        Row::new(
            "accept-destination",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            vec![("a.txt", "destination\n", NORMAL)],
        )
        .with_strategy("ACCEPT_DESTINATION")
        .named(Specifier::Reference, Specifier::Branch, true),
        // AUTOMERGE at the default FILE_LEVEL detail: the strategy itself is
        // "whether to attempt automatically merging two versions of a file".
        Row::new(
            "automerge-disjoint-lines",
            vec![("a.txt", FIVE_LINES)],
            vec![Change::Put("a.txt", "ONE\ntwo\nthree\nfour\nfive\n")],
            vec![Change::Put("a.txt", "one\ntwo\nthree\nfour\nFIVE\n")],
            vec![("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n", NORMAL)],
        )
        .with_strategy("AUTOMERGE"),
        // AUTOMERGE with a USE_NEW_CONTENT resolution entry: the caller's own bytes
        // settle the path, under the mode the entry names.
        Row::new(
            "automerge-use-new-content-with-mode",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            vec![("a.txt", "#!/bin/sh\n", EXECUTABLE)],
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{
                "filePath": "a.txt",
                "replacementType": "USE_NEW_CONTENT",
                "content": base64(b"#!/bin/sh\n"),
                "fileMode": EXECUTABLE,
            }],
        })),
        // AUTOMERGE with KEEP_BASE: the version the merge base held.
        Row::new(
            "automerge-keep-base",
            vec![("a.txt", "one\n")],
            vec![Change::Put("a.txt", "destination\n")],
            vec![Change::Put("a.txt", "source\n")],
            vec![("a.txt", "one\n", NORMAL)],
        )
        .with_strategy("AUTOMERGE")
        .with_resolution(json!({
            "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_BASE" }],
        })),
    ]
}

/// The specifier one side of the merge is named by in a request.
fn specifier(form: Specifier, branch: &str, tip: &str) -> String {
    match form {
        Specifier::Branch => branch.to_string(),
        Specifier::Reference => format!("refs/heads/{branch}"),
        Specifier::Head => "HEAD".to_string(),
        Specifier::CommitId => tip.to_string(),
    }
}

/// The merge request one row sends, which is also the `CreateUnreferencedMergeCommit`
/// request its three-way cross-check sends (`mergeOption` and `targetBranch` apart).
fn request(row: &Row, repository: &str, source_tip: &str, destination_tip: &str) -> Value {
    let mut input = json!({
        "repositoryName": repository,
        "sourceCommitSpecifier": specifier(row.source_as, SOURCE, source_tip),
        "destinationCommitSpecifier": specifier(row.destination_as, DESTINATION, destination_tip),
        "commitMessage": format!("squash merge for {}", row.case),
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
    input
}

/// The commit a branch of a repository points at, read back through `GetBranch`.
async fn tip_of(router: &MultiServiceRouter, repository: &str, branch: &str) -> String {
    let payload = ok(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": branch }),
    )
    .await;
    string_at(&payload, "/branch/commitId")
}

/// Build a repository with a base commit on `main`, a `feature` branch at it, and
/// one commit of each side's changes, reporting `(destination tip, source tip)`.
async fn history(router: &MultiServiceRouter, repository: &str, row: &Row) -> (String, String) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let base_changes: Vec<Change> = row
        .base
        .iter()
        .map(|(path, content)| Change::Put(path, content))
        .collect();
    let base = commit(router, repository, DESTINATION, None, &base_changes, "base").await;
    ok(
        router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    let destination_tip = if row.destination.is_empty() {
        base.clone()
    } else {
        commit(
            router,
            repository,
            DESTINATION,
            Some(&base),
            &row.destination,
            "destination",
        )
        .await
    };
    let source_tip = if row.source.is_empty() {
        base.clone()
    } else {
        commit(
            router,
            repository,
            SOURCE,
            Some(&base),
            &row.source,
            "source",
        )
        .await
    };
    (destination_tip, source_tip)
}

#[tokio::test]
async fn a_squash_merge_writes_the_three_way_tree_as_one_commit_on_the_target_branch() {
    let router = emulator();

    for row in rows() {
        let case = row.case;
        let repository = format!("squash-{case}");
        let (destination_tip, source_tip) = history(&router, &repository, &row).await;

        // The expectation, as the rule says the merged tree looks: the git object
        // name of each file's bytes, under its mode.
        let expected: BTreeMap<String, (String, String)> = row
            .expected
            .iter()
            .map(|(path, content, mode)| {
                (
                    (*path).to_string(),
                    (git_blob_id(content.as_bytes()), (*mode).to_string()),
                )
            })
            .collect();
        if row.keep_empty_folders {
            assert!(
                expected.keys().any(|path| path.ends_with(GITKEEP)),
                "case {case:?}: a kept-folder row must expect a {GITKEEP} placeholder"
            );
        }

        // The SAME merge written as an unreferenced three-way merge commit, which
        // moves no reference: the tree a squash of these two tips must agree with.
        let mut three_way_input = request(&row, &repository, &source_tip, &destination_tip);
        three_way_input["mergeOption"] = json!("THREE_WAY_MERGE");
        let three_way = ok(&router, "CreateUnreferencedMergeCommit", &three_way_input).await;
        let three_way_id = string_at(&three_way, "/commitId");
        let three_way_tree = string_at(&three_way, "/treeId");

        let mut body = request(&row, &repository, &source_tip, &destination_tip);
        if row.explicit_target {
            body["targetBranch"] = json!(DESTINATION);
        }
        let payload = ok(&router, "MergeBranchesBySquash", &body).await;
        let commit_id = string_at(&payload, "/commitId");
        let tree_id = string_at(&payload, "/treeId");
        assert!(
            is_object_id(&commit_id),
            "case {case:?}: expected a 40-character commitId, got {commit_id:?}"
        );
        assert!(
            is_object_id(&tree_id),
            "case {case:?}: expected a 40-character treeId, got {tree_id:?}"
        );

        // The squash KEEPS the three-way tree and COLLAPSES the history: same tree,
        // a different commit, and the destination tip as the only parent.
        assert_eq!(
            tree_id, three_way_tree,
            "case {case:?}: a squash merge must keep the three-way tree, got {tree_id} against \
             {three_way_tree}"
        );
        assert_ne!(
            commit_id, three_way_id,
            "case {case:?}: a squash merge collapses the history, so its commit must differ from \
             the three-way one, got {commit_id}"
        );
        let squash_commit = commit_of(&router, &repository, &commit_id).await;
        assert_eq!(
            parents_of(&squash_commit),
            vec![destination_tip.clone()],
            "case {case:?}: a squash merge commit records the destination tip alone, got \
             {squash_commit}"
        );
        assert_eq!(
            string_at(&squash_commit, "/treeId"),
            tree_id,
            "case {case:?}: GetCommit must report the tree the merge reported, got {squash_commit}"
        );

        // The merged tree itself, read back at the commit the merge wrote.
        let actual = tree_of(&router, &repository, &commit_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the merged tree {expected:?}, got {actual:?}"
        );

        // The branch the merge was applied to holds that commit — so the merged
        // content is what a later reader of that branch sees — and the source branch
        // did not move.
        let target_tip = tip_of(&router, &repository, DESTINATION).await;
        assert_eq!(
            target_tip, commit_id,
            "case {case:?}: the target branch must point at the merge commit, got {target_tip:?}"
        );
        let on_branch = tree_of(&router, &repository, DESTINATION).await;
        assert_eq!(
            on_branch, expected,
            "case {case:?}: the target branch must read back the merged tree, got {on_branch:?}"
        );
        let source_branch = tip_of(&router, &repository, SOURCE).await;
        assert_eq!(
            source_branch, source_tip,
            "case {case:?}: the source branch must not move, got {source_branch:?}"
        );
    }
}

#[tokio::test]
async fn a_repeated_squash_merge_keeps_the_tree_and_chains_onto_what_it_wrote() {
    // The source one commit ahead and the destination one commit ahead of their
    // shared base: a merge with something to do on both sides.
    let row = Row::new(
        "repeated",
        vec![("a.txt", "one\n")],
        vec![Change::Put("destination.txt", "destination\n")],
        vec![Change::Put("source.txt", "source\n")],
        vec![
            ("a.txt", "one\n", NORMAL),
            ("destination.txt", "destination\n", NORMAL),
            ("source.txt", "source\n", NORMAL),
        ],
    );
    let router = emulator();
    let repository = "squash-repeated";
    let (destination_tip, source_tip) = history(&router, repository, &row).await;
    let body = request(&row, repository, &source_tip, &destination_tip);

    let first = ok(&router, "MergeBranchesBySquash", &body).await;
    let first_id = string_at(&first, "/commitId");
    let first_tree = string_at(&first, "/treeId");

    // The destination specifier follows the branch, so the second call merges the
    // same source into what the first call wrote: the source's changes are already
    // there, so the tree does not move, and the commit chains onto the first.
    let second = ok(&router, "MergeBranchesBySquash", &body).await;
    let second_id = string_at(&second, "/commitId");
    assert_eq!(
        string_at(&second, "/treeId"),
        first_tree,
        "case \"repeated\": a repeated squash merge must produce the same tree, got {second}"
    );
    let second_commit = commit_of(&router, repository, &second_id).await;
    assert_eq!(
        parents_of(&second_commit),
        vec![first_id.clone()],
        "case \"repeated\": the second merge must record the branch tip the first one left, got \
         {second_commit}"
    );
    let tip = tip_of(&router, repository, DESTINATION).await;
    assert_eq!(
        tip, second_id,
        "case \"repeated\": the target branch must be at the second merge commit, got {tip:?}"
    );

    // And the pinned-commit form of the same request is now a merge against a commit
    // the branch has left behind.
    let mut pinned = body.clone();
    pinned["destinationCommitSpecifier"] = json!(destination_tip);
    pinned["targetBranch"] = json!(DESTINATION);
    let code = error(&router, "MergeBranchesBySquash", &pinned, 400).await;
    assert_eq!(
        code, "ConcurrentReferenceUpdateException",
        "case \"repeated\": merging against a commit the target branch has left must be refused, \
         got {code}"
    );
}

#[tokio::test]
async fn a_squash_merge_is_refused_for_what_it_cannot_be_asked_about() {
    // The source and the destination each one commit past their shared base, with
    // one path they changed DIFFERENTLY: a conflict at the default detail level.
    let row = Row::new(
        "refusals",
        vec![("a.txt", "one\n"), ("b.txt", "two\n")],
        vec![Change::Put("a.txt", "destination\n")],
        vec![Change::Put("a.txt", "source\n")],
        Vec::new(),
    );
    let router = emulator();
    let repository = "squash-refusals";
    let (destination_tip, source_tip) = history(&router, repository, &row).await;
    let mergeable = Row::new("mergeable", Vec::new(), Vec::new(), Vec::new(), Vec::new());

    // One refusal row is a request and the modelled error code it must be refused
    // with.
    //
    // A request naming the two sides by branch, which each row below changes one
    // part of. The merge the `refusals` history admits is a CONFLICT, so the rows
    // that are not about mergeability name the mergeable row's request instead,
    // whose refusal the handler reaches before it evaluates anything.
    let body = |row: &Row| request(row, repository, &source_tip, &destination_tip);
    let with = |row: &Row, member: &str, value: Value| -> Value {
        let mut input = body(row);
        input[member] = value;
        input
    };

    // Rows differ along WHICH part of the request cannot be served: the merge
    // itself, the conflict members, the repository, either specifier, and the branch
    // the merge would be applied to.
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a path both sides changed differently, with nothing to settle it",
            body(&row),
            "ManualMergeRequiredException",
        ),
        (
            "a conflict resolution list without the AUTOMERGE strategy",
            with(
                &row,
                "conflictResolution",
                json!({
                    "replaceContents": [{ "filePath": "a.txt", "replacementType": "KEEP_SOURCE" }],
                }),
            ),
            "InvalidConflictResolutionException",
        ),
        (
            "a conflict detail level the model does not declare",
            with(&mergeable, "conflictDetailLevel", json!("WORD_LEVEL")),
            "InvalidConflictDetailLevelException",
        ),
        (
            "a conflict resolution strategy the model does not declare",
            with(
                &mergeable,
                "conflictResolutionStrategy",
                json!("ACCEPT_EITHER"),
            ),
            "InvalidConflictResolutionStrategyException",
        ),
        (
            "a repository that does not exist",
            with(&mergeable, "repositoryName", json!("absent")),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name no repository can have",
            with(&mergeable, "repositoryName", json!("not a name")),
            "InvalidRepositoryNameException",
        ),
        (
            "a source specifier naming nothing stored",
            with(&mergeable, "sourceCommitSpecifier", json!("no-such-branch")),
            "CommitDoesNotExistException",
        ),
        (
            "a destination specifier naming nothing stored",
            with(
                &mergeable,
                "destinationCommitSpecifier",
                json!("0".repeat(40)),
            ),
            "CommitDoesNotExistException",
        ),
        (
            "a blank source specifier",
            with(&mergeable, "sourceCommitSpecifier", json!("   ")),
            "CommitRequiredException",
        ),
        (
            "a blank destination specifier",
            with(&mergeable, "destinationCommitSpecifier", json!("")),
            "CommitRequiredException",
        ),
        (
            "a target branch that is no reference name at all",
            with(&mergeable, "targetBranch", json!("not a branch")),
            "InvalidBranchNameException",
        ),
        (
            "a target branch holding the sequence git refuses",
            with(&mergeable, "targetBranch", json!("release..1")),
            "InvalidBranchNameException",
        ),
        (
            "a target branch this repository does not hold",
            with(&mergeable, "targetBranch", json!("no-such-branch")),
            "BranchDoesNotExistException",
        ),
        (
            "a target branch that is not at the destination tip",
            with(&mergeable, "targetBranch", json!(SOURCE)),
            "ConcurrentReferenceUpdateException",
        ),
        (
            "a destination naming a commit but no branch, with no target branch",
            with(
                &mergeable,
                "destinationCommitSpecifier",
                json!(destination_tip.clone()),
            ),
            "InvalidTargetBranchException",
        ),
    ];

    for (label, body, expected) in refusals {
        let code = error(&router, "MergeBranchesBySquash", &body, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }

    // Every row above was refused before anything was written: both branches are
    // still at the tips the history left them at.
    for (branch, tip) in [(DESTINATION, &destination_tip), (SOURCE, &source_tip)] {
        let settled = tip_of(&router, repository, branch).await;
        assert_eq!(
            &settled, tip,
            "case \"after the refusals\": the branch {branch} must still be at {tip}, got \
             {settled:?}"
        );
    }
}

/// The identity every seeded commit carries; a commit's author is not what this
/// suite is about.
fn identity() -> UserInfo {
    UserInfo {
        name: Some("tester".to_string()),
        email: Some("tester@example.com".to_string()),
        date: Some("1700000000 +0000".to_string()),
    }
}

/// Commit one file on `branch` of a repository built by hand, moving that branch to
/// the new commit.
fn commit_on(
    repository: &mut Repository,
    branch: &str,
    parents: Vec<String>,
    path: &str,
    content: &[u8],
) -> String {
    let blob_id = repository.insert_blob(content.to_vec());
    let files = BTreeMap::from([(
        path.to_string(),
        FileEntry {
            blob_id,
            file_mode: FILE_MODE_NORMAL.to_string(),
            size: i64::try_from(content.len()).unwrap_or(i64::MAX),
        },
    )]);
    let (commit_id, _) = repository
        .commit_snapshot(files, parents, identity(), identity(), Some("a seeded commit"))
        .expect("the snapshot's blobs are stored");
    repository.branches.insert(
        branch.to_string(),
        Branch {
            name: branch.to_string(),
            commit_id: commit_id.clone(),
        },
    );
    commit_id
}

#[tokio::test]
async fn a_target_that_is_a_tag_name_is_not_a_branch() {
    // The control plane has no API that creates a tag, so a repository holding one
    // is reachable only by writing it here, as a git push would have.
    let repository = "squash-tag-target";
    let (router, engine) = emulator_with_engine();
    let mut built = Repository::new(
        repository,
        None,
        None,
        BTreeMap::new(),
        REGION,
        SystemTime::now(),
    );
    let base = commit_on(&mut built, DESTINATION, Vec::new(), "base.txt", b"base\n");
    commit_on(
        &mut built,
        SOURCE,
        vec![base.clone()],
        "ahead.txt",
        b"ahead\n",
    );
    built.git_tags.insert("release".to_string(), base.clone());
    built.default_branch = Some(DESTINATION.to_string());
    engine
        .state::<CodeCommitState>()
        .lock()
        .repositories
        .insert(repository.to_string(), built);

    let mut body = json!({
        "repositoryName": repository,
        "sourceCommitSpecifier": SOURCE,
        "destinationCommitSpecifier": DESTINATION,
        "commitMessage": "a squash onto a tag name",
        "authorName": "Merge Tester",
        "email": "merge.tester@example.com",
        "targetBranch": "release",
    });
    let code = error(&router, "MergeBranchesBySquash", &body, 400).await;
    assert_eq!(
        code, "BranchNameIsTagNameException",
        "case \"the tag named as the target branch\": expected BranchNameIsTagNameException, got \
         {code}"
    );

    // The branch a real target names still merges, so the row above was refused for
    // the name rather than for the history.
    body["targetBranch"] = json!(DESTINATION);
    let payload = ok(&router, "MergeBranchesBySquash", &body).await;
    let merged = string_at(&payload, "/commitId");
    let tip = tip_of(&router, repository, DESTINATION).await;
    assert_eq!(
        tip, merged,
        "case \"a real target branch\": the target branch must be at the merge commit, got \
         {tip:?}"
    );
    let tree = tree_of(&router, repository, &merged).await;
    // Each seeded commit stores its OWN whole snapshot, so the source commit both
    // added `ahead.txt` and dropped `base.txt`; the destination is still at the base
    // commit, so the merge takes the source's version of each path.
    let expected: BTreeMap<String, (String, String)> = [("ahead.txt", &b"ahead\n"[..])]
        .into_iter()
        .map(|(path, content)| (path.to_string(), (git_blob_id(content), NORMAL.to_string())))
        .collect();
    assert_eq!(
        tree, expected,
        "case \"a real target branch\": the merged tree must be the source's snapshot, got \
         {tree:?}"
    );
}
