// SPDX-License-Identifier: CC0-1.0

//! `MergeBranchesByFastForward` over the wire, driven as awsJson1_1 requests through
//! the router that serves the real emulator.
//!
//! The rule under test: a fast-forward merge leaves one stored reference — the
//! `targetBranch`, or the branch the destination specifier names — at the SOURCE tip
//! and writes no commit, and it is admitted exactly where the destination tip IS the
//! merge base of the two: there the destination's history is a prefix of the source's
//! and the reference advances (two tips that are the same commit advance by nothing).
//! Where the destination tip is not the base — the source already part of the
//! destination's history, or two histories neither of which contains the other — the
//! call is refused with `ManualMergeRequiredException` and the repository is left
//! exactly as it was, which is the same answer `GetMergeOptions` gives by offering no
//! fast-forward for that pair.
//!
//! The tables below span the class that rule covers — how far each side moved, which
//! side is ahead, every commit-specifier form for either side, an explicit target branch
//! and none, and each part of the request that can name no valid target — rather than one
//! recorded request.

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
    base64, emulator, emulator_with_engine, error, from_base64, ok, string_at, REGION,
};

const REPOSITORY: &str = "fast-forwards";
/// The repository's default branch — the destination of every merge below — and the
/// branch the source commits are written on.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// A repository whose `main` and `feature` branches start at one base commit and
/// then take a given number of commits each, so a row can put the source any
/// distance ahead of the destination and move the destination too.
struct History {
    router: MultiServiceRouter,
    base_commit: String,
    destination_commit: String,
    source_commit: String,
    /// The path and content of the file the LAST source commit wrote, which a
    /// fast-forward must make readable on the destination branch.
    last_source_file: Option<(String, Vec<u8>)>,
}

impl History {
    async fn build(destination_commits: usize, source_commits: usize) -> Self {
        let router = emulator();
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": REPOSITORY }),
        )
        .await;
        let base = ok(
            &router,
            "CreateCommit",
            &commit_body(DESTINATION, None, "base.txt", b"base\n"),
        )
        .await;
        let base_commit = string_at(&base, "/commitId");
        ok(
            &router,
            "CreateBranch",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": SOURCE,
                "commitId": base_commit,
            }),
        )
        .await;

        let mut destination_commit = base_commit.clone();
        for index in 0..destination_commits {
            let path = format!("destination-{index}.txt");
            let content = format!("destination {index}\n").into_bytes();
            let written = ok(
                &router,
                "CreateCommit",
                &commit_body(
                    DESTINATION,
                    Some(&destination_commit),
                    &path,
                    content.as_slice(),
                ),
            )
            .await;
            destination_commit = string_at(&written, "/commitId");
        }
        let mut source_commit = base_commit.clone();
        let mut last_source_file = None;
        for index in 0..source_commits {
            let path = format!("source-{index}.txt");
            let content = format!("source {index}\n").into_bytes();
            let written = ok(
                &router,
                "CreateCommit",
                &commit_body(SOURCE, Some(&source_commit), &path, content.as_slice()),
            )
            .await;
            source_commit = string_at(&written, "/commitId");
            last_source_file = Some((path, content));
        }
        Self {
            router,
            base_commit,
            destination_commit,
            source_commit,
            last_source_file,
        }
    }

    /// The commit a branch of this repository points at, read back through
    /// `GetBranch`.
    async fn tip_of(&self, branch: &str) -> String {
        let payload = ok(
            &self.router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": branch }),
        )
        .await;
        string_at(&payload, "/branch/commitId")
    }

    /// The tree a commit of this repository carries, read back through
    /// `GetCommit` — so the `treeId` a merge reports is checked against stored
    /// state rather than against itself.
    async fn tree_of(&self, commit_id: &str) -> String {
        let payload = ok(
            &self.router,
            "GetCommit",
            &json!({ "repositoryName": REPOSITORY, "commitId": commit_id }),
        )
        .await;
        string_at(&payload, "/commit/treeId")
    }
}

