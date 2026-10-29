// SPDX-License-Identifier: CC0-1.0

//! How a `commentId` is classified, over the wire, for every operation that takes
//! one: `GetComment`, `UpdateComment`, `DeleteCommentContent`,
//! `PutCommentReaction`, `GetCommentReactions` and `PostCommentReply`.
//!
//! The rule under test: a comment id is judged by its FORM — a pair of UUIDs
//! joined by a colon — before it is looked up, so an id of that form that names
//! no stored comment is a not-found (`CommentDoesNotExistException`, with the
//! fixed sentence "The following comment ID does not exist: " and the id the
//! request named) while an id that is not of that form, half an id included, is
//! malformed (`InvalidCommentIdException`). Both are HTTP 400. The id the service
//! mints is of the same form, so the form it accepts and the form it issues are
//! one rule: a minted id addresses its comment, and either half of it alone does
//! not.
//!
//! The tables below span the class the rule covers — the halves' digits and case,
//! identical and distinct halves, and each way a candidate can miss the form
//! (half an id, an empty half, a third half, a half of the wrong width, a
//! non-hexadecimal digit, the halves joined by something else, no id shape at
//! all) — rather than one recorded id.

use serde_json::{json, Value};

use crate::comment_lifecycle::{fixture, Fixture};
use crate::harness::error_with_message;
use crate::repository_lifecycle::{is_comment_id, ok, string_at};

/// One candidate comment id, and whether it is of the form a comment id takes.
struct FormCase {
    label: &'static str,
    id: &'static str,
    /// Is the candidate a pair of UUIDs joined by a colon? That decides WHICH
    /// refusal an id naming no comment draws, which is the rule under test.
    well_formed: bool,
}

/// The table: rows differ along what the form rule generalises over — the digits
/// and the case of each half, whether the halves are equal, and each way a
/// candidate can fail the form.
fn form_cases() -> Vec<FormCase> {
    vec![
        FormCase {
            label: "both halves all zero",
            id: "00000000-0000-0000-0000-000000000000:00000000-0000-0000-0000-000000000000",
            well_formed: true,
        },
        FormCase {
            label: "two distinct halves",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:1f9c5a2b-2d3e-4f50-9b0c-1d2e3f4a5b6c",
            well_formed: true,
        },
        FormCase {
            label: "upper-case hexadecimal digits",
            id: "0E8B4F1A-1C2D-4E5F-8A9B-0C1D2E3F4A5B:1F9C5A2B-2D3E-4F50-9B0C-1D2E3F4A5B6C",
            well_formed: true,
        },
        FormCase {
            label: "both halves the same id",
            id: "ffffffff-ffff-ffff-ffff-ffffffffffff:ffffffff-ffff-ffff-ffff-ffffffffffff",
            well_formed: true,
        },
        FormCase {
            label: "half an id: a single UUID",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
            well_formed: false,
        },
        FormCase {
            label: "a first half and nothing after the separator",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:",
            well_formed: false,
        },
        FormCase {
            label: "a separator and nothing before it",
            id: ":0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
            well_formed: false,
        },
        FormCase {
            label: "three halves",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:\
                 0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
            well_formed: false,
        },
        FormCase {
            label: "a second half one group short",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:1f9c5a2b-2d3e-4f50-9b0c",
            well_formed: false,
        },
        FormCase {
            label: "a first half one digit too long",
            id: "0e8b4f1aa-1c2d-4e5f-8a9b-0c1d2e3f4a5b:1f9c5a2b-2d3e-4f50-9b0c-1d2e3f4a5b6c",
            well_formed: false,
        },
        FormCase {
            label: "a non-hexadecimal digit in the second half",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b:1f9c5a2g-2d3e-4f50-9b0c-1d2e3f4a5b6c",
            well_formed: false,
        },
        FormCase {
            label: "the two halves with the dashes stripped out",
            id: "0e8b4f1a1c2d4e5f8a9b0c1d2e3f4a5b:1f9c5a2b2d3e4f509b0c1d2e3f4a5b6c",
            well_formed: false,
        },
        FormCase {
            label: "two halves joined by a dash instead of the separator",
            id: "0e8b4f1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b-1f9c5a2b-2d3e-4f50-9b0c-1d2e3f4a5b6c",
            well_formed: false,
        },
        FormCase {
            label: "no id shape at all",
            id: "comment-1",
            well_formed: false,
        },
    ]
}

/// One operation that takes a comment id: where in its input the id goes, and the
/// other members it needs, so the id is the only thing wrong with the request.
struct CommentIdOperation {
    operation: &'static str,
    /// `PostCommentReply` names the member it takes a comment id in `inReplyTo`;
    /// every other operation calls it `commentId`.
    member: &'static str,
    extra: Value,
}

fn comment_id_operations() -> Vec<CommentIdOperation> {
    vec![
        CommentIdOperation {
            operation: "GetComment",
            member: "commentId",
            extra: json!({}),
        },
        CommentIdOperation {
            operation: "UpdateComment",
            member: "commentId",
            extra: json!({ "content": "an edit" }),
        },
        CommentIdOperation {
            operation: "DeleteCommentContent",
            member: "commentId",
            extra: json!({}),
        },
        CommentIdOperation {
            operation: "PutCommentReaction",
            member: "commentId",
            extra: json!({ "reactionValue": ":thumbsup:" }),
        },
        CommentIdOperation {
            operation: "GetCommentReactions",
            member: "commentId",
            extra: json!({}),
        },
        CommentIdOperation {
            operation: "PostCommentReply",
            member: "inReplyTo",
            extra: json!({ "content": "a reply to nothing" }),
        },
    ]
}

