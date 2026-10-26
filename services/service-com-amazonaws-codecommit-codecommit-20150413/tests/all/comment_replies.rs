// SPDX-License-Identifier: CC0-1.0

//! Replies to a comment over the wire: `PostCommentReply` with `GetComment` and
//! `GetCommentsForComparedCommit`, driven as awsJson1_1 requests through the router
//! that serves the real emulator (the wire harness and the pull request fixture
//! come from the sibling suites rather than a second copy).
//!
//! The rule under test: a reply is stored as a comment of the THREAD it is posted
//! into — the repository, the comparison with its blob ids, the place in that
//! comparison and the pull request (where there is one) of the comment `inReplyTo`
//! names — carrying its own generated id, the caller as its author and `inReplyTo`
//! naming that comment, and the call answers with exactly that stored record; a
//! request naming no stored comment, or content or an id outside what the service
//! accepts, persists nothing and is refused with the error the model declares for
//! it.
//!
//! The table spans what the rule generalises over: what kind of comment is replied
//! to (one on a bare comparison, one anchored to a line of a file in it, one on a
//! pull request, and a reply — so the thread is three deep), the content's kind and
//! size (ASCII, unicode and multi-line, the documented 10,240-character maximum),
//! whether an idempotency token is sent, and which identity signs the reply.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codecommit_codecommit_20150413::state::{
    location_key, CommentRecord, ComAmazonawsCodecommitCodecommit20150413State as CodeCommitState,
    CLIENT_REQUEST_TOKEN_MAX_LENGTH, MAX_COMMENT_CHARACTERS,
};

use crate::comment_lifecycle::OTHER_ACCESS_KEY_ID;
use crate::compared_commit_comments::refused;
use crate::pull_request_comments::{pull_request_on, PullRequest, AFTER_LINES, PATH};
use crate::repository_lifecycle::{emulator_with_engine, is_uuid, ok, string_at};
use crate::wire::{caller_arn, ok_as, DEFAULT_ACCESS_KEY_ID};

/// A comment id of the form the service mints ids in that no comment was minted
/// as: what a reply to a comment the service does not hold names.
const UNMINTED_COMMENT_ID: &str = "00000000-0000-8000-8000-000000000000";

/// What kind of comment the row's reply replies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parent {
    /// A comment on the bare comparison between the two commits.
    Comparison,
    /// The same, anchored to a line of the commented file — so the thread carries
    /// the blob ids of that file in both sides of the comparison.
    ComparisonOnFile,
    /// A comment on the pull request between the two branches.
    PullRequest,
    /// A reply to a comment on the comparison, so the reply under test is the
    /// third level of the thread.
    Reply,
}

/// The thread a row replies into: the comment that roots it, the comment the reply
/// names, and how many comments the thread already holds.
struct Thread {
    root: String,
    parent: String,
    depth: usize,
}

/// Post the comment the row replies to onto an already-built pull request, through
/// the implemented write paths, so every id below is one the service stored.
async fn thread_to_reply_into(
    router: &MultiServiceRouter,
    pr: &PullRequest,
    parent: Parent,
) -> Thread {
    let on_comparison = |location: Option<Value>| {
        let mut body = json!({
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "content": "the comment being replied to",
        });
        if let Some(location) = location {
            body["location"] = location;
        }
        body
    };
    match parent {
        Parent::Comparison | Parent::Reply => {
            let root = ok(
                router,
                "PostCommentForComparedCommit",
                &on_comparison(None),
            )
            .await;
            let root_id = string_at(&root, "/comment/commentId");
            if parent == Parent::Reply {
                let first_reply = ok(
                    router,
                    "PostCommentReply",
                    &json!({ "inReplyTo": root_id, "content": "the first reply" }),
                )
                .await;
                Thread {
                    parent: string_at(&first_reply, "/comment/commentId"),
                    root: root_id,
                    depth: 2,
                }
            } else {
                Thread {
                    parent: root_id.clone(),
                    root: root_id,
                    depth: 1,
                }
            }
        }
        Parent::ComparisonOnFile => {
            let root = ok(
                router,
                "PostCommentForComparedCommit",
                &on_comparison(Some(json!({
                    "filePath": PATH,
                    "filePosition": AFTER_LINES,
                    "relativeFileVersion": "AFTER",
                }))),
            )
            .await;
            let root_id = string_at(&root, "/comment/commentId");
            Thread {
                parent: root_id.clone(),
                root: root_id,
                depth: 1,
            }
        }
        Parent::PullRequest => {
            let root = ok(
                router,
                "PostCommentForPullRequest",
                &json!({
                    "repositoryName": pr.repository,
                    "pullRequestId": pr.pull_request_id,
                    "beforeCommitId": pr.before_commit,
                    "afterCommitId": pr.after_commit,
                    "content": "the pull request comment being replied to",
                }),
            )
            .await;
            let root_id = string_at(&root, "/comment/commentId");
            Thread {
                parent: root_id.clone(),
                root: root_id,
                depth: 1,
            }
        }
    }
}

