// SPDX-License-Identifier: CC0-1.0

//! `ListQuotaShares`, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a listing reports every quota share stored under the job queue the request names and
//!   none of another queue's, one `QuotaShareDetail` per share read from the persisted
//!   record (the same values the share's own reader answers) in one stable order; a queue
//!   that holds none answers an empty list rather than a refusal;
//! * the `jobQueue` the listing hangs off is resolved by NAME or by ARN, and both forms
//!   answer the same page;
//! * paging with `maxResults`/`nextToken` sees every share exactly once in the unpaged
//!   order, `maxResults` is accepted on both published bounds and refused one past either,
//!   and a request that names none is still bounded at the documented default;
//! * a request this operation cannot answer is the modelled `ClientException` (HTTP 400):
//!   a `jobQueue` that resolves to no stored queue, a `maxResults` outside the published
//!   bounds, and a `nextToken` that resumes nothing — which carries the detail Batch words
//!   an unusable token with, echoing nothing of the token itself;
//! * a listing is a READ: it reports a share's `CREATING` status while the clock still owes
//!   the move to `VALID`, it answers identically twice at one instant, and it follows the
//!   writes.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, compute_environment, error_code, error_message, job_queue, Batch};

/// `ListQuotaShares`' route (`smithy_query(query="operation",
/// operation="ListQuotaShares")`: POST /v1/listquotashares).
const LIST: &str = "/v1/listquotashares";

/// `CreateQuotaShare`' route.
const CREATE: &str = "/v1/createquotashare";

/// `DescribeQuotaShare`' route — the reader that owns one share's details.
const DESCRIBE: &str = "/v1/describequotashare";

/// The members `QuotaShareDetail` models (`smithy_query(query="shape",
/// shape="QuotaShareDetail")`), so a listing is checked to invent nothing beyond them.
const DETAIL_MEMBERS: &[&str] = &[
    "quotaShareName",
    "quotaShareArn",
    "jobQueueArn",
    "capacityLimits",
    "resourceSharingConfiguration",
    "preemptionConfiguration",
    "state",
    "status",
];

/// How long to wait for a clock-owed transition to fall due: the span the clock owes it
/// ([`state::JOB_PHASE_MILLIS`]), with margin for a slow machine.
async fn wait_for_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// One quota share this suite lists: the create body it is created with, on the queue it
/// hangs off.
struct Share {
    /// What the row varies: named in every assertion.
    label: String,
    name: String,
    queue: &'static str,
    /// The members laid over the base create body.
    extra: Value,
}

/// The queue the listings under test hang off.
const QUEUE: &str = "jq-listed-shares";

/// A second queue, whose shares a listing of [`QUEUE`] must not report.
const OTHER_QUEUE: &str = "jq-other-shares";

/// The table this suite lists: rows differ in the shape, case and length of the share
/// name, in the sharing strategy and whether it carries a `borrowLimit`, in how many
/// capacity limits the share allocates, in whether `state` is named at all (its absence
/// defaults to `ENABLED`), in the preemption state, in whether tags are carried (which the
/// detail does not model) and in WHICH queue the share hangs off.
fn shares() -> Vec<Share> {
    vec![
        Share {
            label: "RESERVE, one capacity limit, no state member".to_string(),
            name: "qs-reserve".to_string(),
            queue: QUEUE,
            extra: json!({}),
        },
        Share {
            label: "LEND, two capacity limits, DISABLED, mixed-case name".to_string(),
            name: "QS_Lend-2".to_string(),
            queue: QUEUE,
            extra: json!({
                "state": "DISABLED",
                "resourceSharingConfiguration": {"strategy": "LEND"},
                "capacityLimits": [
                    {"maxCapacity": 1, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 7, "capacityUnit": "ml.c5.xlarge"},
                ],
            }),
        },
        Share {
            label: "LEND_AND_BORROW with a borrowLimit, preemption ENABLED, tags, \
                    single-character name"
                .to_string(),
            name: "z".to_string(),
            queue: QUEUE,
            extra: json!({
                "state": "ENABLED",
                "resourceSharingConfiguration": {"strategy": "LEND_AND_BORROW", "borrowLimit": 200},
                "preemptionConfiguration": {"inSharePreemption": "ENABLED"},
                "tags": {"team": "research", "cost-centre": "42"},
                "capacityLimits": [
                    {"maxCapacity": 0, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 3, "capacityUnit": "ml.m5.2xlarge"},
                    {"maxCapacity": 9, "capacityUnit": "ml.p4d.24xlarge"},
                ],
            }),
        },
        Share {
            label: "a name at the documented length bound".to_string(),
            name: "q".repeat(state::MAX_QUOTA_SHARE_NAME_LEN),
            queue: QUEUE,
            extra: json!({}),
        },
        Share {
            label: "a share of ANOTHER queue".to_string(),
            name: "qs-elsewhere".to_string(),
            queue: OTHER_QUEUE,
            extra: json!({}),
        },
    ]
}

