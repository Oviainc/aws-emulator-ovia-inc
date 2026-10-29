// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListDeadLetterSourceQueues`, driven over the wire through the
//! shared harness in [`super::support`].
//!
//! The rule under test: **the listing of a dead-letter queue reports the url of every
//! queue whose CURRENT `RedrivePolicy` names that queue as its `deadLetterTargetArn`,
//! and of no other queue — in one stable order, paged by `MaxResults` with a `NextToken`
//! that continues the same listing.** The set is a projection of the queues' stored
//! attributes rather than a list anybody maintains, so however a link came about (an
//! attribute supplied at create, one set afterwards) and however it went away (the
//! policy re-pointed at another queue, the source queue deleted) the very next listing
//! reflects it.
//!
//! The tables span what the rule generalises over: the FORM the link took, the COUNT of
//! sources (none, one, several), the queue TYPE (standard and FIFO), the dead-letter
//! queue ADDRESSED (three of them, so a source of one is never reported for another),
//! and the page SIZE a caller asks for (each size against the same five sources,
//! including one past their count and none at all).

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    invalid_parameter_message_without_value, INVALID_NEXT_TOKEN_MESSAGE,
    LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE, QUEUE_DOES_NOT_EXIST_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    account, call, create_ok, emulator, error_code, expected_arn, expected_url, ok, HOST,
    OTHER_ACCOUNT,
};

/// The operation under test.
const OPERATION: &str = "ListDeadLetterSourceQueues";

/// The most pages one walk in this suite may take before it is called a loop: five
/// sources at one per page is five pages, so anything past this is a cursor that never
/// advances.
const PAGE_LIMIT: usize = 16;

/// A `RedrivePolicy` attribute value driving a queue's messages to `target_arn`, as the
/// attribute documents it ("the parameters for the dead-letter queue functionality of
/// the source queue as a JSON object").
fn policy(target_arn: &str) -> String {
    json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": "3"}).to_string()
}

/// The `queueUrls` of one listing reply, in the order it handed them over. A dead-letter
/// queue with no source queue reports none, which is the empty vector here.
fn source_urls(reply: &Value) -> Vec<String> {
    reply
        .get("queueUrls")
        .and_then(Value::as_array)
        .map(|urls| {
            urls.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Walk every page of the listing of the dead-letter queue at `url`, asking for
/// `max_results` entries per page, and answer the urls in the order the pages handed
/// them over together with the size of each page.
async fn walk(
    router: &MultiServiceRouter,
    url: &str,
    max_results: Option<i32>,
) -> (Vec<String>, Vec<usize>) {
    let mut urls: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"QueueUrl": url});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let reply = ok(router, OPERATION, request).await;
        let page = source_urls(&reply);
        pages.push(page.len());
        urls.extend(page);
        token = reply
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            return (urls, pages);
        }
        assert!(
            pages.len() < PAGE_LIMIT,
            "case walking {url} {max_results:?} per page: the cursor never ended, \
             got pages {pages:?}"
        );
    }
}

/// Every source queue of the dead-letter queue at `url`, in one unpaged listing.
async fn sources(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let reply = ok(router, OPERATION, json!({"QueueUrl": url})).await;
    assert_eq!(
        reply.get("NextToken"),
        None,
        "case listing {url} with no MaxResults: a listing that reached its end carries \
         no NextToken, got {reply}"
    );
    source_urls(&reply)
}

/// One step of the membership table: a request that changes which queues redrive where,
/// and the whole source listing each dead-letter queue must report afterwards.
struct Step {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// Dead-letter queue name -> the names of the source queues it must then report, in
    /// the order the listing hands them over.
    expected: Vec<(&'static str, Vec<&'static str>)>,
}