/// The record this repository holds for `comment_id`, as the handlers wrote it.
fn stored_comment(
    engine: &Arc<engine::InMemoryEngine>,
    repository: &str,
    comment_id: &str,
) -> CommentRecord {
    engine
        .state::<CodeCommitState>()
        .lock()
        .repositories
        .get(repository)
        .unwrap_or_else(|| panic!("no stored repository {repository}"))
        .comments
        .get(comment_id)
        .unwrap_or_else(|| panic!("no stored comment {comment_id}"))
        .clone()
}

/// How many comments this repository holds.
fn stored_comments(engine: &Arc<engine::InMemoryEngine>, repository: &str) -> usize {
    engine
        .state::<CodeCommitState>()
        .lock()
        .repositories
        .get(repository)
        .unwrap_or_else(|| panic!("no stored repository {repository}"))
        .comments
        .len()
}

/// A stored location as a comparable key: the modeled shape derives no `PartialEq`,
/// so the service's own key function is what two of them are compared through.
fn stored_location(record: &CommentRecord) -> (Option<&str>, Option<i64>, Option<&str>) {
    location_key(record.location.as_ref())
}

/// One row of the reply table.
struct Case {
    label: &'static str,
    parent: Parent,
    content: String,
    token: Option<&'static str>,
    /// The identity that signs the reply, which is its author.
    caller: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a comment on the bare comparison",
            parent: Parent::Comparison,
            content: "Agreed, let us ship it.".to_string(),
            token: None,
            caller: DEFAULT_ACCESS_KEY_ID,
        },
        Case {
            label: "a comment anchored to a line of a file",
            parent: Parent::ComparisonOnFile,
            content: "⚙ respuesta — ¿y la línea anterior?\nsegunda línea\n".to_string(),
            token: Some("token-reply-on-file"),
            caller: DEFAULT_ACCESS_KEY_ID,
        },
        Case {
            label: "a comment on a pull request, answered by another caller",
            parent: Parent::PullRequest,
            // The documented maximum: "Comments are limited to 10,240 characters."
            content: "y".repeat(MAX_COMMENT_CHARACTERS),
            token: None,
            caller: OTHER_ACCESS_KEY_ID,
        },
        Case {
            label: "a reply, so the thread is three deep",
            parent: Parent::Reply,
            content: "and one more thing".to_string(),
            token: Some("token-reply-to-a-reply"),
            caller: DEFAULT_ACCESS_KEY_ID,
        },
    ]
}

