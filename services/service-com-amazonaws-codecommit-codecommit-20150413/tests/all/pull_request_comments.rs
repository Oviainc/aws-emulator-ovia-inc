// SPDX-License-Identifier: CC0-1.0

//! Comments posted on a pull request, over the wire: `PostCommentForPullRequest`
//! with `GetComment`, `GetPullRequest` and `GetCommentsForComparedCommit`, driven
//! as awsJson1_1 requests through the router that serves the real emulator (the
//! wire harness comes from the sibling `repository_lifecycle` suite rather than a
//! second copy).
//!
//! The rule under test: posting a comment on a pull request persists it against
//! that pull request and the two commits the request names, anchored to the blobs
//! of the commented file in each of those commits' stored trees, and answers with
//! the stored record — so a later `GetComment` returns exactly the comment the post
//! returned, the comment is the pull request's rather than the bare comparison's,
//! and a request naming a repository, pull request, commit, path or position the
//! service does not hold persists nothing and is refused with the error the model
//! declares for it.
//!
//! The tables below span what that rule generalises over — content kind and size
//! (ASCII, unicode, multi-line, the documented 10,240-character maximum), whether
//! the comment names a location at all, which version of the file it sits on
//! (`RelativeFileVersionEnum` = BEFORE | AFTER), how the path is spelled, whether a
//! position is named, and whether an idempotency token is sent — rather than one
//! recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState;

use crate::compared_commit_comments::{refused, reported_location};
use crate::repository_lifecycle::{base64, emulator_with_engine, is_uuid, ok, string_at};
use crate::wire::{caller_arn, DEFAULT_ACCESS_KEY_ID};

/// The file the pull request changes, and the two versions of it the destination
/// and the source branch carry. The line counts (2 and 3) are what a comment's
/// `filePosition` is checked against.
const PATH: &str = "README.md";
const BEFORE_CONTENT: &[u8] = b"alpha\nbeta\n";
const AFTER_CONTENT: &[u8] = b"alpha\nbeta\ngamma\n";
const BEFORE_LINES: i64 = 2;
const AFTER_LINES: i64 = 3;

/// The documented comment size bound: "The comment is too large. Comments are
/// limited to 10,240 characters."
/// (`smithy_query(query="shape",shape="CommentContentSizeLimitExceededException")`).
const MAX_COMMENT_CHARACTERS: usize = 10_240;

/// One repository with an open pull request between two branches, each at its own
/// version of [`PATH`]: everything a comment on a pull request is posted against.
struct PullRequest {
    repository: String,
    pull_request_id: String,
    /// The tip of the destination branch — the before side of the pull request.
    before_commit: String,
    /// The tip of the source branch — the after side.
    after_commit: String,
    before_blob: String,
    after_blob: String,
}

/// Build that repository and pull request on an already-running emulator through
/// the implemented write path, so every id below is one the service itself stored.
async fn pull_request_on(router: &MultiServiceRouter, repository: &str) -> PullRequest {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let destination = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "main",
            "filePath": PATH,
            "fileContent": base64(BEFORE_CONTENT),
            "commitMessage": "the destination version",
        }),
    )
    .await;
    let before_commit = string_at(&destination, "/commitId");
    ok(
        router,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": "feature",
            "commitId": before_commit,
        }),
    )
    .await;
    let source = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": "feature",
            "filePath": PATH,
            "fileContent": base64(AFTER_CONTENT),
            "parentCommitId": before_commit,
            "commitMessage": "the source version",
        }),
    )
    .await;
    let created = ok(
        router,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": repository,
                "sourceReference": "feature",
                "destinationReference": "main",
            }],
        }),
    )
    .await;
    PullRequest {
        repository: repository.to_string(),
        pull_request_id: string_at(&created, "/pullRequest/pullRequestId"),
        before_blob: string_at(&destination, "/blobId"),
        after_blob: string_at(&source, "/blobId"),
        before_commit,
        after_commit: string_at(&source, "/commitId"),
    }
}

/// An emulator holding one such pull request, with the engine so a test can read
/// the store the handlers write.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    pr: PullRequest,
}

async fn fixture(repository: &str) -> Fixture {
    let (router, engine) = emulator_with_engine();
    let pr = pull_request_on(&router, repository).await;
    Fixture {
        router,
        engine,
        pr,
    }
}

