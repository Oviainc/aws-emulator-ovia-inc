// SPDX-License-Identifier: CC0-1.0

//! Which merges CodeCommit admits, and which requests about a merge it refuses
//! before it evaluates anything — over the wire, as awsJson1_1 requests through the
//! router that serves the real emulator.
//!
//! Three rules, each asserted over a table that spans what it generalises over
//! rather than one recorded request:
//!
//! 1. A FAST-FORWARD is admitted exactly where the destination tip IS the merge base
//!    of the two tips: only then is the destination's history a prefix of the
//!    source's and only then can the reference advance. Every other relation —
//!    the source already part of the destination's history, two histories neither of
//!    which contains the other — is refused with HTTP 400
//!    `ManualMergeRequiredException` and writes nothing. It is ONE rule, so the
//!    operation that REPORTS which strategies a pair of tips admits
//!    (`GetMergeOptions`) and the two that PERFORM the fast-forward
//!    (`MergeBranchesByFastForward`, `MergePullRequestByFastForward`) are asserted
//!    against each other here: a pair for which no fast-forward is offered is a pair
//!    neither merge performs.
//!
//! 2. A `conflictResolution` is "if AUTOMERGE is the conflict resolution strategy, a
//!    list of inputs to use when resolving conflicts during a merge": supplied under
//!    any other strategy it cannot be honoured, so the COMBINATION is refused with
//!    `InvalidConflictResolutionException` naming the strategy in effect — and the
//!    entries of a list the merge will not read are never examined, so a request
//!    whose entry carries a `replacementType` outside the modelled enum earns that
//!    same refusal rather than one about the entry. Under `AUTOMERGE` the entries
//!    ARE read, which is the control each row is checked against.
//!
//! 3. A PULL REQUEST is between two sides that differ: references naming one branch,
//!    and two differently named branches holding the SAME tip, are both refused with
//!    `SourceAndDestinationAreSameException` — the two conditions its sentence
//!    states — and a reference the repository does not hold with
//!    `ReferenceDoesNotExistException`, whose message names the specifier that
//!    resolved to nothing.
//!
//! Every refusal is checked to have written nothing, by reading the branch tips back
//! through `GetBranch` and the pull request through `GetPullRequest`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{emulator, ok, string_at};
use crate::unreferenced_merge_commits::{commit, Change};
use crate::wire::fresh_client_request_token;

/// The status every error below models (`errorKind: client`, `httpError: 400` on
/// each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