#[tokio::test]
async fn a_reply_is_stored_in_the_thread_of_the_comment_it_replies_to() {
    let mut comment_ids: BTreeSet<String> = BTreeSet::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let (router, engine) = emulator_with_engine();
        let pr = pull_request_on(&router, &format!("comment-replies-{index}")).await;
        let thread = thread_to_reply_into(&router, &pr, case.parent).await;

        let mut request = json!({
            "inReplyTo": thread.parent,
            "content": case.content,
        });
        if let Some(token) = case.token {
            request["clientRequestToken"] = json!(token);
        }
        let posted = ok_as(&router, case.caller, "PostCommentReply", &request).await;

        let comment = posted
            .get("comment")
            .filter(|value| !value.is_null())
            .unwrap_or_else(|| panic!("case {label:?}: no comment in {posted}"))
            .clone();
        let reply_id = string_at(&comment, "/commentId");
        assert!(
            is_uuid(&reply_id),
            "case {label:?}: commentId must be a uuid, got {reply_id:?}"
        );
        assert!(
            comment_ids.insert(reply_id.clone()),
            "case {label:?}: commentId {reply_id} was already issued to another comment"
        );
        assert_eq!(
            comment.get("inReplyTo").and_then(Value::as_str),
            Some(thread.parent.as_str()),
            "case {label:?}: inReplyTo must name the comment replied to, in {comment}"
        );
        assert_eq!(
            comment.get("content").and_then(Value::as_str),
            Some(case.content.as_str()),
            "case {label:?}: content must be what was posted, in {comment}"
        );
        assert_eq!(
            comment.get("authorArn").and_then(Value::as_str),
            Some(caller_arn(case.caller).as_str()),
            "case {label:?}: authorArn must be the identity that signed the reply, in {comment}"
        );
        assert_eq!(
            comment.get("deleted").and_then(Value::as_bool),
            Some(false),
            "case {label:?}: a posted reply is not deleted, in {comment}"
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
            "case {label:?}: a fresh reply was last modified when it was created, in {comment}"
        );

        // The round trip: the reply the post returned is the comment the service
        // holds, read back through the implemented reader.
        let read = ok(&router, "GetComment", &json!({ "commentId": reply_id })).await;
        assert_eq!(
            read.get("comment").and_then(|comment| comment.get("content")),
            comment.get("content"),
            "case {label:?}: GetComment must report the reply the post returned, got {read}"
        );
        assert_eq!(
            read.pointer("/comment/inReplyTo").and_then(Value::as_str),
            Some(thread.parent.as_str()),
            "case {label:?}: the stored reply must name the comment replied to, got {read}"
        );

        // What the reply INHERITED from the comment it replies to: the comparison
        // with its blob ids, the place in that comparison, and the pull request
        // the thread was posted on where there is one. The request named none of
        // them.
        let parent = stored_comment(&engine, &pr.repository, &thread.parent);
        let reply = stored_comment(&engine, &pr.repository, &reply_id);
        assert_eq!(
            reply.comparison, parent.comparison,
            "case {label:?}: the reply must be filed under the comparison of the comment it \
             replies to, got {:?} against {:?}",
            reply.comparison, parent.comparison
        );
        assert_eq!(
            reply.pull_request_id, parent.pull_request_id,
            "case {label:?}: the reply must be filed under the pull request of the comment it \
             replies to, got {:?} against {:?}",
            reply.pull_request_id, parent.pull_request_id
        );
        assert_eq!(
            stored_location(&reply),
            stored_location(&parent),
            "case {label:?}: the reply must sit where the comment it replies to sits"
        );
        assert!(
            reply.sequence > parent.sequence,
            "case {label:?}: the reply must be posted after the comment it replies to, got {} \
             against {}",
            reply.sequence,
            parent.sequence
        );
        // A thread anchored to a file carries the blob ids of that file in both
        // sides of the comparison, and the reply is anchored to the same pair.
        let anchored_to_a_file = case.parent == Parent::ComparisonOnFile;
        assert_eq!(
            reply.comparison.before_blob_id.as_deref(),
            anchored_to_a_file.then_some(pr.before_blob.as_str()),
            "case {label:?}: beforeBlobId of the stored reply"
        );
        assert_eq!(
            reply.comparison.after_blob_id.as_deref(),
            anchored_to_a_file.then_some(pr.after_blob.as_str()),
            "case {label:?}: afterBlobId of the stored reply"
        );
        assert_eq!(
            stored_comments(&engine, &pr.repository),
            thread.depth + 1,
            "case {label:?}: the reply must be stored beside the thread it joined"
        );

        // How the thread reads back: a reply is reported inside the thread of the
        // comment it replies to, never as a thread of its own — and a thread posted
        // on a PULL REQUEST is the pull request's rather than the bare comparison's,
        // so it is reported nowhere here.
        let threads = ok(
            &router,
            "GetCommentsForComparedCommit",
            &json!({
                "repositoryName": pr.repository,
                "beforeCommitId": pr.before_commit,
                "afterCommitId": pr.after_commit,
            }),
        )
        .await;
        let entries = threads
            .get("commentsForComparedCommitData")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no data array in {threads}"));
        if case.parent == Parent::PullRequest {
            assert_eq!(
                entries.len(),
                0,
                "case {label:?}: a reply on a pull request thread is not a comment on the \
                 comparison alone, got {threads}"
            );
        } else {
            assert_eq!(
                entries.len(),
                1,
                "case {label:?}: the thread must be reported once, got {threads}"
            );
            let reported: Vec<String> = entries[0]
                .get("comments")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {label:?}: no comments array in {threads}"))
                .iter()
                .map(|comment| string_at(comment, "/commentId"))
                .collect();
            assert_eq!(
                reported.len(),
                thread.depth + 1,
                "case {label:?}: the thread must report every comment posted into it, got \
                 {reported:?}"
            );
            assert_eq!(
                reported.first().map(String::as_str),
                Some(thread.root.as_str()),
                "case {label:?}: the thread is rooted at the comment it started with, got \
                 {reported:?}"
            );
            assert_eq!(
                reported.last().map(String::as_str),
                Some(reply_id.as_str()),
                "case {label:?}: the reply is reported last, in post order, got {reported:?}"
            );
        }
    }
}