impl Fixture {
    /// A `PostCommentForPullRequest` body for this pull request, with the members
    /// `overrides` names replaced (a null override removes the member).
    fn post_body(&self, overrides: Value) -> Value {
        let mut body = json!({
            "pullRequestId": self.pr.pull_request_id,
            "repositoryName": self.pr.repository,
            "beforeCommitId": self.pr.before_commit,
            "afterCommitId": self.pr.after_commit,
            "content": "a comment",
        });
        let Value::Object(overrides) = overrides else {
            panic!("overrides must be an object");
        };
        for (member, value) in overrides {
            if value.is_null() {
                body.as_object_mut().expect("an object").remove(&member);
            } else {
                body[member] = value;
            }
        }
        body
    }

    /// How many comments this repository's stored state holds.
    fn stored_comments(&self) -> usize {
        self.engine
            .state::<CodeCommitState>()
            .lock()
            .repositories
            .get(&self.pr.repository)
            .expect("the repository written above")
            .comments
            .len()
    }
}

/// One row of the post table: what is posted, and what the service must report.
struct PostCase {
    label: &'static str,
    content: String,
    /// The `location` as sent; `None` comments on the pull request as a whole.
    sent_location: Option<Value>,
    /// The `location` the service must report (a file path is held in the form a
    /// git tree holds it, with leading separators removed).
    expected_location: Option<Value>,
    /// Does the comment sit on a file? Only then is it anchored to blobs.
    on_file: bool,
    token: Option<&'static str>,
}

/// The table: rows differ along content kind and size, whether a location is named
/// at all, which version of the file the comment sits on, how the path is spelled,
/// whether a position is named, and whether an idempotency token is sent.
fn post_cases() -> Vec<PostCase> {
    vec![
        PostCase {
            label: "whole pull request",
            content: "Looks good to me.".to_string(),
            sent_location: None,
            expected_location: None,
            on_file: false,
            token: None,
        },
        PostCase {
            label: "after version, last line",
            // Line 3 exists in the source version only.
            content: "this line is new".to_string(),
            sent_location: Some(json!({
                "filePath": PATH,
                "filePosition": AFTER_LINES,
                "relativeFileVersion": "AFTER",
            })),
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": AFTER_LINES,
                "relativeFileVersion": "AFTER",
            })),
            on_file: true,
            token: Some("token-after-version"),
        },
        PostCase {
            label: "before version, path with a leading separator",
            content: "⚙ reseña — ¿sí?\nlínea 2\n".to_string(),
            // A leading separator addresses the same file as none at all.
            sent_location: Some(json!({
                "filePath": format!("/{PATH}"),
                "filePosition": BEFORE_LINES,
                "relativeFileVersion": "BEFORE",
            })),
            expected_location: Some(json!({
                "filePath": PATH,
                "filePosition": BEFORE_LINES,
                "relativeFileVersion": "BEFORE",
            })),
            on_file: true,
            token: None,
        },
        PostCase {
            label: "file with no position, defaulted version",
            content: "a comment on the whole file".to_string(),
            sent_location: Some(json!({ "filePath": PATH })),
            expected_location: Some(json!({ "filePath": PATH })),
            on_file: true,
            token: Some("token-whole-file"),
        },
        PostCase {
            label: "maximum content length",
            // The documented maximum: "Comments are limited to 10,240 characters."
            content: "x".repeat(MAX_COMMENT_CHARACTERS),
            sent_location: None,
            expected_location: None,
            on_file: false,
            token: None,
        },
        PostCase {
            label: "location naming nothing",
            content: "an empty location places the comment nowhere".to_string(),
            // Every member of the location absent: the comment is on the pull
            // request as a whole, the same as sending no location at all.
            sent_location: Some(json!({})),
            expected_location: None,
            on_file: false,
            token: None,
        },
    ]
}

