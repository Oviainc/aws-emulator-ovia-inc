// SPDX-License-Identifier: CC0-1.0

//! A pull request's event log over the wire: `DescribePullRequestEvents`, driven as
//! awsJson1_1 requests through the router that serves the real emulator (the wire
//! harness comes from the sibling `repository_lifecycle` suite, and calling as a
//! NAMED principal from `crate::wire`, rather than a second copy of either).
//!
//! The rule under test: every change made to a pull request appends ONE entry to
//! that pull request's log — the instant it happened, the identity that made it and
//! the metadata of that kind of change — and the operation reports the stored log
//! oldest first, narrowed by `pullRequestEventType` and `actorArn` and paged by
//! `maxResults`/`nextToken`. A read of the log changes nothing a later caller sees.
//!
//! The tables below span what that rule generalises over: the KIND of change and the
//! operation that made it (opening a pull request, a commit written to its source
//! branch through `PutFile` and through `CreateCommit`, an approval rule created on
//! it and deleted from it, the merge that closed it), the identity that made each
//! one, every value of `PullRequestEventType` as a filter — including the ones no
//! change here produced — every actor as a filter, and page sizes from one entry to
//! past the whole log, with each published `maxResults` bound and one past it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, ok, string_at};
use crate::wire::{caller_arn, ok_as};

/// The status every error asserted here models (`errorKind: client`, `httpError:
/// 400` on each of these error shapes).
const CLIENT_ERROR_STATUS: u16 = 400;

const REPOSITORY: &str = "pull-request-events";

/// The two branches the pull request is between: `main` took the first commit, so
/// it is the repository's default branch and the destination; `feature` is the
/// source.
const DESTINATION: &str = "main";
const SOURCE: &str = "feature";

/// The principals the changes below are made by. A caller is identified by the
/// access key id its request is signed with, so an event's `actorArn` is that
/// caller's ARN.
const AUTHOR: &str = "AKIDAUTHOR";
const PUSHER: &str = "AKIDPUSHER";
const REVIEWER: &str = "AKIDREVIEWER";
const MERGER: &str = "AKIDMERGER";
/// A principal that makes no change at all, so no event names it.
const BYSTANDER: &str = "AKIDBYSTANDER";

/// Every value of `PullRequestEventType`
/// (`smithy_query(query="shape",shape="PullRequestEventType")`), which is the
/// catalog `pullRequestEventType` narrows the log by.
const EVENT_TYPES: &[&str] = &[
    "PULL_REQUEST_CREATED",
    "PULL_REQUEST_STATUS_CHANGED",
    "PULL_REQUEST_SOURCE_REFERENCE_UPDATED",
    "PULL_REQUEST_MERGE_STATE_CHANGED",
    "PULL_REQUEST_APPROVAL_RULE_CREATED",
    "PULL_REQUEST_APPROVAL_RULE_UPDATED",
    "PULL_REQUEST_APPROVAL_RULE_DELETED",
    "PULL_REQUEST_APPROVAL_RULE_OVERRIDDEN",
    "PULL_REQUEST_APPROVAL_STATE_CHANGED",
];

/// Every metadata member a `PullRequestEvent` can carry
/// (`smithy_query(query="shape",shape="PullRequestEvent")`): an event carries the
/// one belonging to its own type and no other.
const METADATA_MEMBERS: &[&str] = &[
    "pullRequestCreatedEventMetadata",
    "pullRequestStatusChangedEventMetadata",
    "pullRequestSourceReferenceUpdatedEventMetadata",
    "pullRequestMergedStateChangedEventMetadata",
    "approvalRuleEventMetadata",
    "approvalStateChangedEventMetadata",
    "approvalRuleOverriddenEventMetadata",
];

/// `MergeOptionTypeEnum`'s value for a three-way merge, and the
/// `PullRequestStatusEnum` value a merged pull request holds
/// (`smithy_query(query="shape",shape="MergeOptionTypeEnum")`,
/// `smithy_query(query="shape",shape="PullRequestStatusEnum")`).
const THREE_WAY: &str = "THREE_WAY_MERGE";
const STATUS_CLOSED: &str = "CLOSED";