/// A reply to a comment whose CONTENT was deleted is stored like any other: the
/// operation models no refusal for it, and the thread stays answerable.
#[tokio::test]
async fn a_comment_whose_content_was_deleted_can_still_be_replied_to() {
    let (router, engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "comment-replies-to-a-deleted-comment").await;
    let thread = thread_to_reply_into(&router, &pr, Parent::Comparison).await;
    ok(
        &router,
        "DeleteCommentContent",
        &json!({ "commentId": thread.parent }),
    )
    .await;

    let posted = ok(
        &router,
        "PostCommentReply",
        &json!({ "inReplyTo": thread.parent, "content": "a reply to an emptied comment" }),
    )
    .await;
    let reply_id = string_at(&posted, "/comment/commentId");
    let reply = stored_comment(&engine, &pr.repository, &reply_id);
    assert_eq!(
        reply.in_reply_to.as_deref(),
        Some(thread.parent.as_str()),
        "case \"deleted parent\": the reply must still name the comment it replies to"
    );
    assert!(
        !reply.deleted,
        "case \"deleted parent\": the reply itself carries content"
    );
}

#[tokio::test]
async fn replying_to_a_pull_request_comment_is_activity_on_the_pull_request() {
    let (router, _engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "comment-replies-activity").await;
    let thread = thread_to_reply_into(&router, &pr, Parent::PullRequest).await;

    let before = ok(
        &router,
        "GetPullRequest",
        &json!({ "pullRequestId": pr.pull_request_id }),
    )
    .await;
    let activity_before = before
        .pointer("/pullRequest/lastActivityDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no lastActivityDate in {before}"));

    // Far enough apart that the two instants are distinguishable.
    tokio::time::sleep(Duration::from_millis(20)).await;
    ok(
        &router,
        "PostCommentReply",
        &json!({ "inReplyTo": thread.parent, "content": "activity on the pull request" }),
    )
    .await;

    let after = ok(
        &router,
        "GetPullRequest",
        &json!({ "pullRequestId": pr.pull_request_id }),
    )
    .await;
    let activity_after = after
        .pointer("/pullRequest/lastActivityDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no lastActivityDate in {after}"));
    assert!(
        activity_after > activity_before,
        "case \"after the reply\": lastActivityDate must move to the reply's instant, got \
         {activity_after} against {activity_before}"
    );
    assert_eq!(
        after.pointer("/pullRequest/revisionId"),
        before.pointer("/pullRequest/revisionId"),
        "a reply does not move the reviewable state of the pull request: {after}"
    );
}

