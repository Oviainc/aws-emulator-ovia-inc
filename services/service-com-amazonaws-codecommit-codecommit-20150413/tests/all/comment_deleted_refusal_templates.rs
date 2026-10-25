// SPDX-License-Identifier: CC0-1.0

//! WHICH of `CommentDeletedException`'s templates each operation answers with,
//! over the wire.
//!
//! The rule under test: a modelled exception's message belongs to the call that
//! raised it, not to the exception — where the service is recorded wording one
//! operation's refusal differently from another's, each operation answers its own
//! fixed template rather than one string shared by every raiser. For
//! `CommentDeletedException` there are two: the sentence pair "The specified
//! comment has already been marked as deleted." / "Verify that you specified a
//! valid comment ID, and then try again.", and that pair led by "You cannot delete
//! this comment." for the calls that ask to EDIT OR DELETE the comment whose
//! content is already gone (an `UpdateComment`, a repeated
//! `DeleteCommentContent`) — the two actions the exception's own modelled sentence
//! names. So an editing client and a reading client see DIFFERENT text, both as a
//! 400 `CommentDeletedException`.
//!
//! `comment_deleted_refusals.rs` spans the comments, principals and operations
//! this refusal generalises over and asserts the whole message against the
//! expectation helper both suites share. What is asserted HERE is only what a
//! single shared string would also satisfy there: that the template is chosen by
//! the CALL (the lead clause is present exactly for the editing calls), that the
//! two templates are therefore UNEQUAL across the groups and equal within them,
//! and that the choice is fixed rather than derived from the request — which is
//! why the second test varies the content the refused edit carries (ASCII,
//! unicode, multi-line, the documented 10,240-character maximum).

use serde_json::{json, Value};

use crate::comment_lifecycle::{fixture, Fixture, MAX_COMMENT_CHARACTERS, OTHER_ACCESS_KEY_ID};
use crate::repository_lifecycle::string_at;
use crate::wire::{ok_as, refusal_as, DEFAULT_ACCESS_KEY_ID};

/// The clause that leads the sentence pair where the refused call asked to edit or
/// delete the dead comment, with the TWO spaces the recording separates sentences
/// by.
const LEAD_CLAUSE: &str = "You cannot delete this comment.  ";

/// The sentence pair itself, which every template ends with whichever call was
/// refused.
const ALREADY_DELETED: &str = "The specified comment has already been marked as deleted.  Verify \
                               that you specified a valid comment ID, and then try again.";

/// One refused call: the operation, the members it needs beyond `commentId`, and
/// whether the service is recorded leading its message with [`LEAD_CLAUSE`] —
/// true exactly for the calls that ask to edit or delete the comment itself.
struct Refused {
    operation: &'static str,
    extra: Value,
    leads_with_clause: bool,
}

/// Every call that names a comment by id, with the template it is recorded
/// answering with: the two that ask to edit or delete the comment lead their
/// message with the clause; the two that only report on it, or on a reaction
/// hanging off it, do not.
fn refused_calls() -> Vec<Refused> {
    vec![
        Refused {
            operation: "GetComment",
            extra: json!({}),
            leads_with_clause: false,
        },
        Refused {
            operation: "PutCommentReaction",
            extra: json!({ "reactionValue": ":thumbsup:" }),
            leads_with_clause: false,
        },
        Refused {
            operation: "UpdateComment",
            extra: json!({ "content": "after the deletion" }),
            leads_with_clause: true,
        },
        Refused {
            operation: "DeleteCommentContent",
            extra: json!({}),
            leads_with_clause: true,
        },
    ]
}

/// The contents a refused edit carries, so the template is shown not to depend on
/// the edit's own text: ASCII, unicode, multi-line, and the documented maximum
/// size (`MAX_COMMENT_CHARACTERS`).
fn edit_contents() -> Vec<(&'static str, String)> {
    vec![
        ("a short ASCII edit", "after the deletion".to_string()),
        ("a unicode edit", "✅ ya está — ¿lo revisamos?".to_string()),
        ("a multi-line edit", "first\nsecond\nthird".to_string()),
        (
            "the longest edit the service accepts",
            "e".repeat(MAX_COMMENT_CHARACTERS),
        ),
    ]
}

/// Post a comment on the fixture's comparison, delete its content, and report the
/// id every refusal below is about.
async fn emptied_comment(fixture: &Fixture, content: &str) -> String {
    let posted = ok_as(
        &fixture.router,
        DEFAULT_ACCESS_KEY_ID,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "beforeCommitId": fixture.before_commit,
            "afterCommitId": fixture.after_commit,
            "content": content,
        }),
    )
    .await;
    let comment_id = string_at(&posted, "/comment/commentId");
    ok_as(
        &fixture.router,
        DEFAULT_ACCESS_KEY_ID,
        "DeleteCommentContent",
        &json!({ "commentId": comment_id }),
    )
    .await;
    comment_id
}