#[tokio::test]
async fn a_comment_posted_on_a_pull_request_is_read_back_from_stored_state() {
    let mut comment_ids: BTreeSet<String> = BTreeSet::new();

    for (index, case) in post_cases().into_iter().enumerate() {
        let label = case.label;
        let fixture = fixture(&format!("pull-request-comments-{index}")).await;

        let mut overrides = json!({ "content": case.content });
        if let Some(location) = &case.sent_location {
            overrides["location"] = location.clone();
        }
        if let Some(token) = case.token {
            overrides["clientRequestToken"] = json!(token);
        }
        let posted = ok(
            &fixture.router,
            "PostCommentForPullRequest",
            &fixture.post_body(overrides),
        )
        .await;

        // What the post reported about the pull request and the comparison it
        // filed the comment under.
        for (member, expected) in [
            ("repositoryName", Some(fixture.pr.repository.as_str())),
            ("pullRequestId", Some(fixture.pr.pull_request_id.as_str())),
            ("beforeCommitId", Some(fixture.pr.before_commit.as_str())),
            ("afterCommitId", Some(fixture.pr.after_commit.as_str())),
            (
                "beforeBlobId",
                case.on_file.then_some(fixture.pr.before_blob.as_str()),
            ),
            (
                "afterBlobId",
                case.on_file.then_some(fixture.pr.after_blob.as_str()),
            ),
        ] {
            assert_eq!(
                posted.get(member).and_then(Value::as_str),
                expected,
                "case {label:?}: {member} must be the stored value, in {posted}"
            );
        }
        assert_eq!(
            reported_location(&posted),
            case.expected_location,
            "case {label:?}: location in {posted}"
        );

        let comment = posted
            .get("comment")
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| panic!("case {label:?}: no comment in {posted}"))
            .clone();
        let comment_id = string_at(&comment, "/commentId");
        assert!(
            is_uuid(&comment_id),
            "case {label:?}: commentId must be a uuid, got {comment_id:?}"
        );
        assert!(
            comment_ids.insert(comment_id.clone()),
            "case {label:?}: commentId {comment_id} was already issued to another comment"
        );
        assert_eq!(
            comment.get("content").and_then(Value::as_str),
            Some(case.content.as_str()),
            "case {label:?}: content must be what was posted, in {comment}"
        );
        assert_eq!(
            comment.get("authorArn").and_then(Value::as_str),
            Some(caller_arn(DEFAULT_ACCESS_KEY_ID).as_str()),
            "case {label:?}: authorArn must be the identity that signed the call, in {comment}"
        );
        assert_eq!(
            comment.get("deleted").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: a posted comment is not deleted, in {comment}"
        );
        assert_eq!(
            comment.get("inReplyTo"),
            None,
            "case {label:?}: a comment posted on a pull request replies to nothing, in {comment}"
        );
        assert_eq!(
            comment.get("clientRequestToken").and_then(Value::as_str),
            case.token,
            "case {label:?}: clientRequestToken must be what was sent, in {comment}"
        );
        let creation_date = comment
            .get("creationDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {label:?}: no creationDate in {comment}"));
        assert_eq!(
            comment.get("lastModifiedDate").and_then(Value::as_f64),
            Some(creation_date),
            "case {label:?}: a fresh comment was last modified when it was created"
        );

        // The round trip: the comment the post returned is the comment the
        // service holds, read back through the implemented reader.
        let read = ok(
            &fixture.router,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            read.get("comment"),
            Some(&comment),
            "case {label:?}: GetComment must report the comment the post returned"
        );
        assert_eq!(
            fixture.stored_comments(),
            1,
            "case {label:?}: the post must have stored exactly one comment"
        );
    }
}