/// A `CreateCommit` body writing one file on `branch`.
fn commit_body(branch: &str, parent: Option<&str>, path: &str, content: &[u8]) -> Value {
    let mut body = json!({
        "repositoryName": REPOSITORY,
        "branchName": branch,
        "commitMessage": format!("{branch}: {path}"),
        "putFiles": [{ "filePath": path, "fileContent": base64(content) }],
    });
    if let Some(parent) = parent {
        body["parentCommitId"] = json!(parent);
    }
    body
}

/// A `MergeBranchesByFastForward` body naming the two specifiers, and a
/// `targetBranch` only where a row asks for one.
fn request(source: &str, destination: &str, target: Option<&str>) -> Value {
    let mut body = json!({
        "repositoryName": REPOSITORY,
        "sourceCommitSpecifier": source,
        "destinationCommitSpecifier": destination,
    });
    if let Some(target) = target {
        body["targetBranch"] = json!(target);
    }
    body
}

/// One row of the fast-forward table: how far each side moved, and how the request
/// names the two sides and the branch the merge is applied to.
struct Case<'a> {
    label: &'a str,
    /// How many commits the destination branch takes past the base commit. Nothing
    /// past it leaves the destination AT the base, which is what makes the merge a
    /// fast-forward; a destination that moved is a refusal row instead
    /// ([`a_destination_that_is_not_the_merge_base_is_refused_and_changes_nothing`]).
    destination_commits: usize,
    /// How many commits the source branch takes past the base commit.
    source_commits: usize,
    /// How the row names each side: a branch name, a fully qualified reference,
    /// `HEAD`, or a commit id of the built history.
    source: fn(&History) -> String,
    destination: fn(&History) -> String,
    target: Option<&'a str>,
    /// Does the destination specifier FOLLOW the branch (a branch name, a
    /// reference, `HEAD`) rather than pin one commit? A pinned commit id is the
    /// destination tip only until the merge advances the branch past it, which is
    /// what makes a repeat of that one request a different question.
    destination_follows_the_branch: bool,
}

fn branch_source(_: &History) -> String {
    SOURCE.to_string()
}
fn branch_destination(_: &History) -> String {
    DESTINATION.to_string()
}
fn reference_source(_: &History) -> String {
    format!("refs/heads/{SOURCE}")
}
fn reference_destination(_: &History) -> String {
    format!("refs/heads/{DESTINATION}")
}
fn head_destination(_: &History) -> String {
    "HEAD".to_string()
}
fn commit_source(history: &History) -> String {
    history.source_commit.clone()
}
fn commit_destination(history: &History) -> String {
    history.destination_commit.clone()
}