/// The schema version of an approval rule document and the only statement type its
/// schema defines, as the CodeCommit User Guide documents them.
const DOCUMENT_VERSION: &str = "2018-11-08";
const STATEMENT_TYPE: &str = "Approvers";

/// The largest `maxResults` `DescribePullRequestEvents` accepts, which is also the
/// page it serves when the member is left out: "The default is 100 events, which is
/// also the maximum number of events that can be returned in a result."
const MAX_PAGE: usize = 100;

/// One entry the log must hold: the change that produced it, the type it is
/// reported under, the caller it is attributed to, the metadata member it carries,
/// and the values inside that member (by JSON pointer, so a nested member is
/// pinned the same way a flat one is).
struct ExpectedEvent {
    case: &'static str,
    event_type: &'static str,
    actor: String,
    metadata: &'static str,
    fields: Vec<(&'static str, Value)>,
}

/// An emulator whose one pull request has been changed in every way an implemented
/// operation can change it, and the log those changes must have left behind.
struct History {
    router: MultiServiceRouter,
    pull_request_id: String,
    /// The tip `feature` holds after the last change, so a further push can name it
    /// as its parent.
    source_tip: String,
    expected: Vec<ExpectedEvent>,
}

/// An approval rule document, as compact JSON text.
fn document(approvals: u32) -> String {
    serde_json::to_string(&json!({
        "Version": DOCUMENT_VERSION,
        "Statements": [{
            "Type": STATEMENT_TYPE,
            "NumberOfApprovalsNeeded": approvals,
        }],
    }))
    .expect("a JSON document")
}

/// Write one commit on `branch` through `PutFile`, as `caller`, reporting its id.
async fn put_file(
    router: &MultiServiceRouter,
    caller: &str,
    branch: &str,
    parent: Option<&str>,
    content: &[u8],
) -> String {
    let mut body = json!({
        "repositoryName": REPOSITORY,
        "branchName": branch,
        "filePath": "README.md",
        "fileContent": base64(content),
        "commitMessage": "a revision",
    });
    if let Some(parent) = parent {
        body["parentCommitId"] = json!(parent);
    }
    string_at(&ok_as(router, caller, "PutFile", &body).await, "/commitId")
}

/// The events and continuation token one `DescribePullRequestEvents` call answers.
async fn describe(router: &MultiServiceRouter, body: &Value) -> (Vec<Value>, Option<String>) {
    let payload = ok(router, "DescribePullRequestEvents", body).await;
    let events = payload
        .get("pullRequestEvents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("no pullRequestEvents in {payload}"));
    let next_token = payload
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (events, next_token)
}

/// The event types the log reports for a request, in order.
fn types_of(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            event
                .get("pullRequestEventType")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("no pullRequestEventType in {event}"))
                .to_string()
        })
        .collect()
}

/// The pull request `GetPullRequest` reports.
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