/// The request `refused` sends about `comment_id`.
fn body(refused: &Refused, comment_id: &str) -> Value {
    let mut input = json!({ "commentId": comment_id });
    for (member, value) in refused.extra.as_object().expect("each extra is an object") {
        input[member.as_str()] = value.clone();
    }
    input
}

#[tokio::test]
async fn the_editing_and_the_reporting_refusals_are_not_one_message() {
    let fixture = fixture("comment-deleted-template-repo").await;

    // Both principals: the template is the call's, so neither authorship nor who
    // is asking selects it.
    for caller in [DEFAULT_ACCESS_KEY_ID, OTHER_ACCESS_KEY_ID] {
        let comment_id = emptied_comment(&fixture, "the comment under test").await;
        let mut answers: Vec<(&str, bool, String)> = Vec::new();

        for refused in refused_calls() {
            let operation = refused.operation;
            let (code, message) = refusal_as(
                &fixture.router,
                caller,
                operation,
                &body(&refused, &comment_id),
                400,
            )
            .await;
            assert_eq!(
                code, "CommentDeletedException",
                "case ({operation}, {caller}): expected CommentDeletedException, got {code:?}"
            );
            assert!(
                message.ends_with(ALREADY_DELETED),
                "case ({operation}, {caller}): expected a message ending with the exception's \
                 sentence pair, got {message:?}"
            );
            assert_eq!(
                message.starts_with(LEAD_CLAUSE),
                refused.leads_with_clause,
                "case ({operation}, {caller}): expected leads_with_clause={}, got {message:?}",
                refused.leads_with_clause
            );
            answers.push((operation, refused.leads_with_clause, message));
        }

        // The two templates are not one string: a client that tells the refusals
        // apart by their text sees the editing calls and the reporting calls
        // differ, while the calls within each group agree.
        for (operation, leads, message) in &answers {
            for (other, other_leads, other_message) in &answers {
                let same_template = leads == other_leads;
                assert_eq!(
                    message == other_message,
                    same_template,
                    "case ({operation} vs {other}, {caller}): expected the messages to be \
                     {}equal, got {message:?} and {other_message:?}",
                    if same_template { "" } else { "un" }
                );
            }
        }
    }
}

#[tokio::test]
async fn the_edit_refusal_does_not_depend_on_the_content_the_edit_carries() {
    let fixture = fixture("comment-deleted-edit-content-repo").await;

    for (label, content) in edit_contents() {
        let comment_id = emptied_comment(&fixture, "a comment to be emptied").await;

        // Whichever principal asks, and whatever the edit says, the refusal is
        // the editing template.
        for caller in [DEFAULT_ACCESS_KEY_ID, OTHER_ACCESS_KEY_ID] {
            let (code, message) = refusal_as(
                &fixture.router,
                caller,
                "UpdateComment",
                &json!({ "commentId": comment_id, "content": content }),
                400,
            )
            .await;
            assert_eq!(
                code, "CommentDeletedException",
                "case ({label}, {caller}): expected CommentDeletedException, got {code:?}"
            );
            assert_eq!(
                message,
                format!("{LEAD_CLAUSE}{ALREADY_DELETED}"),
                "case ({label}, {caller}): unexpected message {message:?}"
            );
            assert!(
                !message.contains(content.as_str()),
                "case ({label}, {caller}): expected a message carrying none of the edit's own \
                 content, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_edit_is_accepted_until_the_content_is_deleted() {
    let fixture = fixture("comment-deleted-template-live-repo").await;

    for (label, content) in edit_contents() {
        let posted = ok_as(
            &fixture.router,
            DEFAULT_ACCESS_KEY_ID,
            "PostCommentForComparedCommit",
            &json!({
                "repositoryName": fixture.repository,
                "beforeCommitId": fixture.before_commit,
                "afterCommitId": fixture.after_commit,
                "content": "the comment the edit lands on",
            }),
        )
        .await;
        let comment_id = string_at(&posted, "/comment/commentId");

        // The round trip: the edit is stored and read back through GetComment, so
        // the refusal the suites above assert is a function of the comment's
        // state rather than of the operation.
        let edited = ok_as(
            &fixture.router,
            DEFAULT_ACCESS_KEY_ID,
            "UpdateComment",
            &json!({ "commentId": comment_id, "content": content }),
        )
        .await;
        assert_eq!(
            edited.pointer("/comment/content"),
            Some(&json!(content)),
            "case {label}: expected the edit to be answered with its own content, got {edited}"
        );
        let read = ok_as(
            &fixture.router,
            DEFAULT_ACCESS_KEY_ID,
            "GetComment",
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            read.pointer("/comment/content"),
            Some(&json!(content)),
            "case {label}: expected the stored edit to read back, got {read}"
        );
    }
}
