// SPDX-License-Identifier: CC0-1.0

//! `UpdatePullRequestDescription` over the wire — the OTHER operation that
//! replaces a pull request's text, asserted against the same rule its sibling
//! `pull_request_title_update` suite states for `title`, on that suite's fixture
//! and helpers rather than a second copy of them.
//!
//! The rule under test: an update that replaces a pull request's TEXT writes
//! exactly the member it names on a pull request that is still OPEN, leaving every
//! other member of the record — including the `revisionId`, which names the tips
//! the pull request is between, and the event log, which `PullRequestEventType`
//! declares no kind of entry for a text change in — as it was, and moving
//! `lastActivityDate` only when the stored value actually changed; a pull request
//! that is no longer open takes no such update and is refused with a message naming
//! the state it is IN; and a value the member does not admit is refused with the
//! error CodeCommit names for that member and persists nothing.
//!
//! The tables below span what that rule generalises over for `description`: values
//! differing in length (one character, the longest the member admits, and one past
//! it), in character set (ASCII, non-ASCII, JSON-significant punctuation), in
//! internal whitespace and in line count, repeated so an accepted no-op is proven
//! to change nothing, applied in sequence so the last write is what a later read
//! reports, and the `description` value that leaves the pull request holding none.

use serde_json::{json, Value};

use crate::harness::{error_with_message, PULL_REQUEST_DOES_NOT_EXIST};
use crate::pull_request_title_update::{
    already_closed_message, apart_from, assert_refused, events, member, opened, stored, Opened,
    Refusal, Text, ACCEPTED_TEXT, CLIENT_ERROR_STATUS, ID_REFUSALS, STATUS_CLOSED,
};
use crate::repository_lifecycle::ok;

/// The longest `description` the member admits: the `length.max` of its target
/// shape (`smithy_query(query="shape",shape="Description")` reports `length {min:
/// 0, max: 10240}`). The prose on `InvalidDescriptionException` names 1,000
/// instead; the member's own constraint is what the service enforces, so this is
/// the bound asserted.
const DESCRIPTION_MAX_CHARACTERS: usize = 10_240;

/// The message `InvalidDescriptionException` carries when the boundary refuses a
/// description the member does not admit, or a request that carries none.
const INVALID_DESCRIPTION_MESSAGE: &str = "The pull request description is not valid.";

/// One accepted `UpdatePullRequestDescription` call, answering the pull request the
/// operation reported.
async fn redescribe(opened: &Opened, description: &str) -> Value {
    let answered = ok(
        &opened.router,
        "UpdatePullRequestDescription",
        &json!({
            "pullRequestId": opened.pull_request_id,
            "description": description,
        }),
    )
    .await;
    answered
        .get("pullRequest")
        .unwrap_or_else(|| panic!("no pullRequest in {answered}"))
        .clone()
}

/// Every description asserted as accepted: the rows shared with `title`, plus the
/// two that state this member's own length bound — the longest it admits, and one
/// shorter than that yet past the number its error shape's prose names.
fn accepted_descriptions() -> Vec<Text> {
    let mut rows: Vec<Text> = ACCEPTED_TEXT
        .iter()
        .map(|row| Text {
            case: row.case,
            text: row.text,
            repeat: row.repeat,
        })
        .collect();
    rows.push(Text {
        case: "a description past the length the error shape's prose names",
        text: "p",
        repeat: DESCRIPTION_MAX_CHARACTERS - 1,
    });
    rows.push(Text {
        case: "the longest description the member admits",
        text: "d",
        repeat: DESCRIPTION_MAX_CHARACTERS,
    });
    rows
}