/// Make every change an implemented operation can make to one pull request, and
/// report what each of them must have appended to its log.
async fn history() -> History {
    let router = emulator();
    ok_as(
        &router,
        AUTHOR,
        "CreateRepository",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    let base = put_file(&router, AUTHOR, DESTINATION, None, b"alpha\n").await;
    ok_as(
        &router,
        AUTHOR,
        "CreateBranch",
        &json!({
            "repositoryName": REPOSITORY,
            "branchName": SOURCE,
            "commitId": base,
        }),
    )
    .await;
    // A commit on the source branch BEFORE the pull request exists: there is
    // nothing yet for it to be an event on, so the log starts at the next change.
    let source_first = put_file(&router, PUSHER, SOURCE, Some(&base), b"alpha\nbeta\n").await;

    let created = ok_as(
        &router,
        AUTHOR,
        "CreatePullRequest",
        &json!({
            "title": "a change to review",
            "targets": [{
                "repositoryName": REPOSITORY,
                "sourceReference": SOURCE,
                "destinationReference": DESTINATION,
            }],
        }),
    )
    .await;
    let pull_request_id = string_at(&created, "/pullRequest/pullRequestId");
    let mut expected = vec![ExpectedEvent {
        case: "the pull request was opened",
        event_type: "PULL_REQUEST_CREATED",
        actor: caller_arn(AUTHOR),
        metadata: "pullRequestCreatedEventMetadata",
        fields: vec![
            ("/repositoryName", json!(REPOSITORY)),
            ("/sourceCommitId", json!(source_first)),
            ("/destinationCommitId", json!(base)),
            ("/mergeBase", json!(base)),
        ],
    }];

    // A commit written to the source branch through `PutFile` …
    let source_second = put_file(
        &router,
        PUSHER,
        SOURCE,
        Some(&source_first),
        b"alpha\nbeta\ngamma\n",
    )
    .await;
    expected.push(ExpectedEvent {
        case: "a commit was written to the source branch through PutFile",
        event_type: "PULL_REQUEST_SOURCE_REFERENCE_UPDATED",
        actor: caller_arn(PUSHER),
        metadata: "pullRequestSourceReferenceUpdatedEventMetadata",
        fields: vec![
            ("/repositoryName", json!(REPOSITORY)),
            ("/beforeCommitId", json!(source_first)),
            ("/afterCommitId", json!(source_second)),
            ("/mergeBase", json!(base)),
        ],
    });

    // … and one written through `CreateCommit`: a different operation moving the
    // same reference, so it is the same kind of event.
    let source_third = string_at(
        &ok_as(
            &router,
            REVIEWER,
            "CreateCommit",
            &json!({
                "repositoryName": REPOSITORY,
                "branchName": SOURCE,
                "parentCommitId": source_second,
                "commitMessage": "notes for the reviewer",
                "putFiles": [{
                    "filePath": "notes.txt",
                    "fileContent": base64(b"read me first\n"),
                }],
            }),
        )
        .await,
        "/commitId",
    );
    expected.push(ExpectedEvent {
        case: "a commit was written to the source branch through CreateCommit",
        event_type: "PULL_REQUEST_SOURCE_REFERENCE_UPDATED",
        actor: caller_arn(REVIEWER),
        metadata: "pullRequestSourceReferenceUpdatedEventMetadata",
        fields: vec![
            ("/repositoryName", json!(REPOSITORY)),
            ("/beforeCommitId", json!(source_second)),
            ("/afterCommitId", json!(source_third)),
            ("/mergeBase", json!(base)),
        ],
    });

    let rule = ok_as(
        &router,
        REVIEWER,
        "CreatePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": "needs-one-approval",
            "approvalRuleContent": document(1),
        }),
    )
    .await;
    let rule_id = string_at(&rule, "/approvalRule/approvalRuleId");
    // The document the pull request HOLDS, which is the canonical form of the one
    // sent — so the event names the rule's own content rather than the request's.
    let rule_content = string_at(&rule, "/approvalRule/approvalRuleContent");
    let rule_fields = vec![
        ("/approvalRuleName", json!("needs-one-approval")),
        ("/approvalRuleId", json!(rule_id)),
        ("/approvalRuleContent", json!(rule_content)),
    ];
    expected.push(ExpectedEvent {
        case: "an approval rule was created on the pull request",
        event_type: "PULL_REQUEST_APPROVAL_RULE_CREATED",
        actor: caller_arn(REVIEWER),
        metadata: "approvalRuleEventMetadata",
        fields: rule_fields.clone(),
    });

    ok_as(
        &router,
        REVIEWER,
        "DeletePullRequestApprovalRule",
        &json!({
            "pullRequestId": pull_request_id,
            "approvalRuleName": "needs-one-approval",
        }),
    )
    .await;
    expected.push(ExpectedEvent {
        case: "that approval rule was deleted from the pull request",
        event_type: "PULL_REQUEST_APPROVAL_RULE_DELETED",
        actor: caller_arn(REVIEWER),
        metadata: "approvalRuleEventMetadata",
        fields: rule_fields,
    });

    ok_as(
        &router,
        MERGER,
        "MergePullRequestByThreeWay",
        &json!({
            "repositoryName": REPOSITORY,
            "pullRequestId": pull_request_id,
            "commitMessage": "merge the reviewed change",
            "authorName": "Merge Tester",
            "email": "merge.tester@example.com",
        }),
    )
    .await;
    expected.push(ExpectedEvent {
        case: "the pull request was merged",
        event_type: "PULL_REQUEST_MERGE_STATE_CHANGED",
        actor: caller_arn(MERGER),
        metadata: "pullRequestMergedStateChangedEventMetadata",
        fields: vec![
            ("/repositoryName", json!(REPOSITORY)),
            (
                "/destinationReference",
                json!(format!("refs/heads/{DESTINATION}")),
            ),
            ("/mergeMetadata/isMerged", json!(true)),
            ("/mergeMetadata/mergedBy", json!(caller_arn(MERGER))),
            ("/mergeMetadata/mergeOption", json!(THREE_WAY)),
        ],
    });
    expected.push(ExpectedEvent {
        case: "the merge closed the pull request",
        event_type: "PULL_REQUEST_STATUS_CHANGED",
        actor: caller_arn(MERGER),
        metadata: "pullRequestStatusChangedEventMetadata",
        fields: vec![("/pullRequestStatus", json!(STATUS_CLOSED))],
    });

    History {
        router,
        pull_request_id,
        source_tip: source_third,
        expected,
    }
}

