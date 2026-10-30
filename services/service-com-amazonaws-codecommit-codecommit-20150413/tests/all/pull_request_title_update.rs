// SPDX-License-Identifier: CC0-1.0

//! `UpdatePullRequestTitle` over the wire, driven as awsJson1_1 requests through
//! the router that serves the real emulator (the harness comes from the sibling
//! `repository_lifecycle` suite and the pull request fixture from
//! `pull_request_comments`, rather than a third copy of either).
//!
//! The rule under test, which this suite asserts for `title` and the sibling
//! `pull_request_text_update` suite asserts for `description`: an update that
//! replaces a pull request's TEXT writes exactly the member it names on a pull
//! request that is still OPEN, leaving every other member of the record — including
//! the `revisionId`, which names the tips the pull request is between, and the
//! event log, which `PullRequestEventType` declares no kind of entry for a text
//! change in — as it was, and moving `lastActivityDate` only when the stored value
//! actually changed; a pull request that is no longer open takes no such update and
//! is refused with a message naming the state it is IN; and a value the member does
//! not admit is refused with the error CodeCommit names for that member and
//! persists nothing.
//!
//! The tables below span what that rule generalises over for `title`: values
//! differing in length (one character, the longest the member admits, and one past
//! it), in character set (ASCII, non-ASCII, JSON-significant punctuation), in
//! internal whitespace and in line count, repeated so an accepted no-op is proven
//! to change nothing, applied in sequence so the last write is what a later read
//! reports, and every `pullRequestId` and `title` value the operation models an
//! error for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_with_message, PULL_REQUEST_DOES_NOT_EXIST};
use crate::pull_request_comments::pull_request_on;
use crate::repository_lifecycle::{emulator, ok};

/// The longest `title` the member admits: the `length.max` of its target shape
/// (`smithy_query(query="shape",shape="Title")` reports `length {min: 0, max:
/// 150}`). The prose on `InvalidTitleException` names 100 instead; the member's
/// own constraint is what the service enforces, so this is the bound asserted.
const TITLE_MAX_CHARACTERS: usize = 150;

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
///
/// `pub(crate)` here and on the fixture helpers below: the sibling
/// `pull_request_text_update` suite asserts the same rule for `description` on the
/// same fixture rather than building a second copy of it.
pub(crate) const CLIENT_ERROR_STATUS: u16 = 400;

pub(crate) const STATUS_OPEN: &str = "OPEN";
pub(crate) const STATUS_CLOSED: &str = "CLOSED";

/// The title [`pull_request_on`] opens its pull request with — the value every
/// replacement below is measured against. It opens with no description at all.
pub(crate) const ORIGINAL_TITLE: &str = "a change to review";

/// The message `TitleRequiredException` carries.
const TITLE_REQUIRED_MESSAGE: &str = "A pull request title is required.";

/// The message `InvalidTitleException` carries when the boundary refuses a title
/// the member does not admit.
const INVALID_TITLE_MESSAGE: &str = "The title of the pull request is not valid.";

/// The message `PullRequestIdRequiredException` carries, for a request that named
/// no `pullRequestId` member at all.
pub(crate) const ID_REQUIRED_MESSAGE: &str =
    "A pull request ID is required, but none was provided.";

/// The message `InvalidPullRequestIdException` carries for each id the rows below
/// name: the value the request carried, in the sentence CodeCommit words that error
/// with, and the clause that says so where the value holds no characters.
pub(crate) const EMPTY_ID_MESSAGE: &str =
    "The following pull request ID is empty or null and therefore not valid: . To view a list of \
     valid pull request IDs for this repository, use ListPullRequests.";
pub(crate) const INVALID_ID_MESSAGE: &str =
    "The following pull request ID is not valid: not-an-id. To view a list of valid pull request \
     IDs for this repository, use ListPullRequests.";
pub(crate) const SPACED_ID_MESSAGE: &str =
    "The following pull request ID is not valid:  1 . To view a list of valid pull request IDs \
     for this repository, use ListPullRequests.";