#[tokio::test]
async fn a_pull_request_comment_is_the_pull_requests_rather_than_the_bare_comparisons() {
    let fixture = fixture("pull-request-comment-scope").await;

    let posted = ok(
        &fixture.router,
        "PostCommentForPullRequest",
        &fixture.post_body(json!({ "content": "on the pull request" })),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");

    let comparison_request = json!({
        "repositoryName": fixture.pr.repository,
        "beforeCommitId": fixture.pr.before_commit,
        "afterCommitId": fixture.pr.after_commit,
    });

    // The same pair of commits read as a bare comparison carries no comment: this
    // one was posted on the pull request.
    let comparison = ok(
        &fixture.router,
        "GetCommentsForComparedCommit",
        &comparison_request,
    )
    .await;
    assert_eq!(
        comparison
            .get("commentsForComparedCommitData")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a comment posted on a pull request is not a comment on the comparison alone: {comparison}"
    );

    // A comment posted on the comparison itself is reported there, and the two do
    // not mix.
    let on_comparison = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.pr.repository,
            "beforeCommitId": fixture.pr.before_commit,
            "afterCommitId": fixture.pr.after_commit,
            "content": "on the comparison",
        }),
    )
    .await;
    let comparison = ok(
        &fixture.router,
        "GetCommentsForComparedCommit",
        &comparison_request,
    )
    .await;
    let reported: Vec<String> = comparison
        .get("commentsForComparedCommitData")
        .and_then(Value::as_array)
        .expect("commentsForComparedCommitData")
        .iter()
        .map(|entry| string_at(entry, "/comments/0/commentId"))
        .collect();
    assert_eq!(
        reported,
        vec![string_at(&on_comparison, "/comment/commentId")],
        "the comparison reports its own comment only: {comparison}"
    );

    // Both comments are records of the one repository, and the pull request
    // comment is the one filed under the pull request.
    let state = fixture.engine.state::<CodeCommitState>();
    let store = state.lock();
    let repository = store
        .repositories
        .get(&fixture.pr.repository)
        .expect("the repository written above");
    assert_eq!(
        repository.comments.len(),
        2,
        "the store must hold one record per posted comment"
    );
    let record = repository
        .comments
        .get(&comment_id)
        .unwrap_or_else(|| panic!("no stored comment {comment_id}"));
    assert_eq!(
        record.pull_request_id.map(|id| id.to_string()),
        Some(fixture.pr.pull_request_id.clone()),
        "the stored comment must be filed under the pull request it was posted on"
    );
}

#[tokio::test]
async fn posting_a_comment_is_activity_on_the_pull_request() {
    let fixture = fixture("pull-request-comment-activity").await;
    let before = ok(
        &fixture.router,
        "GetPullRequest",
        &json!({ "pullRequestId": fixture.pr.pull_request_id }),
    )
    .await;
    let created_at = before
        .pointer("/pullRequest/creationDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no creationDate in {before}"));
    assert_eq!(
        before
            .pointer("/pullRequest/lastActivityDate")
            .and_then(Value::as_f64),
        Some(created_at),
        "case \"before the comment\": a pull request nothing has touched was last active when it \
         was created: {before}"
    );

    // Far enough apart that the two instants are distinguishable.
    tokio::time::sleep(Duration::from_millis(20)).await;
    ok(
        &fixture.router,
        "PostCommentForPullRequest",
        &fixture.post_body(json!({ "content": "activity on the pull request" })),
    )
    .await;

    let after = ok(
        &fixture.router,
        "GetPullRequest",
        &json!({ "pullRequestId": fixture.pr.pull_request_id }),
    )
    .await;
    let last_activity = after
        .pointer("/pullRequest/lastActivityDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no lastActivityDate in {after}"));
    assert!(
        last_activity > created_at,
        "case \"after the comment\": lastActivityDate must move to the comment's instant, got \
         {last_activity} against a creationDate of {created_at}"
    );
    assert_eq!(
        after.pointer("/pullRequest/creationDate"),
        before.pointer("/pullRequest/creationDate"),
        "a comment does not change when the pull request was created: {after}"
    );
    assert_eq!(
        after.pointer("/pullRequest/revisionId"),
        before.pointer("/pullRequest/revisionId"),
        "a comment does not move the reviewable state of the pull request: {after}"
    );
}

#[tokio::test]
async fn a_repeated_client_request_token_posts_one_comment() {
    let fixture = fixture("pull-request-comment-idempotent").await;
    let request = fixture.post_body(json!({
        "content": "posted once",
        "clientRequestToken": "token-repeated",
    }));

    let first = ok(&fixture.router, "PostCommentForPullRequest", &request).await;
    let again = ok(&fixture.router, "PostCommentForPullRequest", &request).await;
    assert_eq!(
        again, first,
        "the same request with the same token must return the first comment"
    );

    // A changed parameter under the same token is refused rather than posting a
    // second comment.
    let swapped_comparison = {
        let mut changed = request.clone();
        // The two sides of the comparison, the other way around: a different
        // comparison, and still two different commits.
        changed["beforeCommitId"] = json!(fixture.pr.after_commit.clone());
        changed["afterCommitId"] = json!(fixture.pr.before_commit.clone());
        changed
    };
    for (case, changed) in [
        ("content", {
            let mut changed = request.clone();
            changed["content"] = json!("posted twice");
            changed
        }),
        ("location", {
            let mut changed = request.clone();
            changed["location"] = json!({ "filePath": PATH });
            changed
        }),
        ("comparison", swapped_comparison),
    ] {
        let code = refused(&fixture.router, case, "PostCommentForPullRequest", &changed).await;
        assert_eq!(
            code, "IdempotencyParameterMismatchException",
            "case {case:?}: a reused token with a changed parameter must be refused"
        );
    }

    assert_eq!(
        fixture.stored_comments(),
        1,
        "the repeated post must have stored one comment, and the refused ones none"
    );
}