#[tokio::test]
async fn a_fast_forward_leaves_the_target_branch_at_the_descendant_tip() {
    // Rows differ along how far each side moved (the source one commit ahead,
    // several, not at all; the destination ahead with the source already in its
    // history), which specifier form names either side, and whether the request
    // names a target branch of its own.
    let cases: Vec<Case> = vec![
        Case {
            label: "one commit ahead, both sides named by branch",
            destination_commits: 0,
            source_commits: 1,
            source: branch_source,
            destination: branch_destination,
            target: None,
            destination_follows_the_branch: true,
        },
        Case {
            label: "three commits ahead: the whole chain is fast-forwarded",
            destination_commits: 0,
            source_commits: 3,
            source: branch_source,
            destination: branch_destination,
            target: None,
            destination_follows_the_branch: true,
        },
        Case {
            label: "both sides named by fully qualified reference",
            destination_commits: 0,
            source_commits: 2,
            source: reference_source,
            destination: reference_destination,
            target: None,
            destination_follows_the_branch: true,
        },
        Case {
            label: "HEAD names the default branch as the destination",
            destination_commits: 0,
            source_commits: 1,
            source: branch_source,
            destination: head_destination,
            target: None,
            destination_follows_the_branch: true,
        },
        Case {
            label: "a commit id as the source specifier",
            destination_commits: 0,
            source_commits: 2,
            source: commit_source,
            destination: branch_destination,
            target: None,
            destination_follows_the_branch: true,
        },
        Case {
            label: "a commit id destination with an explicit target branch",
            destination_commits: 0,
            source_commits: 1,
            source: branch_source,
            destination: commit_destination,
            target: Some(DESTINATION),
            destination_follows_the_branch: false,
        },
        Case {
            label: "an explicit target branch beside a branch destination",
            destination_commits: 0,
            source_commits: 2,
            source: reference_source,
            destination: branch_destination,
            target: Some(DESTINATION),
            destination_follows_the_branch: true,
        },
        Case {
            label: "nothing to advance: the source IS the destination",
            destination_commits: 0,
            source_commits: 0,
            source: branch_source,
            destination: branch_destination,
            target: None,
            destination_follows_the_branch: true,
        },
    ];

    for case in cases {
        let label = case.label;
        let history = History::build(case.destination_commits, case.source_commits).await;
        // Every row here leaves the destination AT the merge base, so the merge is
        // the SOURCE tip and the destination is the side that did not move.
        let (merged, other) = (&history.source_commit, &history.destination_commit);
        let body = request(
            &(case.source)(&history),
            &(case.destination)(&history),
            case.target,
        );
        let payload = ok(&history.router, "MergeBranchesByFastForward", &body).await;

        // A fast-forward writes no commit: the merge IS one of the two tips, and the
        // tree reported is that commit's tree as stored state holds it.
        assert_eq!(
            &string_at(&payload, "/commitId"),
            merged,
            "case {label:?}: commitId must be the descendant of the two tips, got {payload}"
        );
        let expected_tree = history.tree_of(merged).await;
        assert_eq!(
            string_at(&payload, "/treeId"),
            expected_tree,
            "case {label:?}: treeId must be that commit's tree, got {payload}"
        );
        assert!(
            merged == other || other == &history.base_commit,
            "case {label:?}: the side that did not move must still be at the base commit, \
             got {other:?}"
        );

        // The target branch holds that commit, and the source branch did not move.
        let target = history.tip_of(DESTINATION).await;
        assert_eq!(
            &target, merged,
            "case {label:?}: the target branch must point at the merged commit, got {target:?}"
        );
        let source_branch = history.tip_of(SOURCE).await;
        assert_eq!(
            source_branch, history.source_commit,
            "case {label:?}: the source branch must not move, got {source_branch:?}"
        );

        // What the source wrote is readable on the destination branch now, which is
        // what advancing the reference means for a later reader.
        if let Some((path, content)) = &history.last_source_file {
            let read = ok(
                &history.router,
                "GetFile",
                &json!({
                    "repositoryName": REPOSITORY,
                    "filePath": path,
                    "commitSpecifier": DESTINATION,
                }),
            )
            .await;
            assert_eq!(
                from_base64(&string_at(&read, "/fileContent")),
                *content,
                "case {label:?}: {path} must be readable on {DESTINATION} after the merge"
            );
        }

        // Merging again has nothing left to advance. Where the destination follows
        // the branch it is the same question and the same answer; where it pinned a
        // commit the branch has now advanced past, the reference the merge would
        // advance is no longer at the commit it was evaluated against.
        if case.destination_follows_the_branch {
            let again = ok(&history.router, "MergeBranchesByFastForward", &body).await;
            assert_eq!(
                again, payload,
                "case {label:?}: a repeated fast-forward must answer the same"
            );
        } else {
            let code = error(&history.router, "MergeBranchesByFastForward", &body, 400).await;
            assert_eq!(
                code, "ConcurrentReferenceUpdateException",
                "case {label:?}: repeating the merge against a commit the branch has left must \
                 be refused, got {code}"
            );
        }
        let settled = history.tip_of(DESTINATION).await;
        assert_eq!(
            &settled, merged,
            "case {label:?}: the target branch must still be at the merged commit, \
             got {settled:?}"
        );
    }
}