#[tokio::test]
async fn every_change_to_a_pull_request_is_one_event_in_the_order_it_happened() {
    let history = history().await;
    let (events, next_token) = describe(
        &history.router,
        &json!({ "pullRequestId": history.pull_request_id }),
    )
    .await;

    assert_eq!(
        types_of(&events),
        history
            .expected
            .iter()
            .map(|expected| expected.event_type.to_string())
            .collect::<Vec<String>>(),
        "case {:?}: expected one event per change, oldest first, got {events:?}",
        "the whole log"
    );
    assert!(
        next_token.is_none(),
        "case {:?}: a log shorter than one page carries no token, got {next_token:?}",
        "the whole log"
    );

    let mut previous = f64::MIN;
    for (event, expected) in events.iter().zip(&history.expected) {
        let case = expected.case;
        assert_eq!(
            event.get("pullRequestId").and_then(Value::as_str),
            Some(history.pull_request_id.as_str()),
            "case {case:?}: expected the pull request's own id, got {event}"
        );
        assert_eq!(
            event.get("actorArn").and_then(Value::as_str),
            Some(expected.actor.as_str()),
            "case {case:?}: expected the caller that made the change, got {event}"
        );
        let date = event
            .get("eventDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: no numeric eventDate in {event}"));
        assert!(
            date >= previous,
            "case {case:?}: expected an event no earlier than {previous}, got {date}"
        );
        previous = date;

        let metadata = event
            .get(expected.metadata)
            .unwrap_or_else(|| panic!("case {case:?}: no {} in {event}", expected.metadata));
        for (pointer, value) in &expected.fields {
            assert_eq!(
                metadata.pointer(pointer),
                Some(value),
                "case {case:?}: expected {pointer} to be {value}, got {metadata}"
            );
        }
        for member in METADATA_MEMBERS
            .iter()
            .filter(|member| **member != expected.metadata)
        {
            assert!(
                event.get(member).is_none(),
                "case {case:?}: an event carries only its own type's metadata, got {member} in \
                 {event}"
            );
        }
    }
}

#[tokio::test]
async fn an_open_pull_request_follows_the_tip_of_its_source_branch() {
    let history = history().await;
    let merged = stored_pull_request(&history.router, &history.pull_request_id).await;
    let target = merged
        .pointer("/pullRequestTargets/0")
        .unwrap_or_else(|| panic!("no pullRequestTargets in {merged}"))
        .clone();
    assert_eq!(
        target.get("sourceCommit").and_then(Value::as_str),
        Some(history.source_tip.as_str()),
        "case {:?}: expected the stored target to hold the branch's tip, got {target}",
        "the source branch moved while the pull request was open"
    );

    // A pull request that is no longer open is a record of what it was merged at,
    // so a later commit on the same branch moves neither its target nor its log.
    let (before, _) = describe(
        &history.router,
        &json!({ "pullRequestId": history.pull_request_id }),
    )
    .await;
    put_file(
        &history.router,
        PUSHER,
        SOURCE,
        Some(&history.source_tip),
        b"alpha\nbeta\ngamma\ndelta\n",
    )
    .await;
    let (after, _) = describe(
        &history.router,
        &json!({ "pullRequestId": history.pull_request_id }),
    )
    .await;
    assert_eq!(
        after, before,
        "case {:?}: expected the log of a closed pull request to be untouched, got {after:?}",
        "a commit written after the pull request was closed"
    );
    let read = stored_pull_request(&history.router, &history.pull_request_id).await;
    assert_eq!(
        read.pointer("/pullRequestTargets/0/sourceCommit")
            .and_then(Value::as_str),
        Some(history.source_tip.as_str()),
        "case {:?}: expected the merged target to keep the tip it was merged at, got {read}",
        "a commit written after the pull request was closed"
    );
}