#[tokio::test]
async fn a_repeated_client_request_token_posts_one_reply() {
    let (router, engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "comment-replies-idempotent").await;
    let thread = thread_to_reply_into(&router, &pr, Parent::Comparison).await;
    let request = json!({
        "inReplyTo": thread.parent,
        "content": "replied once",
        "clientRequestToken": "token-repeated",
    });

    let first = ok(&router, "PostCommentReply", &request).await;
    let again = ok(&router, "PostCommentReply", &request).await;
    assert_eq!(
        again, first,
        "the same request with the same token must return the first reply"
    );

    // A changed parameter under the same token is refused rather than posting a
    // second reply: the content, and the comment replied to.
    let second_root = ok(
        &router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": pr.repository,
            "beforeCommitId": pr.before_commit,
            "afterCommitId": pr.after_commit,
            "content": "another thread",
        }),
    )
    .await;
    for (case, changed) in [
        ("content", {
            let mut changed = request.clone();
            changed["content"] = json!("replied twice");
            changed
        }),
        ("inReplyTo", {
            let mut changed = request.clone();
            changed["inReplyTo"] = json!(string_at(&second_root, "/comment/commentId"));
            changed
        }),
    ] {
        let code = refused(&router, case, "PostCommentReply", &changed).await;
        assert_eq!(
            code, "IdempotencyParameterMismatchException",
            "case {case:?}: a reused token with a changed parameter must be refused"
        );
    }

    assert_eq!(
        stored_comments(&engine, &pr.repository),
        3,
        "the repeated reply must have stored one comment beside the two roots, and the refused \
         ones none"
    );
}

#[tokio::test]
async fn a_reply_the_service_cannot_file_is_refused_with_the_modeled_error() {
    let (router, engine) = emulator_with_engine();
    let pr = pull_request_on(&router, "comment-replies-refusals").await;
    let thread = thread_to_reply_into(&router, &pr, Parent::Comparison).await;
    let reply = |overrides: Value| {
        let mut body = json!({
            "inReplyTo": thread.parent,
            "content": "a reply",
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
    };

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a comment id no comment was minted as",
            reply(json!({ "inReplyTo": UNMINTED_COMMENT_ID })),
            "CommentDoesNotExistException",
        ),
        (
            "a blank comment id",
            reply(json!({ "inReplyTo": "" })),
            "CommentIdRequiredException",
        ),
        (
            "no comment id at all",
            reply(json!({ "inReplyTo": null })),
            "CommentIdRequiredException",
        ),
        (
            "a comment id that is not of the form the service mints",
            reply(json!({ "inReplyTo": "not-a-comment-id" })),
            "InvalidCommentIdException",
        ),
        (
            "empty content",
            reply(json!({ "content": "" })),
            "CommentContentRequiredException",
        ),
        (
            "no content at all",
            reply(json!({ "content": null })),
            "CommentContentRequiredException",
        ),
        (
            "content one character past the limit",
            reply(json!({ "content": "z".repeat(MAX_COMMENT_CHARACTERS + 1) })),
            "CommentContentSizeLimitExceededException",
        ),
        (
            "an empty idempotency token",
            reply(json!({ "clientRequestToken": "" })),
            "InvalidClientRequestTokenException",
        ),
        (
            // The documented maximum a token carries, stated once in the service's
            // own `CLIENT_REQUEST_TOKEN_MAX_LENGTH`.
            "an idempotency token one character past the limit",
            reply(json!({ "clientRequestToken": "t".repeat(CLIENT_REQUEST_TOKEN_MAX_LENGTH + 1) })),
            "InvalidClientRequestTokenException",
        ),
    ];
    for (case, body, expected) in rows {
        let code = refused(&router, case, "PostCommentReply", &body).await;
        assert_eq!(code, expected, "case {case:?}: PostCommentReply");
    }

    assert_eq!(
        stored_comments(&engine, &pr.repository),
        1,
        "a refused reply must store nothing beside the comment it would have replied to"
    );
}