/// The body of a `CreateQuotaShare` for `name` on `queue`, with `extra` laid over it.
fn create_body(name: &str, queue: &str, extra: &Value) -> Value {
    let mut body = json!({
        "quotaShareName": name,
        "jobQueue": queue,
        "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
        "resourceSharingConfiguration": {"strategy": "RESERVE"},
        "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

/// The ARN a share of `name` on `queue` is stored under.
fn share_arn(queue: &str, name: &str) -> String {
    arn(&format!("job-queue/{queue}/quota-share/{name}"))
}

/// Create both queues of the fixture, each over a compute environment of its own.
async fn queues(batch: &Batch) {
    for queue in [QUEUE, OTHER_QUEUE] {
        let environment = compute_environment(batch, &format!("ce-for-{queue}")).await;
        job_queue(batch, queue, &environment).await;
    }
}

/// Create every row of [`shares`] over the wire.
async fn fixture(batch: &Batch) {
    queues(batch).await;
    for share in shares() {
        batch
            .ok(CREATE, create_body(&share.name, share.queue, &share.extra))
            .await;
    }
}

/// `ListQuotaShares` over the wire: the status and the decoded body.
async fn list(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call(LIST, body).await
}

/// `ListQuotaShares` over the wire, requiring the 200.
async fn listed(batch: &Batch, case: &str, body: Value) -> Value {
    let (status, decoded) = list(batch, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected ListQuotaShares to answer 200, got {status} {decoded}"
    );
    decoded
}

/// The details of a listing, in the order it answered them.
fn details(body: &Value) -> Vec<Value> {
    body["quotaShares"].as_array().cloned().unwrap_or_default()
}

/// The `quotaShareArn`s of a listing, in the order it answered them.
fn arns(body: &Value) -> Vec<String> {
    details(body)
        .iter()
        .map(|detail| {
            detail["quotaShareArn"]
                .as_str()
                .unwrap_or("<no quotaShareArn>")
                .to_string()
        })
        .collect()
}

/// The `nextToken` of a listing, if it carries one.
fn token_of(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The ARNs of the [`shares`] of `queue`, in the order a listing must answer them: the
/// store keys a share by its ARN, which nests the share's name under its queue's, so the
/// share name is the listing's stable order.
fn stored_order(queue: &str) -> Vec<String> {
    let mut rows: Vec<Share> = shares().into_iter().filter(|row| row.queue == queue).collect();
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows.iter()
        .map(|row| share_arn(queue, &row.name))
        .collect()
}

/// RULE: a listing reports every quota share stored under the queue the request names and
/// none of another queue's, each detail carrying the persisted record and no member
/// `QuotaShareDetail` does not model; a queue that holds none answers an empty list.
#[tokio::test]
async fn a_listing_reports_every_share_of_the_named_queue() {
    let batch = Batch::new();
    queues(&batch).await;

    // Nothing stored yet: the listing is EMPTY, never a refusal.
    let empty = listed(&batch, "a queue with no share", json!({"jobQueue": QUEUE})).await;
    assert_eq!(
        arns(&empty),
        Vec::<String>::new(),
        "case a queue with no share: expected an empty page, got {empty}"
    );
    assert!(
        token_of(&empty).is_none(),
        "case a queue with no share: a page that ends the results carries no nextToken, got \
         {empty}"
    );

    // Each create adds exactly one row to ITS queue's listing, and every earlier row stays
    // listed.
    let mut expected: Vec<String> = Vec::new();
    for share in shares() {
        batch
            .ok(CREATE, create_body(&share.name, share.queue, &share.extra))
            .await;
        if share.queue == QUEUE {
            expected.push(share_arn(QUEUE, &share.name));
            expected.sort();
        }
        let case = format!("after creating {:?} ({})", share.name, share.label);
        let body = listed(&batch, &case, json!({"jobQueue": QUEUE})).await;
        assert_eq!(
            arns(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
        // A read: the same request at the same instant answers identically.
        let again = listed(&batch, &case, json!({"jobQueue": QUEUE})).await;
        assert_eq!(
            arns(&again),
            arns(&body),
            "case {case}: listing twice must answer the same shares, got {again} then {body}"
        );
    }

    // The other queue reports only its own share.
    let other = listed(&batch, "the other queue", json!({"jobQueue": OTHER_QUEUE})).await;
    assert_eq!(
        arns(&other),
        stored_order(OTHER_QUEUE),
        "case the other queue: expected only its own share, got {other}"
    );

    // Every detail is the record the share's own reader answers, and nothing beyond the
    // modelled members.
    let body = listed(&batch, "every detail", json!({"jobQueue": QUEUE})).await;
    for detail in details(&body) {
        let case = detail["quotaShareArn"]
            .as_str()
            .unwrap_or("<no quotaShareArn>")
            .to_string();
        let object = detail
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a detail object, got {detail}"));
        for member in object.keys() {
            assert!(
                DETAIL_MEMBERS.contains(&member.as_str()),
                "case {case}: {member} is not a member QuotaShareDetail models, got {detail}"
            );
        }
        let described = batch
            .ok(DESCRIBE, json!({"quotaShareArn": case.clone()}))
            .await;
        for member in object.keys() {
            assert_eq!(
                detail.get(member),
                described.get(member),
                "case {case}: the listing must report the reader's {member}, got {detail} and \
                 {described}"
            );
        }
    }
}

/// RULE: the `jobQueue` a listing hangs off is resolved by NAME or by ARN, and both forms
/// answer the same page.
#[tokio::test]
async fn a_listing_resolves_its_queue_by_name_or_by_arn() {
    let batch = Batch::new();
    fixture(&batch).await;

    for queue in [QUEUE, OTHER_QUEUE] {
        let by_name = listed(&batch, queue, json!({"jobQueue": queue})).await;
        let reference = arn(&format!("job-queue/{queue}"));
        let by_arn = listed(&batch, queue, json!({"jobQueue": reference.clone()})).await;
        assert_eq!(
            arns(&by_name),
            stored_order(queue),
            "case {queue} by name: expected {:?}, got {by_name}",
            stored_order(queue)
        );
        assert_eq!(
            arns(&by_arn),
            arns(&by_name),
            "case {queue} by ARN {reference}: a name and its ARN name one queue, got {by_arn} \
             and {by_name}"
        );
    }
}

/// RULE: paging with `maxResults`/`nextToken` sees every share exactly once in the unpaged
/// order, `maxResults` is accepted on both published bounds, and a request that names none
/// is bounded at the documented default.
#[tokio::test]
async fn paging_sees_every_quota_share_exactly_once() {
    let batch = Batch::new();
    queues(&batch).await;

    // Enough shares that a small page cannot hold them, named so the name order is the
    // index order.
    let total = 6;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        let name = format!("qs-paged-{index:02}");
        batch.ok(CREATE, create_body(&name, QUEUE, &json!({}))).await;
        expected.push(share_arn(QUEUE, &name));
    }

    // A request that names no page size answers them all, bounded at the documented
    // default ("returns up to 100 results"), which this fixture is inside.
    let unpaged = listed(&batch, "no maxResults", json!({"jobQueue": QUEUE})).await;
    assert_eq!(
        arns(&unpaged),
        expected,
        "case no maxResults: expected every created share in the stored order, got {unpaged}"
    );
    assert!(
        token_of(&unpaged).is_none(),
        "case no maxResults: a page that ends the results carries no nextToken, got {unpaged}"
    );

    // Page sizes differ: one at a time, a size that does not divide the total, the exact
    // total, and both published bounds of `maxResults` (1 and 100).
    for page_size in [
        state::MIN_LIST_QUOTA_SHARES_RESULTS,
        4,
        total,
        state::MAX_LIST_QUOTA_SHARES_RESULTS,
    ] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"jobQueue": QUEUE, "maxResults": page_size});
            if let Some(resume) = token.as_deref() {
                request["nextToken"] = json!(resume);
            }
            let body = listed(&batch, &case, request).await;
            let page = arns(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page holds at most maxResults shares, got {body}"
            );
            seen.extend(page);
            token = token_of(&body);
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case}: paging did not terminate after {pages} pages, last token {token:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must see every share exactly once in the unpaged order, got \
             {seen:?}"
        );
    }
}

/// RULE: a request this operation cannot answer is the modelled `ClientException` (HTTP
/// 400): an unknown `jobQueue`, a `maxResults` outside the published 1..100, and a
/// `nextToken` that resumes nothing.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_answer() {
    let batch = Batch::new();
    fixture(&batch).await;

    // Rows differ in WHICH part of the request is unusable; each must be the modelled 400,
    // never a 5xx or a 501.
    let cases: Vec<(String, Value)> = vec![
        (
            "a job queue name nothing holds".to_string(),
            json!({"jobQueue": "jq-no-such-queue"}),
        ),
        (
            "a job queue ARN nothing holds".to_string(),
            json!({"jobQueue": arn("job-queue/jq-no-such-queue")}),
        ),
        (
            "a jobQueue that is not a reference to anything".to_string(),
            json!({"jobQueue": "not-a-queue-at-all"}),
        ),
        (
            "maxResults one below the published minimum".to_string(),
            json!({"jobQueue": QUEUE, "maxResults": state::MIN_LIST_QUOTA_SHARES_RESULTS - 1}),
        ),
        (
            "maxResults one past the published maximum".to_string(),
            json!({"jobQueue": QUEUE, "maxResults": state::MAX_LIST_QUOTA_SHARES_RESULTS + 1}),
        ),
        (
            "a negative maxResults".to_string(),
            json!({"jobQueue": QUEUE, "maxResults": -1}),
        ),
        (
            "a nextToken that resumes nothing".to_string(),
            json!({"jobQueue": QUEUE, "maxResults": 1, "nextToken": "not-a-token"}),
        ),
        (
            "a nextToken past the end of the results".to_string(),
            json!({"jobQueue": QUEUE, "nextToken": "9999"}),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = list(&batch, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException with HTTP 400, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected ClientException, got {body} ({})",
            error_message(&body)
        );
    }

    // Both published bounds of `maxResults` are ACCEPTED, so the refusals above are the
    // bound and not the member.
    for accepted in [
        state::MIN_LIST_QUOTA_SHARES_RESULTS,
        state::MAX_LIST_QUOTA_SHARES_RESULTS,
    ] {
        let case = format!("maxResults {accepted}");
        listed(
            &batch,
            &case,
            json!({"jobQueue": QUEUE, "maxResults": accepted}),
        )
        .await;
    }

    // A `nextToken` this service cannot resume carries the detail Batch words an unusable
    // token with, whatever the token was: the message states the failure and echoes no
    // value of the request.
    for token in ["not-a-token", "", "-1", "9999", "0.5"] {
        let (status, body) = list(&batch, json!({"jobQueue": QUEUE, "nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case nextToken {token:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            state::NEXT_TOKEN_NOT_PROCESSABLE,
            "case nextToken {token:?}: expected the unusable-token detail, got {body}"
        );
    }
}

/// RULE: a listing is a READ — it reports the `CREATING` status while the clock still owes
/// the move to `VALID`, never advances it itself, and follows the writes.
#[tokio::test]
async fn a_listing_reports_the_status_the_clock_owes_and_follows_the_writes() {
    let batch = Batch::new();
    queues(&batch).await;

    for share in shares().into_iter().filter(|row| row.queue == QUEUE) {
        let case = share.label.clone();
        batch
            .ok(CREATE, create_body(&share.name, share.queue, &share.extra))
            .await;
        let fresh = listed(&batch, &case, json!({"jobQueue": QUEUE})).await;
        let listed_share = details(&fresh)
            .into_iter()
            .find(|detail| {
                detail["quotaShareName"].as_str() == Some(share.name.as_str())
            })
            .unwrap_or_else(|| panic!("case {case}: the created share is listed, got {fresh}"));
        // The span is never zero, so the transient status is observable.
        assert_eq!(
            listed_share["status"].as_str(),
            Some(state::STATUS_CREATING),
            "case {case}: a fresh share is listed in the non-terminal status, got {listed_share}"
        );
        // The state the create asked for (or the documented default) is reported as stored.
        let requested_state = share.extra["state"]
            .as_str()
            .unwrap_or(state::STATE_ENABLED)
            .to_string();
        assert_eq!(
            listed_share["state"].as_str(),
            Some(requested_state.as_str()),
            "case {case}: expected the stored state {requested_state}, got {listed_share}"
        );
    }

    // Once the clock owes the move, the listing reports it — and two listings at one
    // instant answer identically.
    wait_for_the_clock().await;
    let settled = listed(&batch, "after the clock", json!({"jobQueue": QUEUE})).await;
    for detail in details(&settled) {
        assert_eq!(
            detail["status"].as_str(),
            Some(state::STATUS_VALID),
            "case after the clock: expected the settled status, got {detail}"
        );
    }
    let again = listed(&batch, "after the clock", json!({"jobQueue": QUEUE})).await;
    assert_eq!(
        again, settled,
        "case after the clock: a listing must not change what a later listing sees, got {again} \
         then {settled}"
    );
}