#[tokio::test]
async fn the_log_is_narrowed_by_event_type_and_by_actor() {
    let history = history().await;

    for event_type in EVENT_TYPES {
        let (events, next_token) = describe(
            &history.router,
            &json!({
                "pullRequestId": history.pull_request_id,
                "pullRequestEventType": event_type,
            }),
        )
        .await;
        let want: Vec<String> = history
            .expected
            .iter()
            .filter(|expected| expected.event_type == *event_type)
            .map(|expected| expected.event_type.to_string())
            .collect();
        assert_eq!(
            types_of(&events),
            want,
            "case {event_type:?}: expected only the events of that type, got {events:?}"
        );
        assert!(
            next_token.is_none(),
            "case {event_type:?}: expected no further page, got {next_token:?}"
        );
    }

    for caller in [AUTHOR, PUSHER, REVIEWER, MERGER, BYSTANDER] {
        let actor = caller_arn(caller);
        let (events, _) = describe(
            &history.router,
            &json!({
                "pullRequestId": history.pull_request_id,
                "actorArn": actor,
            }),
        )
        .await;
        let want: Vec<String> = history
            .expected
            .iter()
            .filter(|expected| expected.actor == actor)
            .map(|expected| expected.event_type.to_string())
            .collect();
        assert_eq!(
            types_of(&events),
            want,
            "case {caller:?}: expected only the changes that caller made, got {events:?}"
        );
    }

    // Both filters at once narrow by both: the reviewer wrote a commit on the
    // source branch as well as creating and deleting the approval rule.
    let reviewer = caller_arn(REVIEWER);
    let (events, _) = describe(
        &history.router,
        &json!({
            "pullRequestId": history.pull_request_id,
            "actorArn": reviewer,
            "pullRequestEventType": "PULL_REQUEST_SOURCE_REFERENCE_UPDATED",
        }),
    )
    .await;
    assert_eq!(
        types_of(&events),
        vec!["PULL_REQUEST_SOURCE_REFERENCE_UPDATED".to_string()],
        "case {:?}: expected the one event matching both filters, got {events:?}",
        "the reviewer's source reference update"
    );
}

#[tokio::test]
async fn a_page_is_at_most_max_results_and_the_token_resumes_after_it() {
    let history = history().await;
    let whole: Vec<String> = history
        .expected
        .iter()
        .map(|expected| expected.event_type.to_string())
        .collect();

    for page_size in [1usize, 2, 3, whole.len(), MAX_PAGE] {
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({
                "pullRequestId": history.pull_request_id,
                "maxResults": page_size,
            });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let (events, next_token) = describe(&history.router, &body).await;
            assert!(
                !events.is_empty() && events.len() <= page_size,
                "case {page_size}: expected between 1 and {page_size} events in a page, got {}",
                events.len()
            );
            seen.extend(events);
            assert!(
                seen.len() <= whole.len(),
                "case {page_size}: expected at most {} events in all, got {}",
                whole.len(),
                seen.len()
            );
            match next_token {
                Some(next_token) => token = Some(next_token),
                None => break,
            }
        }
        assert_eq!(
            types_of(&seen),
            whole,
            "case {page_size}: expected the pages to carry the whole log in order, got {seen:?}"
        );
    }
}