#[tokio::test]
async fn every_description_the_member_admits_replaces_it_and_nothing_else() {
    for (row, accepted) in accepted_descriptions().iter().enumerate() {
        let case = accepted.case;
        let description = accepted.value();
        let opened = opened(&format!("description-replace-{row}")).await;

        let before = stored(&opened.router, &opened.pull_request_id).await;
        let log_before = events(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            before.pointer("/description"),
            None,
            "case {case:?}: the fixture must open with no description, got {before}"
        );

        let answered = redescribe(&opened, &description).await;
        assert_eq!(
            member(&answered, "/description").as_deref(),
            Some(description.as_str()),
            "case {case:?}: expected the answered description to be the requested value, got {:?}",
            member(&answered, "/description")
        );
        // The answer IS the stored record, so the reader must report it exactly.
        let after = stored(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            after, answered,
            "case {case:?}: expected GetPullRequest to report exactly what the update answered, \
             got {after} for {answered}"
        );
        // Only the description is replaced: the revisionId names the tips the pull
        // request is between, which a text change does not move, and the title is
        // the OTHER text member, which this operation does not carry.
        assert_eq!(
            apart_from(&after, "description"),
            apart_from(&before, "description"),
            "case {case:?}: expected every member but the description and lastActivityDate to be \
             unchanged, got {after} for {before}"
        );
        assert!(
            after.pointer("/lastActivityDate") != before.pointer("/lastActivityDate"),
            "case {case:?}: expected lastActivityDate to move off {:?}, got {:?}",
            before.pointer("/lastActivityDate"),
            after.pointer("/lastActivityDate")
        );
        // `PullRequestEventType` names no kind of event for a text change.
        let log_after = events(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            log_after, log_before,
            "case {case:?}: expected the event log to be unchanged, got {log_after:?} for \
             {log_before:?}"
        );

        // Asking for the description already held asks for the state already held,
        // so the whole record — the activity instant included — must be left as it
        // is, however often it is asked for.
        for repeat in 0..3 {
            let again = redescribe(&opened, &description).await;
            assert_eq!(
                again, after,
                "case {case:?} repeat {repeat}: expected the pull request to be unchanged, got \
                 {again} for {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_sequence_of_descriptions_leaves_the_last_one_stored() {
    let opened = opened("description-sequence").await;
    // Every accepted description in turn on ONE pull request: each read reports the
    // value the latest accepted call named, never an earlier one.
    for accepted in accepted_descriptions() {
        let case = accepted.case;
        let description = accepted.value();
        redescribe(&opened, &description).await;
        let after = stored(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            member(&after, "/description").as_deref(),
            Some(description.as_str()),
            "case {case:?}: expected the stored description to be the latest value, got {:?}",
            member(&after, "/description")
        );
    }
}

#[tokio::test]
async fn an_empty_description_leaves_the_pull_request_with_none() {
    // `Description`'s modelled minimum length is 0 and the operation models no
    // "required" error for it, so an empty description is ACCEPTED and the pull
    // request simply holds none — the rule `CreatePullRequest` already stores a
    // description by, which is why the member is ABSENT rather than reported empty.
    let opened = opened("description-cleared").await;
    let written = redescribe(&opened, "something to say").await;
    assert_eq!(
        member(&written, "/description").as_deref(),
        Some("something to say"),
        "expected the description to be stored, got {written}"
    );

    let cleared = redescribe(&opened, "").await;
    assert_eq!(
        cleared.pointer("/description"),
        None,
        "expected an empty description to leave the pull request with none, got {cleared}"
    );
    let after = stored(&opened.router, &opened.pull_request_id).await;
    assert_eq!(
        after, cleared,
        "expected GetPullRequest to report exactly what the update answered, got {after} for \
         {cleared}"
    );
    // Clearing a description the pull request does not hold asks for the state
    // already held, so it changes nothing at all.
    let again = redescribe(&opened, "").await;
    assert_eq!(
        again, cleared,
        "expected a second clearing to change nothing, got {again} for {cleared}"
    );
}

#[tokio::test]
async fn a_closed_pull_request_takes_no_new_description() {
    let opened = opened("description-after-close").await;
    let settled = "the description it was closed with";
    redescribe(&opened, settled).await;
    ok(
        &opened.router,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": opened.pull_request_id,
            "pullRequestStatus": STATUS_CLOSED,
        }),
    )
    .await;
    let closed = stored(&opened.router, &opened.pull_request_id).await;
    assert_eq!(
        member(&closed, "/pullRequestStatus").as_deref(),
        Some(STATUS_CLOSED),
        "the fixture must be closed before the refusals, got {closed}"
    );

    // Every accepted description, the one the pull request already holds, and the
    // empty one that would otherwise clear it: once closed, the record takes no
    // update at all rather than quietly accepting a no-op.
    let descriptions: Vec<String> = accepted_descriptions()
        .iter()
        .map(Text::value)
        .chain([settled.to_string(), String::new()])
        .collect();
    for (row, description) in descriptions.iter().enumerate() {
        let case = format!("row {row}");
        let (code, message) = error_with_message(
            &opened.router,
            "UpdatePullRequestDescription",
            &json!({
                "pullRequestId": opened.pull_request_id,
                "description": description,
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestAlreadyClosedException",
            "case {case:?}: expected PullRequestAlreadyClosedException, got {code} saying \
             {message:?}"
        );
        assert_eq!(
            message,
            already_closed_message(STATUS_CLOSED),
            "case {case:?}: expected the message to name the state the pull request is in, got \
             {message:?}"
        );
        let after = stored(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            after, closed,
            "case {case:?}: expected a refused update to persist nothing, got {after} for \
             {closed}"
        );
    }
}

#[tokio::test]
async fn a_refused_description_update_names_the_member_at_fault_and_persists_nothing() {
    let opened = opened("description-refusals").await;
    // The rows that belong to `description` alone. The member is required, so a
    // request carrying none is refused for it; the operation models no "required"
    // error for a description, so that refusal is the member's `Invalid` one.
    let own = [
        Refusal {
            case: "no description at all",
            pull_request_id: Some("{id}"),
            value: None,
            repeat: 1,
            code: "InvalidDescriptionException",
            message: INVALID_DESCRIPTION_MESSAGE,
        },
        Refusal {
            case: "a description one character past the length the member admits",
            pull_request_id: Some("{id}"),
            value: Some("d"),
            repeat: DESCRIPTION_MAX_CHARACTERS + 1,
            code: "InvalidDescriptionException",
            message: INVALID_DESCRIPTION_MESSAGE,
        },
    ];

    for refusal in ID_REFUSALS.iter().chain(own.iter()) {
        assert_refused(
            &opened,
            "UpdatePullRequestDescription",
            "description",
            refusal,
        )
        .await;
    }
}

#[tokio::test]
async fn an_id_naming_no_pull_request_is_refused_with_the_fixed_message() {
    let opened = opened("description-unknown-id").await;
    // An id no pull request was ever minted as, built from the stored one so it is
    // addressable yet names nothing.
    let unknown = format!("{}0", opened.pull_request_id);
    for accepted in accepted_descriptions() {
        let case = accepted.case;
        let (code, message) = error_with_message(
            &opened.router,
            "UpdatePullRequestDescription",
            &json!({ "pullRequestId": unknown, "description": accepted.value() }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestDoesNotExistException",
            "case {case:?}: expected PullRequestDoesNotExistException, got {code} saying \
             {message:?}"
        );
        assert_eq!(
            message, PULL_REQUEST_DOES_NOT_EXIST,
            "case {case:?}: expected the message CodeCommit words that code with, got \
             {message:?}"
        );
    }
}