/// However a `RedrivePolicy` link came about and however it went away, the listing of
/// each dead-letter queue reports exactly the queues currently naming it — never a queue
/// naming another dead-letter queue and never one naming none.
#[tokio::test]
async fn the_listing_reports_exactly_the_queues_whose_policy_names_the_addressed_queue() {
    let router = emulator();
    // Three dead-letter queues, so a row can show a source belonging to one and not to
    // the others; the FIFO one is the same rule over the other queue type.
    for (name, attributes) in [
        ("dlq-primary", json!({})),
        ("dlq-other", json!({})),
        ("dlq-queue.fifo", json!({"FifoQueue": "true"})),
    ] {
        let _ = create_ok(&router, name, &attributes).await;
    }

    // The baseline, before any queue names any of them: a dead-letter queue nothing
    // redrives to reports no source queue at all.
    for dead_letter in ["dlq-primary", "dlq-other", "dlq-queue.fifo"] {
        let reported = sources(&router, &expected_url(dead_letter)).await;
        assert!(
            reported.is_empty(),
            "case a dead-letter queue nothing redrives to on {dead_letter}: expected no \
             source queue, got {reported:?}"
        );
    }

    let steps = vec![
        Step {
            case: "a source queue linked by the policy its create supplied",
            operation: "CreateQueue",
            request: json!({
                "QueueName": "source-b",
                "Attributes": {"RedrivePolicy": policy(&expected_arn("dlq-primary"))},
            }),
            expected: vec![
                ("dlq-primary", vec!["source-b"]),
                ("dlq-other", vec![]),
                ("dlq-queue.fifo", vec![]),
            ],
        },
        Step {
            case: "a queue created with no redrive policy at all",
            operation: "CreateQueue",
            request: json!({"QueueName": "plain"}),
            expected: vec![
                ("dlq-primary", vec!["source-b"]),
                ("dlq-other", vec![]),
                ("dlq-queue.fifo", vec![]),
            ],
        },
        Step {
            case: "a second source of the same dead-letter queue",
            operation: "CreateQueue",
            request: json!({
                "QueueName": "source-a",
                "Attributes": {"RedrivePolicy": policy(&expected_arn("dlq-primary"))},
            }),
            expected: vec![
                ("dlq-primary", vec!["source-a", "source-b"]),
                ("dlq-other", vec![]),
                ("dlq-queue.fifo", vec![]),
            ],
        },
        Step {
            case: "a link added after the create by SetQueueAttributes",
            operation: "SetQueueAttributes",
            request: json!({
                "QueueUrl": expected_url("plain"),
                "Attributes": {"RedrivePolicy": policy(&expected_arn("dlq-primary"))},
            }),
            expected: vec![
                ("dlq-primary", vec!["plain", "source-a", "source-b"]),
                ("dlq-other", vec![]),
                ("dlq-queue.fifo", vec![]),
            ],
        },
        Step {
            case: "a source re-pointed at another dead-letter queue",
            operation: "SetQueueAttributes",
            request: json!({
                "QueueUrl": expected_url("source-a"),
                "Attributes": {"RedrivePolicy": policy(&expected_arn("dlq-other"))},
            }),
            expected: vec![
                ("dlq-primary", vec!["plain", "source-b"]),
                ("dlq-other", vec!["source-a"]),
                ("dlq-queue.fifo", vec![]),
            ],
        },
        Step {
            case: "a FIFO source of a FIFO dead-letter queue",
            operation: "CreateQueue",
            request: json!({
                "QueueName": "source-fifo.fifo",
                "Attributes": {
                    "FifoQueue": "true",
                    "RedrivePolicy": policy(&expected_arn("dlq-queue.fifo")),
                },
            }),
            expected: vec![
                ("dlq-primary", vec!["plain", "source-b"]),
                ("dlq-other", vec!["source-a"]),
                ("dlq-queue.fifo", vec!["source-fifo.fifo"]),
            ],
        },
        Step {
            case: "a source queue that is deleted",
            operation: "DeleteQueue",
            request: json!({"QueueUrl": expected_url("source-b")}),
            expected: vec![
                ("dlq-primary", vec!["plain"]),
                ("dlq-other", vec!["source-a"]),
                ("dlq-queue.fifo", vec!["source-fifo.fifo"]),
            ],
        },
    ];

    for step in &steps {
        let _ = ok(&router, step.operation, step.request.clone()).await;
        for (dead_letter, expected_names) in &step.expected {
            let expected: Vec<String> = expected_names.iter().map(|name| expected_url(name)).collect();
            let reported = sources(&router, &expected_url(dead_letter)).await;
            assert_eq!(
                reported, expected,
                "case {} on {dead_letter}: expected {expected:?}, got {reported:?}",
                step.case
            );
        }
    }

    // A policy naming a queue nothing holds configures a redrive no delivery could ever
    // make, so the create carrying it is refused and no source queue comes into being for
    // any listing to report. (`redrive_policy_document` owns that refusal's own sentence;
    // here it is the listing that must be untouched by it.)
    let before: Vec<Vec<String>> = {
        let mut listings = Vec::new();
        for dead_letter in ["dlq-primary", "dlq-other", "dlq-queue.fifo"] {
            listings.push(sources(&router, &expected_url(dead_letter)).await);
        }
        listings
    };
    let (status, parsed) = call(
        &router,
        "CreateQueue",
        &json!({
            "QueueName": "source-orphan",
            "Attributes": {"RedrivePolicy": policy(&expected_arn("never-created"))},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidParameterValue"),
        "case a policy naming a queue that was never created: expected the create refused, \
         got {status} with {parsed}"
    );
    for (index, dead_letter) in ["dlq-primary", "dlq-other", "dlq-queue.fifo"]
        .iter()
        .enumerate()
    {
        let reported = sources(&router, &expected_url(dead_letter)).await;
        assert_eq!(
            Some(&reported),
            before.get(index),
            "case a policy naming a queue that was never created on {dead_letter}: a refused \
             create must leave the listing as it was, got {reported:?}"
        );
    }

    // A listing changes nothing a later caller sees: the same request repeated answers
    // the same set, and the sources are still exactly what they were.
    let before = sources(&router, &expected_url("dlq-primary")).await;
    let again = sources(&router, &expected_url("dlq-primary")).await;
    assert_eq!(
        again, before,
        "case listing twice: a read must not change the listing, got {again:?} after {before:?}"
    );
}

/// Every page size walks the same listing exactly once: the pages concatenate to the
/// whole set in the same order, each page but the last is full, and a `NextToken` comes
/// back only while entries remain.
#[tokio::test]
async fn every_page_size_walks_the_whole_listing_exactly_once() {
    let router = emulator();
    let dead_letter = create_ok(&router, "paged-dlq", &json!({})).await;
    let target = expected_arn("paged-dlq");
    // Five sources, so a page size below, at and above their count is each a different
    // row below.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..5 {
        let name = format!("paged-source-{index}");
        let url = create_ok(
            &router,
            &name,
            &json!({"RedrivePolicy": policy(&target)}),
        )
        .await;
        expected.push(url);
    }
    expected.sort();

    // `None` is the unparameterised request: "If you do not set MaxResults, the response
    // includes a maximum of 1,000 results", which is every source here, in one page.
    let sizes: Vec<Option<i32>> = vec![
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(5),
        Some(6),
        Some(LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE.0),
        Some(LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE.1),
    ];
    for size in sizes {
        let (walked, pages) = walk(&router, &dead_letter, size).await;
        assert_eq!(
            walked, expected,
            "case {size:?} per page: expected the whole listing {expected:?}, got {walked:?}"
        );
        let page_size = usize::try_from(size.unwrap_or(i32::MAX))
            .unwrap_or(usize::MAX)
            .max(1);
        let expected_pages: Vec<usize> = match expected.len() {
            0 => vec![0],
            total => {
                let mut sizes = vec![page_size; total / page_size];
                if total % page_size != 0 {
                    sizes.push(total % page_size);
                }
                sizes
            }
        };
        assert_eq!(
            pages, expected_pages,
            "case {size:?} per page: expected pages {expected_pages:?}, got {pages:?}"
        );
    }
}

/// One refused request and the answer it must get.
struct Refusal {
    case: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
    message: String,
}

/// A request this listing will not take is refused with the code and the whole message
/// the service builds for that class: a page size outside the published range and a
/// cursor no page handed out are parameter values it does not accept, and a url naming
/// no queue of this account is the absence every operation of this service reports the
/// same way.
#[tokio::test]
async fn a_request_the_listing_will_not_take_is_refused_with_its_own_message() {
    let router = emulator();
    let dead_letter = create_ok(&router, "refusing-dlq", &json!({})).await;
    let _ = create_ok(
        &router,
        "refusing-source",
        &json!({"RedrivePolicy": policy(&expected_arn("refusing-dlq"))}),
    )
    .await;

    let (min, max) = LIST_DEAD_LETTER_SOURCE_QUEUES_MAX_RESULTS_RANGE;
    // The refusal states the range this listing's `MaxResults` publishes, and names
    // neither the rejected value nor this queue.
    let range = format!("MaxResults must be an integer between {min} and {max}.");
    let rows = vec![
        Refusal {
            case: "a page size asking for no result at all",
            request: json!({"QueueUrl": dead_letter, "MaxResults": min - 1}),
            status: 400,
            code: "InvalidParameterValue",
            message: invalid_parameter_message_without_value("MaxResults", &range),
        },
        Refusal {
            case: "a page size one past the published maximum",
            request: json!({"QueueUrl": dead_letter, "MaxResults": max + 1}),
            status: 400,
            code: "InvalidParameterValue",
            message: invalid_parameter_message_without_value("MaxResults", &range),
        },
        Refusal {
            case: "a cursor that is not one this listing issues",
            request: json!({"QueueUrl": dead_letter, "NextToken": "not-a-cursor"}),
            status: 400,
            code: "InvalidParameterValue",
            message: INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        },
        Refusal {
            case: "a cursor past the end of the listing it claims to continue",
            request: json!({"QueueUrl": dead_letter, "NextToken": "9"}),
            status: 400,
            code: "InvalidParameterValue",
            message: INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        },
        Refusal {
            case: "a url of a queue that was never created",
            request: json!({"QueueUrl": expected_url("never-created")}),
            status: 400,
            code: "QueueDoesNotExist",
            message: QUEUE_DOES_NOT_EXIST_MESSAGE.to_string(),
        },
        Refusal {
            case: "another account's queue of a name this account has",
            request: json!({"QueueUrl": format!("https://{HOST}/{OTHER_ACCOUNT}/refusing-dlq")}),
            status: 400,
            code: "QueueDoesNotExist",
            message: QUEUE_DOES_NOT_EXIST_MESSAGE.to_string(),
        },
        Refusal {
            case: "a value that is not a queue url at all",
            request: json!({"QueueUrl": "refusing-dlq"}),
            status: 400,
            code: "QueueDoesNotExist",
            message: QUEUE_DOES_NOT_EXIST_MESSAGE.to_string(),
        },
    ];

    assert_ne!(
        OTHER_ACCOUNT,
        account(),
        "case the other account: the row above needs an account that is not this emulator's"
    );

    for row in &rows {
        let (status, parsed) = call(&router, OPERATION, &row.request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (row.status, row.code),
            "case {}: expected {} {}, got {status} with {parsed}",
            row.case,
            row.status,
            row.code
        );
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("<no message>");
        assert_eq!(
            message, row.message,
            "case {}: expected the service's own message, got {message:?}",
            row.case
        );
    }

    // Each bound of the published range is accepted, so the two refusals above are about
    // the values being outside it.
    for accepted in [min, max] {
        let reply = ok(
            &router,
            OPERATION,
            json!({"QueueUrl": dead_letter, "MaxResults": accepted}),
        )
        .await;
        assert_eq!(
            source_urls(&reply),
            vec![expected_url("refusing-source")],
            "case MaxResults {accepted}: expected the one source queue, got {reply}"
        );
    }
}