#[tokio::test]
async fn a_destination_that_is_not_the_merge_base_is_refused_and_changes_nothing() {
    // Rows differ along how the destination came to hold commits the source does
    // not: both sides moved (neither history contains the other), and only the
    // destination moved, by one commit and by several (the source already part of
    // the destination's history). Either way the destination tip is not the merge
    // base, so the reference cannot advance and the merge is not a fast-forward.
    let cases: Vec<(&str, usize, usize)> = vec![
        ("one commit on each side", 1, 1),
        ("two on the destination, three on the source", 2, 3),
        ("three on the destination, one on the source", 3, 1),
        ("the source already merged: one commit on the destination", 1, 0),
        ("the source already merged three commits back", 3, 0),
    ];

    for (label, destination_commits, source_commits) in cases {
        let history = History::build(destination_commits, source_commits).await;
        // Every specifier form for either side is the same refusal: what decides it
        // is the stored ancestry, not how the request spelled the two tips.
        let bodies = [
            request(SOURCE, DESTINATION, None),
            request(
                &format!("refs/heads/{SOURCE}"),
                &format!("refs/heads/{DESTINATION}"),
                None,
            ),
            request(&history.source_commit, DESTINATION, Some(DESTINATION)),
            request(SOURCE, "HEAD", Some(DESTINATION)),
        ];
        for body in bodies {
            let code = error(
                &history.router,
                "MergeBranchesByFastForward",
                &body,
                400,
            )
            .await;
            assert_eq!(
                code, "ManualMergeRequiredException",
                "case {label:?} with {body}: expected ManualMergeRequiredException, got {code}"
            );
        }

        // Nothing was written: both tips are where the history left them.
        let destination = history.tip_of(DESTINATION).await;
        assert_eq!(
            destination, history.destination_commit,
            "case {label:?}: a refused merge must leave the destination branch alone, \
             got {destination:?}"
        );
        let source = history.tip_of(SOURCE).await;
        assert_eq!(
            source, history.source_commit,
            "case {label:?}: a refused merge must leave the source branch alone, got {source:?}"
        );
    }
}

#[tokio::test]
async fn a_fast_forward_is_refused_for_what_it_cannot_be_asked_about() {
    // One commit ahead on the source, so every refusal below is about the member it
    // names rather than about mergeability.
    let history = History::build(0, 1).await;
    let router = &history.router;

    // Rows differ along WHICH part of the request cannot be served: the repository,
    // either specifier, and the branch the merge would be applied to.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that does not exist",
            json!({
                "repositoryName": "absent",
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
            }),
            "RepositoryDoesNotExistException",
        ),
        (
            "a repository name no repository can have",
            json!({
                "repositoryName": "not a name",
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
            }),
            "InvalidRepositoryNameException",
        ),
        (
            "a source specifier naming nothing stored",
            request("no-such-branch", DESTINATION, None),
            "CommitDoesNotExistException",
        ),
        (
            "a destination specifier naming nothing stored",
            request(SOURCE, &"0".repeat(40), None),
            "CommitDoesNotExistException",
        ),
        (
            "a blank source specifier",
            request("   ", DESTINATION, None),
            "CommitRequiredException",
        ),
        (
            "a blank destination specifier",
            request(SOURCE, "", None),
            "CommitRequiredException",
        ),
        (
            "a target branch that is no reference name at all",
            request(SOURCE, DESTINATION, Some("not a branch")),
            "InvalidBranchNameException",
        ),
        (
            "a target branch holding the sequence git refuses",
            request(SOURCE, DESTINATION, Some("release..1")),
            "InvalidBranchNameException",
        ),
        (
            "a target branch this repository does not hold",
            request(SOURCE, DESTINATION, Some("no-such-branch")),
            "BranchDoesNotExistException",
        ),
        (
            "a target branch that is not at the destination tip",
            request(SOURCE, DESTINATION, Some(SOURCE)),
            "ConcurrentReferenceUpdateException",
        ),
        (
            "a destination naming a commit but no branch, with no target branch",
            request(SOURCE, &history.base_commit, None),
            "InvalidTargetBranchException",
        ),
    ];

    for (label, body, expected) in cases {
        let code = error(router, "MergeBranchesByFastForward", &body, 400).await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code}"
        );
    }

    // Every row above was refused before anything was written.
    let destination = history.tip_of(DESTINATION).await;
    assert_eq!(
        destination, history.base_commit,
        "case \"after the refusals\": the destination branch must still be at the base commit, \
         got {destination:?}"
    );
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

