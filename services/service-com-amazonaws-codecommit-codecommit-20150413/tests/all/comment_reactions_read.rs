// SPDX-License-Identifier: CC0-1.0

//! Reading the reactions on a comment: `GetCommentReactions`, driven as awsJson1_1
//! requests through the router that serves the real emulator.
//!
//! The rule under test: a read of a comment's reactions reports one entry per
//! reaction the service HOLDS on that comment — each naming that reaction in the
//! three forms the model reports it in and the principals that left it — computed
//! from the stored comment, so the answer follows what `PutCommentReaction` wrote
//! and a read changes nothing. The tables below span the class the rule covers —
//! every reaction in the service's catalog and every form a caller may send it as,
//! comments holding none, one and several reactions, several principals sharing
//! one reaction, a read narrowed to one principal, a page at a time, and the
//! published `maxResults` bounds — rather than one recorded request.

use serde_json::{json, Value};

use crate::comment_lifecycle::{fixture, Fixture, OTHER_ACCESS_KEY_ID};
use crate::repository_lifecycle::{error, ok, string_at};
use crate::wire::{caller_arn, error_as, ok_as, DEFAULT_ACCESS_KEY_ID};

/// One case of the read table: a reaction named by one of its published forms,
/// with the three forms a read reports it in — the short code, the Emoji Version
/// 1.0 graphic and the Unicode codepoint. The rows are projected out of the
/// crate's one test-side reaction catalog
/// ([`crate::comment_reaction_surfacing::CATALOG`]), which names its source.
struct ReactionCase {
    /// The value as the request sends it.
    sent: String,
    short_code: &'static str,
    emoji: &'static str,
    unicode: &'static str,
}

fn reaction_cases() -> Vec<ReactionCase> {
    crate::comment_reaction_surfacing::sent_forms()
        .into_iter()
        .map(|(sent, row)| ReactionCase {
            sent,
            short_code: row.short_code,
            emoji: row.emoji,
            unicode: row.unicode,
        })
        .collect()
}

/// The reactions a read reports for a comment, as sent by `access_key_id`.
async fn reactions_of(
    fixture: &Fixture,
    access_key_id: &str,
    body: &Value,
) -> (Vec<Value>, Option<String>) {
    let read = ok_as(
        &fixture.router,
        access_key_id,
        "GetCommentReactions",
        body,
    )
    .await;
    let entries = read
        .get("reactionsForComment")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no reactionsForComment in {read}"))
        .clone();
    let next_token = read
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (entries, next_token)
}

/// A comment on the fixture's comparison, for a suite that only needs one.
async fn comment_on(fixture: &Fixture, content: &str) -> String {
    let posted = ok(
        &fixture.router,
        "PostCommentForComparedCommit",
        &json!({
            "repositoryName": fixture.repository,
            "afterCommitId": fixture.after_commit,
            "content": content,
        }),
    )
    .await;
    string_at(&posted, "/comment/commentId")
}

#[tokio::test]
async fn a_read_reports_every_form_of_every_reaction_in_the_catalog() {
    let fixture = fixture("comment-reactions-read-catalog").await;
    let comment_id = comment_on(&fixture, "react to me in every form").await;

    for case in reaction_cases() {
        ok(
            &fixture.router,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": case.sent }),
        )
        .await;

        // One reaction per caller, so the caller's single reaction is the one
        // entry the read reports — in the three forms the catalog names it by,
        // whichever form the request sent.
        let (entries, next_token) = reactions_of(
            &fixture,
            DEFAULT_ACCESS_KEY_ID,
            &json!({ "commentId": comment_id }),
        )
        .await;
        assert_eq!(
            entries,
            vec![json!({
                "reaction": {
                    "emoji": case.emoji,
                    "shortCode": case.short_code,
                    "unicode": case.unicode,
                },
                "reactionUsers": [caller_arn(DEFAULT_ACCESS_KEY_ID)],
                "reactionsFromDeletedUsersCount": 0,
            })],
            "case {:?}: expected the one reaction the caller holds, got {entries:?}",
            case.sent
        );
        assert!(
            next_token.is_none(),
            "case {:?}: expected no further page, got {next_token:?}",
            case.sent
        );
    }
}