/// The request `operation` sends about `id`.
fn body(operation: &CommentIdOperation, id: &str) -> Value {
    let mut input = json!({});
    input[operation.member] = json!(id);
    for (member, value) in operation.extra.as_object().expect("an object") {
        input[member.as_str()] = value.clone();
    }
    input
}

/// A comment on the fixture's comparison.
async fn comment_on(fixture: &Fixture, content: &str) -> String {
    let posted = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "beforeCommitId": fixture.before_commit,
            "content": content,
        }),
    )
    .await;
    string_at(&posted, "/comment/commentId")
}

/// Every comment id the fixture's comparison reports, through
/// `GetCommentsForComparedCommit`.
async fn ids_on_the_comparison(fixture: &Fixture) -> Vec<String> {
    let read = ok(
        &fixture.router,
        "GetCommentsForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "beforeCommitId": fixture.before_commit,
        }),
    )
    .await;
    let mut ids: Vec<String> = read
        .get("commentsForComparedCommitData")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no comparison data in {read}"))
        .iter()
        .filter_map(|entry| entry.get("comments").and_then(Value::as_array))
        .flatten()
        .map(|comment| string_at(comment, "/commentId"))
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_comment_id_is_classified_by_its_form_before_it_is_looked_up() {
    let fixture = fixture("comment-id-form-classification").await;
    // A real comment, so the suite can prove the refusals below left the stored
    // thread alone.
    let real = comment_on(&fixture, "the one comment this comparison holds").await;
    let before = ids_on_the_comparison(&fixture).await;

    for operation in comment_id_operations() {
        for case in form_cases() {
            let FormCase {
                label,
                id,
                well_formed,
            } = case;
            // The rule: the form decides WHICH refusal an id that names no stored
            // comment draws.
            let expected_code = if well_formed {
                "CommentDoesNotExistException"
            } else {
                "InvalidCommentIdException"
            };
            let (code, message) = error_with_message(
                &fixture.router,
                operation.operation,
                &body(&operation, id),
                400,
            )
            .await;
            assert_eq!(
                code, expected_code,
                "case {label:?} on {}: expected {expected_code}, got {code:?} saying {message:?}",
                operation.operation
            );
            if well_formed {
                // The not-found message: a fixed sentence and then the id the
                // request named, which is its only variable part.
                let expected_message = format!("The following comment ID does not exist: {id}");
                assert_eq!(
                    message, expected_message,
                    "case {label:?} on {}: expected {expected_message:?}, got {message:?}",
                    operation.operation
                );
            } else {
                // The malformed refusal's own wording is not asserted here: no
                // recorded answer for it exists, and this family's templates are
                // demonstrably not the exception's documentation (the not-found
                // template above is not its documented sentence either), so only
                // what is known is pinned — the code, the status and that a
                // message came with it.
                assert!(
                    !message.is_empty(),
                    "case {label:?} on {}: expected a message with the refusal, got {message:?}",
                    operation.operation
                );
            }
        }
    }

    // None of those refusals wrote anything: the comparison still holds exactly
    // the comment that was posted, and it is still readable.
    let after = ids_on_the_comparison(&fixture).await;
    assert_eq!(
        after,
        before,
        "expected the refusals to leave the thread holding {before:?}, got {after:?}"
    );
    let read = ok(&fixture.router, "GetComment", &json!({ "commentId": real })).await;
    assert_eq!(
        string_at(&read, "/comment/commentId"),
        real,
        "expected the real comment {real} to still be readable, got {read}"
    );
}

#[tokio::test]
async fn a_minted_comment_id_is_a_pair_of_uuids_and_only_the_full_id_addresses_it() {
    let fixture = fixture("comment-id-form-minted").await;

    // Ids minted by each operation that posts a comment on a comparison: a root
    // comment, a reply to it, and a reply to that reply.
    let root = comment_on(&fixture, "the root of the thread").await;
    let posted_reply = ok(
        &fixture.router,
        "PostCommentReply",
        &json!({ "inReplyTo": root, "content": "the first reply" }),
    )
    .await;
    let reply = string_at(&posted_reply, "/comment/commentId");
    let posted_nested = ok(
        &fixture.router,
        "PostCommentReply",
        &json!({ "inReplyTo": reply, "content": "a reply to the reply" }),
    )
    .await;
    let nested = string_at(&posted_nested, "/comment/commentId");
    let second_root = comment_on(&fixture, "a second root on the same comparison").await;

    let minted = [
        ("a root comment", &root),
        ("a reply", &reply),
        ("a reply to a reply", &nested),
        ("a second root comment", &second_root),
    ];
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (case, id) in minted {
        assert!(
            is_comment_id(id),
            "case {case:?}: expected a comment id of the form <uuid>:<uuid>, got {id:?}"
        );
        assert!(
            seen.insert(id.as_str()),
            "case {case:?}: the id {id} was already issued to another comment"
        );
        // The full id addresses the comment it was minted for.
        let read = ok(&fixture.router, "GetComment", &json!({ "commentId": id })).await;
        assert_eq!(
            string_at(&read, "/comment/commentId"),
            *id,
            "case {case:?}: expected GetComment to report {id}, got {read}"
        );
        // Either half on its own is not a full comment id, so it is malformed
        // rather than a comment that cannot be found.
        let (first, second) = id.split_once(':').expect("a minted id has two halves");
        for (half, part) in [("the first half", first), ("the second half", second)] {
            let (code, message) = error_with_message(
                &fixture.router,
                "GetComment",
                &json!({ "commentId": part }),
                400,
            )
            .await;
            assert_eq!(
                code, "InvalidCommentIdException",
                "case {case:?} with {half}: expected InvalidCommentIdException, got {code:?} \
                 saying {message:?}"
            );
        }
    }
}