/// Store `content` as a blob of this repository and hand back the tree entry that
/// names it.
fn entry(repository: &mut Repository, content: &[u8]) -> FileEntry {
    let blob_id = repository.insert_blob(content.to_vec());
    FileEntry {
        blob_id,
        file_mode: FILE_MODE_NORMAL.to_string(),
        size: i64::try_from(content.len()).unwrap_or(i64::MAX),
    }
}

/// Commit one file on `branch`, moving that branch to the new commit.
fn commit_on(
    repository: &mut Repository,
    branch: &str,
    parents: Vec<String>,
    path: &str,
    content: &[u8],
) -> String {
    let file = entry(repository, content);
    let files = BTreeMap::from([(path.to_string(), file)]);
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

/// Install a repository built by `fill` as the emulator's only repository, and hand
/// back the router that serves it.
///
/// The control plane cannot write unrelated histories (every commit it makes is a
/// child of a branch tip) or a tag (CodeCommit has no API that creates one), so a
/// state holding either is reachable only by writing it here, as a git push would
/// have.
fn seeded(fill: impl FnOnce(&mut Repository)) -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    let mut repository = Repository::new(
        REPOSITORY,
        None,
        None,
        BTreeMap::new(),
        REGION,
        SystemTime::now(),
    );
    fill(&mut repository);
    repository.default_branch = Some(DESTINATION.to_string());
    engine
        .state::<CodeCommitState>()
        .lock()
        .repositories
        .insert(REPOSITORY.to_string(), repository);
    router
}

#[tokio::test]
async fn two_tips_that_share_no_commit_cannot_be_fast_forwarded() {
    // Two root commits in one repository: histories that share nothing, so there is
    // no merge base to tell whether either tip is an ancestor of the other.
    let router = seeded(|repository| {
        for (branch, path, content) in [
            (DESTINATION, "left.txt", &b"left\n"[..]),
            (SOURCE, "right.txt", &b"right\n"[..]),
        ] {
            commit_on(repository, branch, Vec::new(), path, content);
        }
    });

    for (label, body) in [
        ("the source into the destination", request(SOURCE, DESTINATION, None)),
        ("the destination into the source", request(DESTINATION, SOURCE, None)),
        (
            "with the target branch named explicitly",
            request(SOURCE, DESTINATION, Some(DESTINATION)),
        ),
    ] {
        let code = error(&router, "MergeBranchesByFastForward", &body, 400).await;
        assert_eq!(
            code, "TipsDivergenceExceededException",
            "case {label:?}: expected TipsDivergenceExceededException, got {code}"
        );
    }
}

#[tokio::test]
async fn a_target_that_is_a_tag_name_is_not_a_branch() {
    // A linear history whose source IS one commit ahead — so the merge itself is a
    // fast-forward — plus a name the repository holds only as a tag.
    let mut tips: Vec<String> = Vec::new();
    let router = seeded(|repository| {
        let base = commit_on(repository, DESTINATION, Vec::new(), "base.txt", b"base\n");
        let ahead = commit_on(
            repository,
            SOURCE,
            vec![base.clone()],
            "ahead.txt",
            b"ahead\n",
        );
        repository.git_tags.insert("release".to_string(), base.clone());
        tips.push(base);
        tips.push(ahead);
    });

    // A name the repository holds only in its tag namespace is a tag name rather
    // than a branch name, whichever branch member names it.
    let label = "the tag named as the target branch";
    let code = error(
        &router,
        "MergeBranchesByFastForward",
        &request(SOURCE, DESTINATION, Some("release")),
        400,
    )
    .await;
    assert_eq!(
        code, "BranchNameIsTagNameException",
        "case {label:?}: expected BranchNameIsTagNameException, got {code}"
    );

    // The branch a real target names still fast-forwards, so the rows above were
    // refused for the name rather than for the history.
    let payload = ok(
        &router,
        "MergeBranchesByFastForward",
        &request(SOURCE, DESTINATION, Some(DESTINATION)),
    )
    .await;
    assert_eq!(
        string_at(&payload, "/commitId"),
        tips[1],
        "case \"a real target branch\": the merge must report the source tip, got {payload}"
    );
}