const REPOSITORY: &str = "merge-rules";
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// `MergeOptionTypeEnum`'s fast-forward value, which `GetMergeOptions` reports and
/// the two fast-forward merges perform
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`).
const FAST_FORWARD: &str = "FAST_FORWARD_MERGE";

/// The one sentence the service answers `ManualMergeRequiredException` with on every
/// merge path, naming nothing of the request.
const MANUAL_MERGE_MESSAGE: &str =
    "Current branches cannot be merged, please try to merge using a Git client locally.";

/// The one sentence `SourceAndDestinationAreSameException` carries, covering both
/// conditions it is raised for.
const SOURCE_AND_DESTINATION_ARE_SAME_MESSAGE: &str =
    "The source branch name and the destination branch name are either the same, or the tips of \
     both branches are identical. Make sure that your source branch and destination branch have \
     unique names and that the tips of the branches are different.";

/// The sentence the model documents on `ReferenceDoesNotExistException`
/// (`smithy_query(query="shape",shape="ReferenceDoesNotExistException")`), which the
/// service answers where the request named no specifier to state.
const REFERENCE_DOES_NOT_EXIST_MESSAGE: &str = "The specified reference does not exist.";

/// `InvalidConflictResolutionException`'s message for a resolution list the
/// request's strategy cannot honour, with the strategy in effect filled in.
fn unhonourable_resolution_message(strategy: &str) -> String {
    format!(
        "Conflict resolution parameter cannot be used together with {strategy} conflict \
         resolution strategy"
    )
}

/// The content each extra commit on a side writes, so a row can take a side any
/// distance past the base without two commits colliding.
const CONTENTS: [&str; 3] = ["one\n", "two\n", "three\n"];

/// A repository whose `main` and `feature` branches start at one base commit and
/// then take `destination_commits` and `source_commits` commits of their own, on
/// paths only that side touches.
struct History {
    router: MultiServiceRouter,
    destination_tip: String,
    source_tip: String,
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
        let base = commit(
            &router,
            REPOSITORY,
            DESTINATION,
            None,
            &[Change::Put("base.txt", "base\n")],
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
        let mut destination_tip = base.clone();
        for content in CONTENTS.iter().take(destination_commits) {
            destination_tip = commit(
                &router,
                REPOSITORY,
                DESTINATION,
                Some(&destination_tip),
                &[Change::Put("destination.txt", content)],
                "a destination commit",
            )
            .await;
        }
        let mut source_tip = base.clone();
        for content in CONTENTS.iter().take(source_commits) {
            source_tip = commit(
                &router,
                REPOSITORY,
                SOURCE,
                Some(&source_tip),
                &[Change::Put("source.txt", content)],
                "a source commit",
            )
            .await;
        }
        Self {
            router,
            destination_tip,
            source_tip,
        }
    }

    async fn tip_of(&self, branch: &str) -> String {
        let read = ok(
            &self.router,
            "GetBranch",
            &json!({ "repositoryName": REPOSITORY, "branchName": branch }),
        )
        .await;
        string_at(&read, "/branch/commitId")
    }

    /// Open a pull request from `feature` into `main`, reporting its id.
    async fn pull_request(&self) -> String {
        let created = ok(
            &self.router,
            "CreatePullRequest",
            &json!({
                "title": "a merge the rule decides",
                "targets": [{
                    "repositoryName": REPOSITORY,
                    "sourceReference": SOURCE,
                    "destinationReference": DESTINATION,
                }],
                "clientRequestToken": fresh_client_request_token(),
            }),
        )
        .await;
        string_at(&created, "/pullRequest/pullRequestId")
    }
}

/// One row of the fast-forward table: how far each side moved past the base, and
/// whether the destination tip is then the merge base — the one condition that
/// admits a fast-forward.
struct Relation {
    case: &'static str,
    destination_commits: usize,
    source_commits: usize,
    advances: bool,
}

/// Rows differ along which side holds commits the other does not, and by how many:
/// only the source (the reference can advance), only the destination (the source is
/// already merged), both (diverged), and neither (one commit, two references).
fn relations() -> Vec<Relation> {
    vec![
        Relation {
            case: "the source one commit ahead",
            destination_commits: 0,
            source_commits: 1,
            advances: true,
        },
        Relation {
            case: "the source three commits ahead",
            destination_commits: 0,
            source_commits: 3,
            advances: true,
        },
        Relation {
            case: "both references at the same commit",
            destination_commits: 0,
            source_commits: 0,
            advances: true,
        },
        Relation {
            case: "only the destination moved, by one commit",
            destination_commits: 1,
            source_commits: 0,
            advances: false,
        },
        Relation {
            case: "only the destination moved, by three commits",
            destination_commits: 3,
            source_commits: 0,
            advances: false,
        },
        Relation {
            case: "one commit on each side",
            destination_commits: 1,
            source_commits: 1,
            advances: false,
        },
        Relation {
            case: "two on the destination, three on the source",
            destination_commits: 2,
            source_commits: 3,
            advances: false,
        },
    ]
}

#[tokio::test]
async fn a_fast_forward_is_offered_and_performed_by_the_one_ancestry_rule() {
    for relation in relations() {
        let case = relation.case;
        let advances = relation.advances;
        let history =
            History::build(relation.destination_commits, relation.source_commits).await;

        // What the REPORTING operation says the pair admits.
        let options = ok(
            &history.router,
            "GetMergeOptions",
            &json!({
                "repositoryName": REPOSITORY,
                "sourceCommitSpecifier": SOURCE,
                "destinationCommitSpecifier": DESTINATION,
            }),
        )
        .await;
        let offered = options
            .get("mergeOptions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no mergeOptions in {options}"))
            .iter()
            .any(|option| option.as_str() == Some(FAST_FORWARD));
        assert_eq!(
            offered, advances,
            "case {case:?}: expected the fast-forward option offered: {advances}, got {options}"
        );
        assert_eq!(
            string_at(&options, "/baseCommitId") == history.destination_tip,
            advances,
            "case {case:?}: the option is offered exactly where the destination tip is the \
             merge base, got {options}"
        );

        // What the BRANCH merge does with the same pair.
        let request = json!({
            "repositoryName": REPOSITORY,
            "sourceCommitSpecifier": SOURCE,
            "destinationCommitSpecifier": DESTINATION,
            "targetBranch": DESTINATION,
        });
        if advances {
            let merged = ok(&history.router, "MergeBranchesByFastForward", &request).await;
            assert_eq!(
                string_at(&merged, "/commitId"),
                history.source_tip,
                "case {case:?}: an advance leaves the source tip as the merge, got {merged}"
            );
            let tip = history.tip_of(DESTINATION).await;
            assert_eq!(
                tip, history.source_tip,
                "case {case:?}: the target branch must hold the source tip, got {tip:?}"
            );
        } else {
            let (code, message) = error_with_message(
                &history.router,
                "MergeBranchesByFastForward",
                &request,
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "ManualMergeRequiredException",
                "case {case:?}: expected the modelled refusal, got {code:?} saying {message:?}"
            );
            assert_eq!(
                message, MANUAL_MERGE_MESSAGE,
                "case {case:?}: expected the sentence the service answers that class with, \
                 got {message:?}"
            );
            let tip = history.tip_of(DESTINATION).await;
            assert_eq!(
                tip, history.destination_tip,
                "case {case:?}: a refused merge must leave the destination branch alone, \
                 got {tip:?}"
            );
        }

        // And what the PULL REQUEST merge does, on a repository the branch merge has
        // not touched. Two references at one commit are two sides with nothing
        // between them, so no pull request can be opened over them at all — that
        // condition is asserted in `a_pull_request_needs_two_sides_that_differ`.
        if relation.destination_commits == 0 && relation.source_commits == 0 {
            continue;
        }
        let history =
            History::build(relation.destination_commits, relation.source_commits).await;
        let pull_request_id = history.pull_request().await;
        let request = json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": pull_request_id,
        });
        if advances {
            let merged = ok(&history.router, "MergePullRequestByFastForward", &request).await;
            assert_eq!(
                string_at(&merged, "/pullRequest/pullRequestStatus"),
                "CLOSED",
                "case {case:?}: a merged pull request is closed, got {merged}"
            );
            let tip = history.tip_of(DESTINATION).await;
            assert_eq!(
                tip, history.source_tip,
                "case {case:?}: the destination reference must hold the source tip, got {tip:?}"
            );
        } else {
            let (code, message) = error_with_message(
                &history.router,
                "MergePullRequestByFastForward",
                &request,
                CLIENT_ERROR_STATUS,
            )
            .await;
            assert_eq!(
                code, "ManualMergeRequiredException",
                "case {case:?}: expected the modelled refusal of the pull request merge, \
                 got {code:?} saying {message:?}"
            );
            assert_eq!(
                message, MANUAL_MERGE_MESSAGE,
                "case {case:?}: expected the same sentence for the pull request merge, \
                 got {message:?}"
            );
            let read = ok(
                &history.router,
                "GetPullRequest",
                &json!({ "pullRequestId": pull_request_id }),
            )
            .await;
            assert_eq!(
                string_at(&read, "/pullRequest/pullRequestStatus"),
                "OPEN",
                "case {case:?}: a refused merge leaves the pull request open, got {read}"
            );
            let tip = history.tip_of(DESTINATION).await;
            assert_eq!(
                tip, history.destination_tip,
                "case {case:?}: a refused pull request merge must leave the destination \
                 reference alone, got {tip:?}"
            );
        }
    }
}

/// The path both branches of the conflict fixture change differently, which the
/// resolution rows below name.
const CONFLICTING_PATH: &str = "shared.txt";

/// A repository whose two branches changed one path differently, plus the pull
/// request between them: the merges below each need a resolution to go through, so a
/// row refused for its resolution is refused before any of that is evaluated.
struct Conflicting {
    router: MultiServiceRouter,
    destination_tip: String,
    source_tip: String,
    pull_request_id: String,
}

async fn conflicting() -> Conflicting {
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
            "title": "a merge that needs a resolution",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
            "clientRequestToken": fresh_client_request_token(),
        }),
    )
    .await;
    Conflicting {
        router,
        destination_tip,
        source_tip,
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
    }
}

/// A merge request for one of the five operations that take a `conflictResolution`,
/// without the conflict members a row adds.
fn merge_request(operation: &str, fixture: &Conflicting) -> Value {
    match operation {
        "CreateUnreferencedMergeCommit" => json!({
            "repositoryName": REPOSITORY,
            "sourceCommitSpecifier": SOURCE,
            "destinationCommitSpecifier": DESTINATION,
            "mergeOption": "THREE_WAY_MERGE",
        }),
        "MergeBranchesBySquash" | "MergeBranchesByThreeWay" => json!({
            "repositoryName": REPOSITORY,
            "sourceCommitSpecifier": SOURCE,
            "destinationCommitSpecifier": DESTINATION,
            "targetBranch": DESTINATION,
        }),
        "MergePullRequestBySquash" | "MergePullRequestByThreeWay" => json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": fixture.pull_request_id,
        }),
        other => panic!("{other} takes no conflictResolution"),
    }
}

/// The five operations that carry a `conflictResolution` member.
const RESOLVING_OPERATIONS: [&str; 5] = [
    "CreateUnreferencedMergeCommit",
    "MergeBranchesBySquash",
    "MergeBranchesByThreeWay",
    "MergePullRequestBySquash",
    "MergePullRequestByThreeWay",
];

/// `ConflictResolutionStrategyTypeEnum`'s values other than `AUTOMERGE`, each
/// paired with how a row names it: `None` is the request that names none at all,
/// which the model documents as evaluated with `NONE`
/// (`smithy_query(query="shape",shape="ConflictResolutionStrategyTypeEnum")`).
const UNHONOURING_STRATEGIES: [(Option<&str>, &str); 4] = [
    (None, "NONE"),
    (Some("NONE"), "NONE"),
    (Some("ACCEPT_SOURCE"), "ACCEPT_SOURCE"),
    (Some("ACCEPT_DESTINATION"), "ACCEPT_DESTINATION"),
];

/// The `conflictResolution` blocks a row supplies: every list the block carries,
/// and an entry whose `replacementType` is outside the modelled enum — the value
/// the request boundary would refuse on its own if the combination were not judged
/// first (`smithy_query(query="shape",shape="ReplacementTypeEnum")` declares
/// KEEP_BASE, KEEP_SOURCE, KEEP_DESTINATION and USE_NEW_CONTENT).
fn resolutions() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a content replacement keeping the source version",
            json!({
                "replaceContents": [
                    { "filePath": CONFLICTING_PATH, "replacementType": "KEEP_SOURCE" },
                ],
            }),
        ),
        (
            "a content replacement with new content",
            json!({
                "replaceContents": [{
                    "filePath": CONFLICTING_PATH,
                    "replacementType": "USE_NEW_CONTENT",
                    "content": "cmVzb2x2ZWQK",
                }],
            }),
        ),
        (
            "a content replacement whose type the model does not declare",
            json!({
                "replaceContents": [
                    { "filePath": CONFLICTING_PATH, "replacementType": "KEEP_EITHER" },
                ],
            }),
        ),
        (
            "a deletion",
            json!({ "deleteFiles": [{ "filePath": CONFLICTING_PATH }] }),
        ),
        (
            "a file mode change",
            json!({
                "setFileModes": [
                    { "filePath": CONFLICTING_PATH, "fileMode": "EXECUTABLE" },
                ],
            }),
        ),
    ]
}

#[tokio::test]
async fn a_conflict_resolution_is_refused_under_every_strategy_but_automerge() {
    let fixture = conflicting().await;

    for operation in RESOLVING_OPERATIONS {
        for (named, strategy) in UNHONOURING_STRATEGIES {
            for (resolution, block) in resolutions() {
                let case = format!("{operation} with {resolution} under {strategy}");
                let mut body = merge_request(operation, &fixture);
                body["conflictResolution"] = block;
                if let Some(named) = named {
                    body["conflictResolutionStrategy"] = json!(named);
                }
                let (code, message) = error_with_message(
                    &fixture.router,
                    operation,
                    &body,
                    CLIENT_ERROR_STATUS,
                )
                .await;
                assert_eq!(
                    code, "InvalidConflictResolutionException",
                    "case {case:?}: expected the combination refused, got {code:?} saying \
                     {message:?}"
                );
                assert_eq!(
                    message,
                    unhonourable_resolution_message(strategy),
                    "case {case:?}: expected the strategy in effect named, got {message:?}"
                );
            }
        }
    }

    // Under AUTOMERGE the entries ARE read, so each is judged on its own: the one
    // the model refuses is refused for THAT, and a usable one is honoured and the
    // merge goes through.
    let mut refused_entry = merge_request("CreateUnreferencedMergeCommit", &fixture);
    refused_entry["conflictResolutionStrategy"] = json!("AUTOMERGE");
    refused_entry["conflictResolution"] = json!({
        "replaceContents": [{ "filePath": CONFLICTING_PATH, "replacementType": "KEEP_EITHER" }],
    });
    let (code, _) = error_with_message(
        &fixture.router,
        "CreateUnreferencedMergeCommit",
        &refused_entry,
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "InvalidReplacementTypeException",
        "under AUTOMERGE an entry the model refuses is refused for the entry, got {code}"
    );
    let mut honoured = merge_request("CreateUnreferencedMergeCommit", &fixture);
    honoured["conflictResolutionStrategy"] = json!("AUTOMERGE");
    honoured["conflictResolution"] = json!({
        "replaceContents": [{ "filePath": CONFLICTING_PATH, "replacementType": "KEEP_SOURCE" }],
    });
    let written = ok(
        &fixture.router,
        "CreateUnreferencedMergeCommit",
        &honoured,
    )
    .await;
    assert!(
        written.get("commitId").and_then(Value::as_str).is_some(),
        "a resolution AUTOMERGE can honour must write the merge, got {written}"
    );

    // A block that asks for nothing is no resolution at all, so it is not the
    // combination that fails: the merge is evaluated and refused for the conflict
    // the two sides hold.
    let mut empty = merge_request("MergeBranchesByThreeWay", &fixture);
    empty["conflictResolution"] = json!({});
    let (code, _) = error_with_message(
        &fixture.router,
        "MergeBranchesByThreeWay",
        &empty,
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "ManualMergeRequiredException",
        "an empty conflictResolution asks for nothing, so the merge itself is evaluated, \
         got {code}"
    );

    // A value the model refuses OUTSIDE the resolution block is refused for that
    // value: the combination is what the resolution rows are about, not everything
    // a request carrying one may get wrong. The rows differ along WHICH member
    // carries it: one unrelated to the resolution, and the SIBLING member whose
    // name the block's own path is a prefix of — a strategy value the model does
    // not declare is refused for that member, in the sentence the boundary words
    // it with, however the resolution list beside it looks.
    let outside: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a repository name the model refuses",
            "repositoryName",
            json!("not a name"),
            "InvalidRepositoryNameException",
            "The repository name is not valid. Repository names can be any valid combination of \
             letters, numbers, periods, underscores, and dashes between 1 and 100 characters in \
             length. Names are case sensitive. For more information, see Limits in the AWS \
             CodeCommit User Guide.",
        ),
        (
            "a conflict resolution strategy the model does not declare",
            "conflictResolutionStrategy",
            json!("ACCEPT_EITHER"),
            "InvalidConflictResolutionStrategyException",
            "The specified conflict resolution strategy is not valid.",
        ),
    ];
    for (case, member, value, expected_code, expected_message) in outside {
        for operation in RESOLVING_OPERATIONS {
            let mut body = merge_request(operation, &fixture);
            body[member] = value.clone();
            body["conflictResolution"] = json!({
                "replaceContents": [
                    { "filePath": CONFLICTING_PATH, "replacementType": "KEEP_EITHER" },
                ],
            });
            let (code, message) =
                error_with_message(&fixture.router, operation, &body, CLIENT_ERROR_STATUS).await;
            assert_eq!(
                code, expected_code,
                "case {case:?} on {operation}: a refused value outside the resolution block \
                 keeps its own refusal, got {code:?} saying {message:?}"
            );
            assert_eq!(
                message, expected_message,
                "case {case:?} on {operation}: expected the sentence that member is refused \
                 with, got {message:?}"
            );
        }
    }

    // None of the refusals moved a branch, and the pull request is still open.
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

/// A `CreatePullRequest` body naming one target.
fn create_body(repository: &str, source: &str, destination: Option<&str>) -> Value {
    let mut target = json!({
        "repositoryName": repository,
        "sourceReference": source,
    });
    if let Some(destination) = destination {
        target["destinationReference"] = json!(destination);
    }
    json!({
        "title": "a pull request between two sides",
        "targets": [target],
        "clientRequestToken": fresh_client_request_token(),
    })
}

#[tokio::test]
async fn a_pull_request_needs_two_sides_that_differ() {
    // Both branches at the base commit: two references, one commit.
    let history = History::build(0, 0).await;
    let router = &history.router;

    // Rows differ along HOW the two sides fail to be a comparison: the same branch
    // named twice, the same branch spelled two ways, and two differently named
    // branches holding the same tip (by name, by fully qualified reference, and
    // left to the repository's default branch).
    let same: Vec<(&str, Value)> = vec![
        (
            "the same branch on both sides",
            create_body(REPOSITORY, SOURCE, Some(SOURCE)),
        ),
        (
            "the same branch spelled two ways",
            create_body(REPOSITORY, &format!("refs/heads/{DESTINATION}"), Some(DESTINATION)),
        ),
        (
            "two branches whose tips are the same commit",
            create_body(REPOSITORY, SOURCE, Some(DESTINATION)),
        ),
        (
            "the same tips, each side fully qualified",
            create_body(
                REPOSITORY,
                &format!("refs/heads/{SOURCE}"),
                Some(&format!("refs/heads/{DESTINATION}")),
            ),
        ),
        (
            "the same tips, the destination left to the default branch",
            create_body(REPOSITORY, SOURCE, None),
        ),
    ];
    for (case, body) in same {
        let (code, message) =
            error_with_message(router, "CreatePullRequest", &body, CLIENT_ERROR_STATUS).await;
        assert_eq!(
            code, "SourceAndDestinationAreSameException",
            "case {case:?}: expected the two sides refused, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message, SOURCE_AND_DESTINATION_ARE_SAME_MESSAGE,
            "case {case:?}: expected the sentence covering both conditions, got {message:?}"
        );
    }
    let listed = ok(
        router,
        "ListPullRequests",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    assert_eq!(
        listed.pointer("/pullRequestIds").and_then(Value::as_array),
        Some(&Vec::new()),
        "a refused creation must store no pull request, got {listed}"
    );

    // One commit on the source and the same request is a comparison, so it is
    // served: the refusals above are about the two sides, not about the request.
    let moved = History::build(0, 1).await;
    let created = ok(
        &moved.router,
        "CreatePullRequest",
        &create_body(REPOSITORY, SOURCE, Some(DESTINATION)),
    )
    .await;
    assert_eq!(
        string_at(&created, "/pullRequest/pullRequestStatus"),
        "OPEN",
        "two sides that differ must open a pull request, got {created}"
    );

    // A reference the repository does not hold is refused naming THAT specifier, as
    // the request spelled it — on either side, by name and fully qualified.
    let absent: Vec<(&str, Value, &str)> = vec![
        (
            "a source branch the repository does not hold",
            create_body(REPOSITORY, "does-not-exist-branch", Some(DESTINATION)),
            "does-not-exist-branch",
        ),
        (
            "a fully qualified source reference naming nothing",
            create_body(REPOSITORY, "refs/heads/absent-source", Some(DESTINATION)),
            "refs/heads/absent-source",
        ),
        (
            "a destination branch the repository does not hold",
            create_body(REPOSITORY, SOURCE, Some("absent-destination")),
            "absent-destination",
        ),
        (
            "a fully qualified destination reference naming nothing",
            create_body(REPOSITORY, SOURCE, Some("refs/heads/absent-destination")),
            "refs/heads/absent-destination",
        ),
    ];
    for (case, body, specifier) in absent {
        let (code, message) = error_with_message(
            &moved.router,
            "CreatePullRequest",
            &body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "ReferenceDoesNotExistException",
            "case {case:?}: expected the reference refused, got {code:?} saying {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "AWS CodeCommit could not find the commit specifier {specifier}. Either the \
                 specifier does not exist, or the commit has not yet been pushed to the remote \
                 repository."
            ),
            "case {case:?}: expected the specifier named in the message, got {message:?}"
        );
    }

    // A repository with no branch at all has no default branch either, so a request
    // that leaves the destination to it names no specifier the refusal could state:
    // the shape's own sentence is what it answers.
    ok(
        &moved.router,
        "CreateRepository",
        &json!({ "repositoryName": "no-default-branch" }),
    )
    .await;
    let (code, message) = error_with_message(
        &moved.router,
        "CreatePullRequest",
        &create_body("no-default-branch", SOURCE, None),
        CLIENT_ERROR_STATUS,
    )
    .await;
    assert_eq!(
        code, "ReferenceDoesNotExistException",
        "a destination left to a repository with no default branch is refused, got {code:?} \
         saying {message:?}"
    );
    assert_eq!(
        message, REFERENCE_DOES_NOT_EXIST_MESSAGE,
        "with no specifier to name, the shape's own sentence is answered, got {message:?}"
    );

    // Every refusal above stored nothing: the one pull request this test opened is
    // the only one the repository holds.
    let listed = ok(
        &moved.router,
        "ListPullRequests",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    assert_eq!(
        listed
            .pointer("/pullRequestIds")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "a refused creation must store no pull request, got {listed}"
    );
}