/// The message `PullRequestAlreadyClosedException` carries for an update of a pull
/// request's text: the sentence pair names the state the pull request is IN — a
/// `PullRequestStatusEnum` value
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`) — and then which
/// changes a pull request no longer takes once it is closed.
pub(crate) fn already_closed_message(status: &str) -> String {
    format!(
        "The change cannot be made because the state of the pull request is {status}. You cannot \
         update the title or description of a pull request after it has been closed."
    )
}

/// One stored, open pull request and the router that serves it.
pub(crate) struct Opened {
    pub(crate) router: MultiServiceRouter,
    pub(crate) pull_request_id: String,
}

/// Open one pull request on a fresh emulator through the implemented write path.
pub(crate) async fn opened(repository: &str) -> Opened {
    let router = emulator();
    let built = pull_request_on(&router, repository).await;
    Opened {
        router,
        pull_request_id: built.pull_request_id,
    }
}

/// The pull request `GetPullRequest` reports — the reader every write below is
/// proven through.
pub(crate) async fn stored(router: &MultiServiceRouter, pull_request_id: &str) -> Value {
    let read = ok(
        router,
        "GetPullRequest",
        &json!({ "pullRequestId": pull_request_id }),
    )
    .await;
    read.get("pullRequest")
        .unwrap_or_else(|| panic!("no pullRequest in {read}"))
        .clone()
}

/// The entries `DescribePullRequestEvents` reports for a pull request, oldest
/// first: a text change is no `PullRequestEventType`, so this list is what proves
/// the log untouched.
pub(crate) async fn events(router: &MultiServiceRouter, pull_request_id: &str) -> Vec<Value> {
    let payload = ok(
        router,
        "DescribePullRequestEvents",
        &json!({ "pullRequestId": pull_request_id }),
    )
    .await;
    payload
        .get("pullRequestEvents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("no pullRequestEvents in {payload}"))
}

/// A string member of a payload, or `None` where the payload carries none.
pub(crate) fn member(payload: &Value, pointer: &str) -> Option<String> {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A pull request with the member under test and the activity instant removed, so
/// comparing two of these asserts that NOTHING else moved.
pub(crate) fn apart_from(pull_request: &Value, name: &str) -> Value {
    let mut rest = pull_request.clone();
    if let Some(object) = rest.as_object_mut() {
        object.remove(name);
        object.remove("lastActivityDate");
    }
    rest
}

/// One accepted `UpdatePullRequestTitle` call, answering the pull request the
/// operation reported.
async fn retitle(opened: &Opened, title: &str) -> Value {
    let answered = ok(
        &opened.router,
        "UpdatePullRequestTitle",
        &json!({ "pullRequestId": opened.pull_request_id, "title": title }),
    )
    .await;
    answered
        .get("pullRequest")
        .unwrap_or_else(|| panic!("no pullRequest in {answered}"))
        .clone()
}

/// One text value a request names, built as `text` repeated `repeat` times so a
/// row can state a length as well as a character set.
pub(crate) struct Text {
    pub(crate) case: &'static str,
    pub(crate) text: &'static str,
    pub(crate) repeat: usize,
}

impl Text {
    pub(crate) fn value(&self) -> String {
        self.text.repeat(self.repeat)
    }
}

/// The values both text members admit, spanning their character set, their
/// internal whitespace and their line count. A row stating a LENGTH belongs to one
/// member, because the two admit different lengths.
pub(crate) const ACCEPTED_TEXT: &[Text] = &[
    Text {
        case: "a single character",
        text: "x",
        repeat: 1,
    },
    Text {
        case: "inner and surrounding whitespace, kept verbatim",
        text: "  spaced   out  ",
        repeat: 1,
    },
    Text {
        case: "characters outside ASCII",
        text: "レビューしてください ✅",
        repeat: 1,
    },
    Text {
        case: "JSON-significant punctuation",
        text: "fix: \"quotes\", back\\slashes & <angles>",
        repeat: 1,
    },
    Text {
        case: "several lines",
        text: "why\n\n- one\n- two\n",
        repeat: 1,
    },
];

/// Every title asserted as accepted: the shared rows, plus the two that state this
/// member's own length bound — the longest it admits, and one shorter than that yet
/// past the number its error shape's prose names.
fn accepted_titles() -> Vec<Text> {
    let mut rows: Vec<Text> = ACCEPTED_TEXT
        .iter()
        .map(|row| Text {
            case: row.case,
            text: row.text,
            repeat: row.repeat,
        })
        .collect();
    rows.push(Text {
        case: "a title past the length the error shape's prose names",
        text: "p",
        repeat: TITLE_MAX_CHARACTERS - 1,
    });
    rows.push(Text {
        case: "the longest title the member admits",
        text: "t",
        repeat: TITLE_MAX_CHARACTERS,
    });
    rows
}

#[tokio::test]
async fn every_title_the_member_admits_replaces_it_and_nothing_else() {
    for (row, accepted) in accepted_titles().iter().enumerate() {
        let case = accepted.case;
        let title = accepted.value();
        let opened = opened(&format!("title-replace-{row}")).await;

        let before = stored(&opened.router, &opened.pull_request_id).await;
        let log_before = events(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            member(&before, "/title").as_deref(),
            Some(ORIGINAL_TITLE),
            "case {case:?}: the fixture must open with {ORIGINAL_TITLE:?}, got {:?}",
            member(&before, "/title")
        );

        let answered = retitle(&opened, &title).await;
        assert_eq!(
            member(&answered, "/title").as_deref(),
            Some(title.as_str()),
            "case {case:?}: expected the answered title to be the requested value, got {:?}",
            member(&answered, "/title")
        );
        // The answer IS the stored record, so the reader must report it exactly.
        let after = stored(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            after, answered,
            "case {case:?}: expected GetPullRequest to report exactly what the update answered, \
             got {after} for {answered}"
        );
        // Only the title is replaced: the revisionId names the tips the pull request
        // is between, which a text change does not move, and nor does it close,
        // merge or re-approve anything.
        assert_eq!(
            apart_from(&after, "title"),
            apart_from(&before, "title"),
            "case {case:?}: expected every member but the title and lastActivityDate to be \
             unchanged, got {after} for {before}"
        );
        // Replacing a pull request's text is activity on it, so the instant moved.
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

        // Asking for the title already held asks for the state already held, so the
        // whole record — the activity instant included — must be left as it is,
        // however often it is asked for.
        for repeat in 0..3 {
            let again = retitle(&opened, &title).await;
            assert_eq!(
                again, after,
                "case {case:?} repeat {repeat}: expected the pull request to be unchanged, got \
                 {again} for {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_sequence_of_retitles_leaves_the_last_one_stored() {
    let opened = opened("title-sequence").await;
    // Every accepted title in turn on ONE pull request: each read reports the title
    // the latest accepted call named, never an earlier one.
    for accepted in accepted_titles() {
        let case = accepted.case;
        let title = accepted.value();
        retitle(&opened, &title).await;
        let after = stored(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            member(&after, "/title").as_deref(),
            Some(title.as_str()),
            "case {case:?}: expected the stored title to be the latest value, got {:?}",
            member(&after, "/title")
        );
    }
}

#[tokio::test]
async fn a_closed_pull_request_takes_no_retitle() {
    let opened = opened("title-after-close").await;
    let settled = "the title it was closed with";
    retitle(&opened, settled).await;
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

    // Every accepted title, plus the one the pull request already holds: once
    // closed, the record takes no update at all rather than quietly accepting a
    // no-op.
    let titles: Vec<String> = accepted_titles()
        .iter()
        .map(Text::value)
        .chain([settled.to_string()])
        .collect();
    for (row, title) in titles.iter().enumerate() {
        let case = format!("row {row}");
        let (code, message) = error_with_message(
            &opened.router,
            "UpdatePullRequestTitle",
            &json!({ "pullRequestId": opened.pull_request_id, "title": title }),
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

/// One request an operation refuses, and the error it names for it.
pub(crate) struct Refusal {
    pub(crate) case: &'static str,
    /// The `pullRequestId` the request names, or `None` to leave the member out;
    /// `{id}` stands for the stored pull request's id.
    pub(crate) pull_request_id: Option<&'static str>,
    /// The text the operation's own member is built from, or `None` to leave it
    /// out.
    pub(crate) value: Option<&'static str>,
    /// How many times that text is repeated, so a row can state a length.
    pub(crate) repeat: usize,
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

/// The rows every text update is refused by, whatever member it carries: the
/// `pullRequestId` values these operations model an error for.
pub(crate) const ID_REFUSALS: &[Refusal] = &[
    Refusal {
        case: "no pullRequestId at all",
        pull_request_id: None,
        value: Some("new text"),
        repeat: 1,
        code: "PullRequestIdRequiredException",
        message: ID_REQUIRED_MESSAGE,
    },
    Refusal {
        case: "an empty pullRequestId",
        pull_request_id: Some(""),
        value: Some("new text"),
        repeat: 1,
        code: "InvalidPullRequestIdException",
        message: EMPTY_ID_MESSAGE,
    },
    Refusal {
        case: "a pullRequestId that is not a decimal id",
        pull_request_id: Some("not-an-id"),
        value: Some("new text"),
        repeat: 1,
        code: "InvalidPullRequestIdException",
        message: INVALID_ID_MESSAGE,
    },
    Refusal {
        case: "a pullRequestId carrying surrounding space",
        pull_request_id: Some(" 1 "),
        value: Some("new text"),
        repeat: 1,
        code: "InvalidPullRequestIdException",
        message: SPACED_ID_MESSAGE,
    },
];

/// Send one request built from a refusal row and assert the code, the message, and
/// that the pull request is exactly as it was.
pub(crate) async fn assert_refused(
    opened: &Opened,
    operation: &str,
    name: &str,
    refusal: &Refusal,
) {
    let case = format!("{operation} / {}", refusal.case);
    let mut body = json!({});
    if let Some(id) = refusal.pull_request_id {
        let id = if id == "{id}" {
            opened.pull_request_id.clone()
        } else {
            id.to_string()
        };
        body["pullRequestId"] = json!(id);
    }
    if let Some(text) = refusal.value {
        body[name] = json!(text.repeat(refusal.repeat));
    }
    let (code, message) =
        error_with_message(&opened.router, operation, &body, CLIENT_ERROR_STATUS).await;
    assert_eq!(
        code, refusal.code,
        "case {case:?}: expected error {}, got {code} saying {message:?}",
        refusal.code
    );
    assert_eq!(
        message, refusal.message,
        "case {case:?}: expected the documented message, got {message:?}"
    );
    let after = stored(&opened.router, &opened.pull_request_id).await;
    assert_eq!(
        member(&after, "/title").as_deref(),
        Some(ORIGINAL_TITLE),
        "case {case:?}: expected a refused request to leave the title at {ORIGINAL_TITLE:?}, got \
         {:?}",
        member(&after, "/title")
    );
    assert_eq!(
        after.pointer("/description"),
        None,
        "case {case:?}: expected a refused request to leave the pull request with no \
         description, got {after}"
    );
    assert_eq!(
        member(&after, "/pullRequestStatus").as_deref(),
        Some(STATUS_OPEN),
        "case {case:?}: expected a refused request to leave the pull request open, got {:?}",
        member(&after, "/pullRequestStatus")
    );
}

#[tokio::test]
async fn a_refused_retitle_names_the_member_at_fault_and_persists_nothing() {
    let opened = opened("title-refusals").await;
    // The rows that belong to `title` alone: a pull request cannot be titled with
    // nothing, which is why this operation models `TitleRequiredException`.
    let own = [
        Refusal {
            case: "no title at all",
            pull_request_id: Some("{id}"),
            value: None,
            repeat: 1,
            code: "TitleRequiredException",
            message: TITLE_REQUIRED_MESSAGE,
        },
        Refusal {
            // The member's minimum length is 0, so an empty title reaches the
            // handler rather than being refused at the boundary — and titles the
            // pull request with nothing either way.
            case: "an empty title",
            pull_request_id: Some("{id}"),
            value: Some(""),
            repeat: 1,
            code: "TitleRequiredException",
            message: TITLE_REQUIRED_MESSAGE,
        },
        Refusal {
            case: "a title of nothing but whitespace",
            pull_request_id: Some("{id}"),
            value: Some(" \t \n "),
            repeat: 1,
            code: "TitleRequiredException",
            message: TITLE_REQUIRED_MESSAGE,
        },
        Refusal {
            case: "a title one character past the length the member admits",
            pull_request_id: Some("{id}"),
            value: Some("t"),
            repeat: TITLE_MAX_CHARACTERS + 1,
            code: "InvalidTitleException",
            message: INVALID_TITLE_MESSAGE,
        },
    ];

    for refusal in ID_REFUSALS.iter().chain(own.iter()) {
        assert_refused(&opened, "UpdatePullRequestTitle", "title", refusal).await;
    }
}

#[tokio::test]
async fn an_id_naming_no_pull_request_is_refused_with_the_fixed_message() {
    let opened = opened("title-unknown-id").await;
    // An id no pull request was ever minted as, built from the stored one so it is
    // addressable yet names nothing.
    let unknown = format!("{}0", opened.pull_request_id);
    for accepted in accepted_titles() {
        let case = accepted.case;
        let (code, message) = error_with_message(
            &opened.router,
            "UpdatePullRequestTitle",
            &json!({ "pullRequestId": unknown, "title": accepted.value() }),
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