#[tokio::test]
async fn a_comment_the_pull_request_cannot_carry_is_refused_with_the_modeled_error() {
    let fixture = fixture("pull-request-comment-refusals").await;
    // A second repository on the SAME emulator, with its own pull request, so a
    // pull request that exists but belongs elsewhere can be told from one that
    // exists nowhere.
    let elsewhere = pull_request_on(&fixture.router, "pull-request-comment-elsewhere").await;
    // A well-formed object name no commit of this repository has, and a decimal id
    // no pull request was minted as.
    let unknown_commit = "0".repeat(40);
    let unknown_pull_request = "999999";

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "unknown repository",
            fixture.post_body(json!({ "repositoryName": "no-such-repository" })),
            "RepositoryDoesNotExistException",
        ),
        (
            "pull request of another repository",
            fixture.post_body(json!({ "pullRequestId": elsewhere.pull_request_id.clone() })),
            "RepositoryNotAssociatedWithPullRequestException",
        ),
        (
            "unknown pull request",
            fixture.post_body(json!({ "pullRequestId": unknown_pull_request })),
            "PullRequestDoesNotExistException",
        ),
        (
            "blank pull request id",
            fixture.post_body(json!({ "pullRequestId": "" })),
            "PullRequestIdRequiredException",
        ),
        (
            "malformed pull request id",
            fixture.post_body(json!({ "pullRequestId": "not-a-pull-request" })),
            "InvalidPullRequestIdException",
        ),
        (
            "blank after commit id",
            fixture.post_body(json!({ "afterCommitId": "" })),
            "CommitIdRequiredException",
        ),
        (
            "malformed after commit id",
            fixture.post_body(json!({ "afterCommitId": "not-a-commit-id" })),
            "InvalidCommitIdException",
        ),
        (
            "unknown before commit",
            fixture.post_body(json!({ "beforeCommitId": unknown_commit.clone() })),
            "CommitDoesNotExistException",
        ),
        (
            "unknown after commit",
            fixture.post_body(json!({ "afterCommitId": unknown_commit.clone() })),
            "CommitDoesNotExistException",
        ),
        (
            "same before and after commit",
            fixture.post_body(json!({ "beforeCommitId": fixture.pr.after_commit.clone() })),
            "BeforeCommitIdAndAfterCommitIdAreSameException",
        ),
        (
            "empty content",
            fixture.post_body(json!({ "content": "" })),
            "CommentContentRequiredException",
        ),
        (
            "content one character past the limit",
            fixture.post_body(json!({ "content": "x".repeat(MAX_COMMENT_CHARACTERS + 1) })),
            "CommentContentSizeLimitExceededException",
        ),
        (
            "path no version of the comparison holds",
            fixture.post_body(json!({ "location": { "filePath": "docs/absent.md" } })),
            "PathDoesNotExistException",
        ),
        (
            "position past the end of the commented version",
            // The before version has two lines; the after version has three.
            fixture.post_body(json!({
                "location": {
                    "filePath": PATH,
                    "filePosition": AFTER_LINES,
                    "relativeFileVersion": "BEFORE",
                }
            })),
            "InvalidFilePositionException",
        ),
        (
            "position below the first line",
            fixture.post_body(json!({
                "location": {
                    "filePath": PATH,
                    "filePosition": 0,
                    "relativeFileVersion": "AFTER",
                }
            })),
            "InvalidFilePositionException",
        ),
        (
            "location naming no file",
            fixture.post_body(json!({ "location": { "filePosition": 1 } })),
            "InvalidFileLocationException",
        ),
    ];
    for (case, body, expected) in rows {
        let code = refused(&fixture.router, case, "PostCommentForPullRequest", &body).await;
        assert_eq!(code, expected, "case {case:?}: PostCommentForPullRequest");
    }

    assert_eq!(
        fixture.stored_comments(),
        0,
        "a refused post must store nothing"
    );
}