/// One refused request: what it names, and the code and message it must answer.
struct Refusal {
    case: &'static str,
    body: Value,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_request_the_log_cannot_be_read_for_is_refused_with_the_modeled_error() {
    let history = history().await;
    let id = history.pull_request_id.clone();
    let refusals = vec![
        Refusal {
            case: "a pull request id that is no id at all",
            body: json!({ "pullRequestId": "not-an-id" }),
            code: "InvalidPullRequestIdException",
            message: "The pull request ID is not valid. Make sure that you have provided the full \
                      ID and that the pull request is in the specified repository, and then try \
                      again.",
        },
        Refusal {
            case: "a blank pull request id",
            body: json!({ "pullRequestId": "   " }),
            code: "PullRequestIdRequiredException",
            message: "A pull request ID is required, but none was provided.",
        },
        Refusal {
            case: "a well-formed id no pull request holds",
            body: json!({ "pullRequestId": "987654" }),
            code: "PullRequestDoesNotExistException",
            message: "The pull request ID 987654 could not be found. Make sure that you have \
                      specified the correct repository name and pull request ID, and then try \
                      again.",
        },
        Refusal {
            case: "an event type the model does not declare",
            body: json!({
                "pullRequestId": id,
                "pullRequestEventType": "PULL_REQUEST_COMMENTED",
            }),
            code: "InvalidPullRequestEventTypeException",
            message: "The pull request event type is not valid.",
        },
        Refusal {
            case: "a declared event type in the wrong case",
            body: json!({
                "pullRequestId": id,
                "pullRequestEventType": "pull_request_created",
            }),
            code: "InvalidPullRequestEventTypeException",
            message: "The pull request event type is not valid.",
        },
        Refusal {
            case: "an actor that is not an ARN",
            body: json!({ "pullRequestId": id, "actorArn": "Mary_Major" }),
            code: "InvalidActorArnException",
            message: "The Amazon Resource Name (ARN) is not valid. Make sure that you have \
                      provided the full ARN for the user who initiated the change for the pull \
                      request, and then try again.",
        },
        Refusal {
            case: "an ARN whose account is not an account id",
            body: json!({ "pullRequestId": id, "actorArn": "arn:aws:iam::12345:user/Mary_Major" }),
            code: "InvalidActorArnException",
            message: "The Amazon Resource Name (ARN) is not valid. Make sure that you have \
                      provided the full ARN for the user who initiated the change for the pull \
                      request, and then try again.",
        },
        Refusal {
            case: "a page size below the published minimum",
            body: json!({ "pullRequestId": id, "maxResults": 0 }),
            code: "InvalidMaxResultsException",
            message: "A valid limit is between 1 and 100",
        },
        Refusal {
            case: "a page size one past the published maximum",
            body: json!({ "pullRequestId": id, "maxResults": 101 }),
            code: "InvalidMaxResultsException",
            message: "A valid limit is between 1 and 100",
        },
        Refusal {
            case: "a continuation token this service never issued",
            body: json!({ "pullRequestId": id, "nextToken": "not-a-token" }),
            code: "InvalidContinuationTokenException",
            message: "Error parsing next token: not-a-token",
        },
    ];

    for refusal in refusals {
        let case = refusal.case;
        let (code, message) = error_with_message(
            &history.router,
            "DescribePullRequestEvents",
            &refusal.body,
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, refusal.code,
            "case {case:?}: expected {}, got {code} saying {message:?}",
            refusal.code
        );
        assert_eq!(
            message, refusal.message,
            "case {case:?}: expected the sentence that code is worded with, got {message:?}"
        );
    }

    // Each published bound is served rather than refused.
    for page_size in [1usize, MAX_PAGE] {
        let (events, _) = describe(
            &history.router,
            &json!({ "pullRequestId": id, "maxResults": page_size }),
        )
        .await;
        assert!(
            !events.is_empty(),
            "case {page_size}: expected a published bound to be served, got {events:?}"
        );
    }
}

#[tokio::test]
async fn describing_the_events_changes_nothing_a_later_caller_sees() {
    let history = history().await;
    let body = json!({ "pullRequestId": history.pull_request_id });
    let before = stored_pull_request(&history.router, &history.pull_request_id).await;
    let (first, _) = describe(&history.router, &body).await;
    let (second, _) = describe(&history.router, &body).await;
    let after = stored_pull_request(&history.router, &history.pull_request_id).await;

    assert_eq!(
        first, second,
        "case {:?}: expected a second read to answer the first's events, got {second:?}",
        "the log read twice"
    );
    assert_eq!(
        before, after,
        "case {:?}: expected the pull request to be untouched by the reads, got {after}",
        "the log read twice"
    );
}