#[tokio::test]
async fn a_read_reports_each_reaction_held_with_the_principals_that_left_it() {
    let fixture = fixture("comment-reactions-read-principals").await;
    let comment_id = comment_on(&fixture, "react to me from several callers").await;

    // A fresh comment holds no reactions, so the read reports an empty list
    // rather than an error.
    let (entries, next_token) = reactions_of(
        &fixture,
        DEFAULT_ACCESS_KEY_ID,
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert!(
        entries.is_empty() && next_token.is_none(),
        "case \"no reactions\": expected an empty list, got {entries:?} and {next_token:?}"
    );

    // Three principals: two sharing one reaction (sent in two different forms)
    // and one leaving another, so an entry can carry more than one user and a
    // comment more than one entry.
    const THIRD_ACCESS_KEY_ID: &str = "AKIDTHIRDUSER";
    fixture.hold(THIRD_ACCESS_KEY_ID);
    for (access_key_id, sent) in [
        (DEFAULT_ACCESS_KEY_ID, "\u{1f44d}"),
        (OTHER_ACCESS_KEY_ID, ":+1:"),
        (THIRD_ACCESS_KEY_ID, ":tada:"),
    ] {
        ok_as(
            &fixture.router,
            access_key_id,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": sent }),
        )
        .await;
    }

    // The two principals sharing `:thumbsup:` are reported together, in the order
    // the comment holds them (ascending ARN), and the catalog's order puts
    // `:thumbsup:` before `:hooray:`.
    let mut thumbs_up_users = vec![
        caller_arn(DEFAULT_ACCESS_KEY_ID),
        caller_arn(OTHER_ACCESS_KEY_ID),
    ];
    thumbs_up_users.sort();
    let expected = vec![
        json!({
            "reaction": {
                "emoji": "\u{1f44d}",
                "shortCode": ":thumbsup:",
                "unicode": "U+1F44D",
            },
            "reactionUsers": thumbs_up_users,
            "reactionsFromDeletedUsersCount": 0,
        }),
        json!({
            "reaction": {
                "emoji": "\u{1f389}",
                "shortCode": ":hooray:",
                "unicode": "U+1F389",
            },
            "reactionUsers": [caller_arn(THIRD_ACCESS_KEY_ID)],
            "reactionsFromDeletedUsersCount": 0,
        }),
    ];
    let (entries, _) = reactions_of(
        &fixture,
        DEFAULT_ACCESS_KEY_ID,
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert_eq!(
        entries, expected,
        "case \"three principals\": expected one entry per reaction held, got {entries:?}"
    );

    // The answer does not depend on who is asking, and reading changes nothing:
    // another principal's read, and a second read, answer the same.
    for (label, access_key_id) in [
        ("another principal", OTHER_ACCESS_KEY_ID),
        ("a second read", DEFAULT_ACCESS_KEY_ID),
    ] {
        let (again, _) =
            reactions_of(&fixture, access_key_id, &json!({ "commentId": comment_id })).await;
        assert_eq!(
            again, expected,
            "case {label:?}: expected the same stored reactions, got {again:?}"
        );
    }

    // `reactionUserArn` narrows the read to one principal's own reaction; a
    // principal that left none is reported nothing at all.
    let no_reaction = "AKIDNOREACTION";
    fixture.hold(no_reaction);
    for (label, access_key_id, expected_entries) in [
        ("a user holding a shared reaction", DEFAULT_ACCESS_KEY_ID, 1),
        ("a user holding its own reaction", THIRD_ACCESS_KEY_ID, 1),
        ("a user holding none", no_reaction, 0),
    ] {
        let arn = caller_arn(access_key_id);
        let (narrowed, _) = reactions_of(
            &fixture,
            DEFAULT_ACCESS_KEY_ID,
            &json!({ "commentId": comment_id, "reactionUserArn": arn }),
        )
        .await;
        assert_eq!(
            narrowed.len(),
            expected_entries,
            "case {label:?}: expected {expected_entries} entries, got {narrowed:?}"
        );
        for entry in &narrowed {
            assert_eq!(
                entry.get("reactionUsers"),
                Some(&json!([arn])),
                "case {label:?}: expected only that principal's reaction, got {entry}"
            );
        }
    }

    // A removal is a write the read follows: the principal's entry goes, and the
    // reaction it shared keeps the other principal.
    ok(
        &fixture.router,
        "PutCommentReaction",
        &json!({ "commentId": comment_id, "reactionValue": "none" }),
    )
    .await;
    let (after, _) = reactions_of(
        &fixture,
        DEFAULT_ACCESS_KEY_ID,
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert_eq!(
        after
            .first()
            .and_then(|entry| entry.get("reactionUsers"))
            .cloned(),
        Some(json!([caller_arn(OTHER_ACCESS_KEY_ID)])),
        "case \"a removal\": expected the remaining principal alone, got {after:?}"
    );
    assert_eq!(
        after.len(),
        2,
        "case \"a removal\": expected the other reactions to stand, got {after:?}"
    );
}

#[tokio::test]
async fn a_read_pages_the_reactions_it_holds() {
    let fixture = fixture("comment-reactions-read-pages").await;
    let comment_id = comment_on(&fixture, "react to me from many callers").await;

    // One principal per reaction, so the comment holds as many entries as the
    // rows paged over below. The catalog's order is the order they are reported
    // in, whatever order they were left in.
    let left: Vec<&str> = vec![":rocket:", ":eyes:", ":smile:", ":thumbsdown:"];
    for (index, reaction) in left.iter().enumerate() {
        let page_user = format!("AKIDPAGEUSER{index}");
        fixture.hold(&page_user);
        ok_as(
            &fixture.router,
            &page_user,
            "PutCommentReaction",
            &json!({ "commentId": comment_id, "reactionValue": reaction }),
        )
        .await;
    }
    let (whole, whole_token) = reactions_of(
        &fixture,
        DEFAULT_ACCESS_KEY_ID,
        &json!({ "commentId": comment_id }),
    )
    .await;
    assert!(
        whole.len() == left.len() && whole_token.is_none(),
        "case \"the whole listing\": expected {} entries and no token, got {whole:?} and \
         {whole_token:?}",
        left.len()
    );

    // Every page size from one entry to one past the whole listing walks the SAME
    // sequence, in the same order, and only the last page of each walk carries no
    // token.
    for page_size in 1..=left.len() + 1 {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({ "commentId": comment_id, "maxResults": page_size });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let (page, next) = reactions_of(&fixture, DEFAULT_ACCESS_KEY_ID, &body).await;
            assert!(
                page.len() <= page_size,
                "case {page_size:?}: expected at most {page_size} entries, got {page:?}"
            );
            walked.extend(page);
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            walked, whole,
            "case {page_size:?}: expected the paged walk to equal the whole listing, got \
             {walked:?}"
        );
    }

    // The published `maxResults` range: "A non-zero, non-negative integer used to
    // limit the number of returned results. The default is the same as the allowed
    // maximum, 1,000." Each bound is accepted and one past each is refused.
    for accepted in [1, 1_000] {
        let (page, _) = reactions_of(
            &fixture,
            DEFAULT_ACCESS_KEY_ID,
            &json!({ "commentId": comment_id, "maxResults": accepted }),
        )
        .await;
        assert!(
            !page.is_empty(),
            "case {accepted:?}: expected a page of results, got {page:?}"
        );
    }
    for refused in [0, -1, 1_001] {
        let code = error(
            &fixture.router,
            "GetCommentReactions",
            &json!({ "commentId": comment_id, "maxResults": refused }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidMaxResultsException",
            "case {refused:?}: expected InvalidMaxResultsException, got {code:?}"
        );
    }

    // A continuation token this service did not issue for THIS listing names no
    // position in it.
    let other_comment = comment_on(&fixture, "another comment to page").await;
    ok_as(
        &fixture.router,
        "AKIDPAGEUSER0",
        "PutCommentReaction",
        &json!({ "commentId": other_comment, "reactionValue": ":eyes:" }),
    )
    .await;
    ok_as(
        &fixture.router,
        "AKIDPAGEUSER1",
        "PutCommentReaction",
        &json!({ "commentId": other_comment, "reactionValue": ":heart:" }),
    )
    .await;
    let (_, issued_elsewhere) = reactions_of(
        &fixture,
        DEFAULT_ACCESS_KEY_ID,
        &json!({ "commentId": other_comment, "maxResults": 1 }),
    )
    .await;
    let issued_elsewhere =
        issued_elsewhere.expect("a listing of two entries read one at a time has a further page");
    for (label, token) in [
        ("a token from another comment's listing", issued_elsewhere),
        ("a decimal offset", "1".to_string()),
        ("text that is not a token", "not-a-token".to_string()),
    ] {
        let code = error(
            &fixture.router,
            "GetCommentReactions",
            &json!({ "commentId": comment_id, "maxResults": 1, "nextToken": token }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidContinuationTokenException",
            "case {label:?}: expected InvalidContinuationTokenException, got {code:?}"
        );
    }
}

#[tokio::test]
async fn a_read_is_refused_for_a_comment_or_user_the_service_does_not_hold() {
    let fixture = fixture("comment-reactions-read-refusals").await;
    let comment_id = comment_on(&fixture, "react to me before deletion").await;
    ok(
        &fixture.router,
        "PutCommentReaction",
        &json!({ "commentId": comment_id, "reactionValue": ":eyes:" }),
    )
    .await;

    // A `commentId` that addresses no comment, by each reason it cannot: blank,
    // not the form a comment id takes, and a well-formed id no comment carries.
    for (label, requested, expected) in [
        ("blank", "", "CommentIdRequiredException"),
        ("not a comment id", "not-a-comment-id", "InvalidCommentIdException"),
        (
            "half a comment id",
            "11111111-2222-3333-4444-555555555555",
            "InvalidCommentIdException",
        ),
        (
            "an id no comment carries",
            "11111111-2222-3333-4444-555555555555:66666666-7777-8888-9999-aaaaaaaaaaaa",
            "CommentDoesNotExistException",
        ),
    ] {
        let code = error(
            &fixture.router,
            "GetCommentReactions",
            &json!({ "commentId": requested }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {label:?}: expected {expected}, got {code:?}"
        );
    }

    // A `reactionUserArn` that is not a full ARN names no identity.
    for (label, requested) in [
        ("a bare user name", "someone"),
        ("a truncated ARN", "arn:aws:iam::"),
        ("an ARN with no resource", "arn:aws:iam::123456789012:"),
    ] {
        let code = error_as(
            &fixture.router,
            DEFAULT_ACCESS_KEY_ID,
            "GetCommentReactions",
            &json!({ "commentId": comment_id, "reactionUserArn": requested }),
            400,
        )
        .await;
        assert_eq!(
            code, "InvalidReactionUserArnException",
            "case {label:?}: expected InvalidReactionUserArnException, got {code:?}"
        );
    }

    // A comment whose content has been deleted is no longer reported on.
    ok(
        &fixture.router,
        "DeleteCommentContent",
        &json!({ "commentId": comment_id }),
    )
    .await;
    let code = error(
        &fixture.router,
        "GetCommentReactions",
        &json!({ "commentId": comment_id }),
        400,
    )
    .await;
    assert_eq!(
        code, "CommentDeletedException",
        "case \"a deleted comment\": expected CommentDeletedException, got {code:?}"
    );
}
