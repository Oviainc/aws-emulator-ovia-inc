// SPDX-License-Identifier: CC0-1.0

//! `UpdatePullRequestStatus` over the wire, driven as awsJson1_1 requests through
//! the router that serves the real emulator (the wire harness comes from the sibling
//! `repository_lifecycle` suite, and calling as a NAMED principal from
//! `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: a pull request's `pullRequestStatus` admits ONLY the moves
//! its model documents — to the status it already holds, or from `OPEN` to
//! `CLOSED` — so a request naming the status already held changes nothing, a request
//! closing an open pull request persists `CLOSED` and appends one
//! `PULL_REQUEST_STATUS_CHANGED` entry to its event log attributed to the calling
//! identity, and any other move is refused and persists nothing. Closing is not
//! merging: the targets' merge metadata and the pull request's `revisionId` are
//! untouched by a status move.
//!
//! The tables below span what that rule generalises over: every ordered pair of
//! `PullRequestStatusEnum` values as a transition (including the two that are no
//! moves at all), each repeated so an accepted no-op is proven idempotent, and every
//! `pullRequestId` and `pullRequestStatus` value the operation models an error for —
//! absent, empty, mis-cased, outside the catalog and naming no pull request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{error_with_message, PULL_REQUEST_DOES_NOT_EXIST};
use crate::repository_lifecycle::{base64, ok, string_at};
use crate::wire::{caller_arn, emulator_with_callers, ok_as, refusal_as};

/// Every value of `PullRequestStatusEnum`
/// (`smithy_query(query="shape",shape="PullRequestStatusEnum")`), which is the
/// catalog a transition is between.
const STATUSES: &[&str] = &["OPEN", "CLOSED"];

const STATUS_OPEN: &str = "OPEN";
const STATUS_CLOSED: &str = "CLOSED";

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The `PullRequestEventType` a status move is reported under
/// (`smithy_query(query="shape",shape="PullRequestEventType")`).
const STATUS_CHANGED: &str = "PULL_REQUEST_STATUS_CHANGED";

const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// The principals the calls below are made by: a pull request's author, and a
/// different identity that moves its status, so an event's `actorArn` can only come
/// from the call that made the change.
const AUTHOR: &str = "AKIDAUTHOR";
const CLOSER: &str = "AKIDCLOSER";

/// The message `InvalidPullRequestStatusUpdateException` carries: the sentence pair
/// the model documents for that shape.
const INVALID_UPDATE_MESSAGE: &str =
    "The pull request status update is not valid. The only valid update is from OPEN to CLOSED.";

/// The message `InvalidPullRequestStatusException` carries, likewise the shape's own
/// documented sentence pair.
const INVALID_STATUS_MESSAGE: &str =
    "The pull request status is not valid. The only valid values are OPEN and CLOSED.";

/// The message `PullRequestStatusRequiredException` carries.
const STATUS_REQUIRED_MESSAGE: &str = "A pull request status is required, but none was provided.";

/// The message `PullRequestIdRequiredException` carries, for a request that named
/// no `pullRequestId` member at all.
const ID_REQUIRED_MESSAGE: &str = "A pull request ID is required, but none was provided.";

/// The message `InvalidPullRequestIdException` carries for each id the rows below
/// name: the value the request carried, in the sentence CodeCommit words that error
/// with, and the clause that says so where the value holds no characters.
const EMPTY_ID_MESSAGE: &str =
    "The following pull request ID is empty or null and therefore not valid: . To view a list of \
     valid pull request IDs for this repository, use ListPullRequests.";
const INVALID_ID_MESSAGE: &str =
    "The following pull request ID is not valid: not-an-id. To view a list of valid pull request \
     IDs for this repository, use ListPullRequests.";
const SPACED_ID_MESSAGE: &str =
    "The following pull request ID is not valid:  1 . To view a list of valid pull request IDs \
     for this repository, use ListPullRequests.";

/// One stored pull request, and the router that serves it.
struct Opened {
    router: MultiServiceRouter,
    repository: String,
    pull_request_id: String,
}

/// Open one pull request between two branches of a fresh repository, as [`AUTHOR`].
async fn opened(repository: &str) -> Opened {
    let (router, _engine) = emulator_with_callers(&[AUTHOR, CLOSER]);
    ok_as(
        &router,
        AUTHOR,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    // The first commit on `main` makes it the repository's default branch, so a pull
    // request can be opened against it.
    let base = ok_as(
        &router,
        AUTHOR,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": DESTINATION,
            "commitMessage": "base",
            "putFiles": [{ "filePath": "README.md", "fileContent": base64(b"alpha\n") }],
        }),
    )
    .await;
    let base = string_at(&base, "/commitId");
    ok_as(
        &router,
        AUTHOR,
        "CreateBranch",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    ok_as(
        &router,
        AUTHOR,
        "CreateCommit",
        &json!({
            "repositoryName": repository,
            "branchName": SOURCE,
            "parentCommitId": base,
            "commitMessage": "a change",
            "putFiles": [{ "filePath": "README.md", "fileContent": base64(b"alpha\nbeta\n") }],
        }),
    )
    .await;
    let created = ok_as(
        &router,
        AUTHOR,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": repository,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    Opened {
        router,
        repository: repository.to_string(),
        pull_request_id,
    }
}

/// The pull request `GetPullRequest` reports — the reader every write below is
/// proven through.
async fn stored_pull_request(router: &MultiServiceRouter, pull_request_id: &str) -> Value {
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

/// The entries `DescribePullRequestEvents` reports for a pull request, oldest first.
async fn events(router: &MultiServiceRouter, pull_request_id: &str) -> Vec<Value> {
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

/// One `UpdatePullRequestStatus` call as [`CLOSER`], answering the status the
/// operation reported for the pull request.
async fn update(opened: &Opened, status: &str) -> Value {
    let answered = ok_as(
        &opened.router,
        CLOSER,
        "UpdatePullRequestStatus",
        &json!({
            "pullRequestId": opened.pull_request_id,
            "pullRequestStatus": status,
        }),
    )
    .await;
    answered
        .get("pullRequest")
        .unwrap_or_else(|| panic!("no pullRequest in {answered}"))
        .clone()
}

/// A string member of a payload, or `None` where the payload carries none.
fn member(payload: &Value, pointer: &str) -> Option<String> {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One transition of the status: the value the pull request holds when the call is
/// made, the value the call asks for, and what must come of it.
struct Transition {
    case: &'static str,
    /// Whether the pull request is closed before the asserted call is made.
    start_closed: bool,
    requested: &'static str,
    /// The error code the call is refused with, or `None` where it is accepted.
    refused_as: Option<(&'static str, &'static str)>,
    /// The status the pull request holds afterwards.
    resulting: &'static str,
    /// Whether the asserted call appended an entry to the event log.
    appends_event: bool,
}

/// Every ordered pair of `PullRequestStatusEnum` values, which is the whole space
/// the documented rule ("the only valid operations are to update the status from
/// OPEN to OPEN, OPEN to CLOSED or from CLOSED to CLOSED") speaks about.
const TRANSITIONS: &[Transition] = &[
    Transition {
        case: "OPEN to CLOSED closes the pull request",
        start_closed: false,
        requested: STATUS_CLOSED,
        refused_as: None,
        resulting: STATUS_CLOSED,
        appends_event: true,
    },
    Transition {
        case: "OPEN to OPEN is accepted and changes nothing",
        start_closed: false,
        requested: STATUS_OPEN,
        refused_as: None,
        resulting: STATUS_OPEN,
        appends_event: false,
    },
    Transition {
        case: "CLOSED to CLOSED is accepted and changes nothing",
        start_closed: true,
        requested: STATUS_CLOSED,
        refused_as: None,
        resulting: STATUS_CLOSED,
        appends_event: false,
    },
    Transition {
        case: "CLOSED to OPEN is no valid update",
        start_closed: true,
        requested: STATUS_OPEN,
        refused_as: Some((
            "InvalidPullRequestStatusUpdateException",
            INVALID_UPDATE_MESSAGE,
        )),
        resulting: STATUS_CLOSED,
        appends_event: false,
    },
];

#[tokio::test]
async fn every_transition_of_the_status_follows_the_documented_rule() {
    for (row, transition) in TRANSITIONS.iter().enumerate() {
        let case = transition.case;
        let opened = opened(&format!("status-transition-{row}")).await;
        if transition.start_closed {
            update(&opened, STATUS_CLOSED).await;
        }

        let before = stored_pull_request(&opened.router, &opened.pull_request_id).await;
        let events_before = events(&opened.router, &opened.pull_request_id).await;
        let revision_before = member(&before, "/revisionId");
        let activity_before = before.pointer("/lastActivityDate").cloned();

        match transition.refused_as {
            None => {
                let answered = update(&opened, transition.requested).await;
                assert_eq!(
                    member(&answered, "/pullRequestStatus").as_deref(),
                    Some(transition.resulting),
                    "case {case:?}: the answered pullRequestStatus must be \
                     {:?}, got {:?}",
                    transition.resulting,
                    member(&answered, "/pullRequestStatus")
                );
                // The answer is the stored record, so the reader must report the
                // same pull request the write answered with.
                let read = stored_pull_request(&opened.router, &opened.pull_request_id).await;
                assert_eq!(
                    read, answered,
                    "case {case:?}: GetPullRequest must report exactly what the update \
                     answered, got {read} for {answered}"
                );
            }
            Some((code, message)) => {
                let (got_code, got_message) = refusal_as(
                    &opened.router,
                    CLOSER,
                    "UpdatePullRequestStatus",
                    &json!({
                        "pullRequestId": opened.pull_request_id,
                        "pullRequestStatus": transition.requested,
                    }),
                    CLIENT_ERROR_STATUS,
                )
                .await;
                assert_eq!(
                    got_code, code,
                    "case {case:?}: expected error {code}, got {got_code} saying \
                     {got_message:?}"
                );
                assert_eq!(
                    got_message, message,
                    "case {case:?}: expected the documented message, got {got_message:?}"
                );
            }
        }

        let after = stored_pull_request(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            member(&after, "/pullRequestStatus").as_deref(),
            Some(transition.resulting),
            "case {case:?}: the stored pullRequestStatus must be {:?}, got {:?}",
            transition.resulting,
            member(&after, "/pullRequestStatus")
        );
        // A status move is not a revision of the reviewable state, and closing a
        // pull request is not merging it: neither member moves with the status.
        assert_eq!(
            member(&after, "/revisionId"),
            revision_before,
            "case {case:?}: the revisionId must not move with the status, got {:?} for {:?}",
            member(&after, "/revisionId"),
            revision_before
        );
        assert_eq!(
            after.pointer("/pullRequestTargets/0/mergeMetadata/isMerged"),
            Some(&json!(false)),
            "case {case:?}: a status move must not mark the target merged, got {:?}",
            after.pointer("/pullRequestTargets/0/mergeMetadata")
        );

        let events_after = events(&opened.router, &opened.pull_request_id).await;
        let appended = events_after.len() - events_before.len();
        assert_eq!(
            appended,
            usize::from(transition.appends_event),
            "case {case:?}: expected {} appended event(s), got {appended} ({events_after:?})",
            usize::from(transition.appends_event)
        );
        if transition.appends_event {
            let entry = events_after
                .last()
                .unwrap_or_else(|| panic!("case {case:?}: an appended event"));
            assert_eq!(
                member(entry, "/pullRequestEventType").as_deref(),
                Some(STATUS_CHANGED),
                "case {case:?}: expected a {STATUS_CHANGED} entry, got {entry}"
            );
            assert_eq!(
                member(entry, "/actorArn"),
                Some(caller_arn(CLOSER)),
                "case {case:?}: the entry must name the caller that moved the status, got {:?}",
                member(entry, "/actorArn")
            );
            assert_eq!(
                member(
                    entry,
                    "/pullRequestStatusChangedEventMetadata/pullRequestStatus"
                )
                .as_deref(),
                Some(transition.resulting),
                "case {case:?}: the entry must carry the new status, got {entry}"
            );
            assert_eq!(
                member(entry, "/pullRequestId").as_deref(),
                Some(opened.pull_request_id.as_str()),
                "case {case:?}: the entry must name its pull request, got {entry}"
            );
            // An entry in the log IS activity on the pull request, so
            // `lastActivityDate` moved onto this call.
            assert_eq!(
                after.pointer("/lastActivityDate"),
                entry.pointer("/eventDate"),
                "case {case:?}: lastActivityDate must be the appended event's instant, got \
                 {:?} for {:?}",
                after.pointer("/lastActivityDate"),
                entry.pointer("/eventDate")
            );
        } else {
            assert_eq!(
                after.pointer("/lastActivityDate"),
                activity_before.as_ref(),
                "case {case:?}: a call that changed nothing must not move lastActivityDate, \
                 got {:?} for {:?}",
                after.pointer("/lastActivityDate"),
                activity_before
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_status_is_idempotent_however_often_it_is_asked_for() {
    // Every value of the catalog, asked for repeatedly from the state it names: the
    // second and third call must leave the pull request exactly as the first did.
    for status in STATUSES {
        let opened = opened(&format!("status-idempotent-{}", status.to_lowercase())).await;
        if *status == STATUS_CLOSED {
            update(&opened, STATUS_CLOSED).await;
        }
        let settled = stored_pull_request(&opened.router, &opened.pull_request_id).await;
        let log = events(&opened.router, &opened.pull_request_id).await;
        for repeat in 0..3 {
            let answered = update(&opened, status).await;
            assert_eq!(
                answered, settled,
                "case {status:?} repeat {repeat}: the pull request must be unchanged, got \
                 {answered} for {settled}"
            );
            let after = events(&opened.router, &opened.pull_request_id).await;
            assert_eq!(
                after, log,
                "case {status:?} repeat {repeat}: the event log must be unchanged, got \
                 {after:?} for {log:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_closed_pull_request_leaves_the_open_listing() {
    let opened = opened("status-listing").await;
    let listed = |status: &'static str| {
        let router = &opened.router;
        let repository = opened.repository.clone();
        async move {
            let payload = ok(
                router,
                "ListPullRequests",
                &json!({ "repositoryName": repository, "pullRequestStatus": status }),
            )
            .await;
            payload
                .get("pullRequestIds")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| panic!("no pullRequestIds in {payload}"))
        }
    };

    assert_eq!(
        listed(STATUS_OPEN).await,
        vec![json!(opened.pull_request_id)],
        "case {STATUS_OPEN:?} before the close: the open listing must hold the pull request"
    );
    assert!(
        listed(STATUS_CLOSED).await.is_empty(),
        "case {STATUS_CLOSED:?} before the close: the closed listing must be empty, got {:?}",
        listed(STATUS_CLOSED).await
    );

    update(&opened, STATUS_CLOSED).await;

    assert!(
        listed(STATUS_OPEN).await.is_empty(),
        "case {STATUS_OPEN:?} after the close: the open listing must be empty, got {:?}",
        listed(STATUS_OPEN).await
    );
    assert_eq!(
        listed(STATUS_CLOSED).await,
        vec![json!(opened.pull_request_id)],
        "case {STATUS_CLOSED:?} after the close: the closed listing must hold the pull request"
    );
}

/// One request the operation refuses, and the error it names for it.
struct Refusal {
    case: &'static str,
    /// The `pullRequestId` the request names, or `None` to leave the member out.
    pull_request_id: Option<&'static str>,
    /// The `pullRequestStatus` the request names, or `None` to leave it out.
    status: Option<&'static str>,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_refused_request_names_the_member_at_fault_and_persists_nothing() {
    let opened = opened("status-refusals").await;
    // `{id}` stands for the stored pull request's id, so a row can name a request
    // whose id is addressable and whose status is not.
    let refusals = &[
        Refusal {
            case: "no pullRequestId at all",
            pull_request_id: None,
            status: Some(STATUS_CLOSED),
            code: "PullRequestIdRequiredException",
            message: ID_REQUIRED_MESSAGE,
        },
        Refusal {
            case: "an empty pullRequestId",
            pull_request_id: Some(""),
            status: Some(STATUS_CLOSED),
            code: "InvalidPullRequestIdException",
            message: EMPTY_ID_MESSAGE,
        },
        Refusal {
            case: "a pullRequestId that is not a decimal id",
            pull_request_id: Some("not-an-id"),
            status: Some(STATUS_CLOSED),
            code: "InvalidPullRequestIdException",
            message: INVALID_ID_MESSAGE,
        },
        Refusal {
            case: "a pullRequestId carrying surrounding space",
            pull_request_id: Some(" 1 "),
            status: Some(STATUS_CLOSED),
            code: "InvalidPullRequestIdException",
            message: SPACED_ID_MESSAGE,
        },
        Refusal {
            case: "no pullRequestStatus at all",
            pull_request_id: Some("{id}"),
            status: None,
            code: "PullRequestStatusRequiredException",
            message: STATUS_REQUIRED_MESSAGE,
        },
        Refusal {
            case: "an empty pullRequestStatus",
            pull_request_id: Some("{id}"),
            status: Some(""),
            code: "InvalidPullRequestStatusException",
            message: INVALID_STATUS_MESSAGE,
        },
        Refusal {
            case: "a pullRequestStatus in the wrong case",
            pull_request_id: Some("{id}"),
            status: Some("open"),
            code: "InvalidPullRequestStatusException",
            message: INVALID_STATUS_MESSAGE,
        },
        Refusal {
            case: "a pullRequestStatus outside the catalog",
            pull_request_id: Some("{id}"),
            status: Some("MERGED"),
            code: "InvalidPullRequestStatusException",
            message: INVALID_STATUS_MESSAGE,
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let mut body = json!({});
        if let Some(id) = refusal.pull_request_id {
            let id = if id == "{id}" {
                opened.pull_request_id.clone()
            } else {
                id.to_string()
            };
            body["pullRequestId"] = json!(id);
        }
        if let Some(status) = refusal.status {
            body["pullRequestStatus"] = json!(status);
        }
        let (code, message) = error_with_message(
            &opened.router,
            "UpdatePullRequestStatus",
            &body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, refusal.code,
            "case {case:?}: expected error {}, got {code} saying {message:?}",
            refusal.code
        );
        assert_eq!(
            message, refusal.message,
            "case {case:?}: expected the documented message, got {message:?}"
        );
        let after = stored_pull_request(&opened.router, &opened.pull_request_id).await;
        assert_eq!(
            member(&after, "/pullRequestStatus").as_deref(),
            Some(STATUS_OPEN),
            "case {case:?}: a refused request must leave the pull request open, got {:?}",
            member(&after, "/pullRequestStatus")
        );
    }
}

#[tokio::test]
async fn an_id_naming_no_pull_request_is_refused_with_the_fixed_message() {
    let opened = opened("status-unknown-id").await;
    // An id no pull request was ever minted as, built from the stored one so it is
    // addressable yet names nothing.
    let unknown = format!("{}0", opened.pull_request_id);
    for status in STATUSES {
        let (code, message) = error_with_message(
            &opened.router,
            "UpdatePullRequestStatus",
            &json!({ "pullRequestId": unknown, "pullRequestStatus": status }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, "PullRequestDoesNotExistException",
            "case {status:?}: expected PullRequestDoesNotExistException, got {code} saying \
             {message:?}"
        );
        assert_eq!(
            message, PULL_REQUEST_DOES_NOT_EXIST,
            "case {status:?}: expected the message CodeCommit words that code with, got \
             {message:?}"
        );
    }
}
