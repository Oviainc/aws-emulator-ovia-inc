// SPDX-License-Identifier: CC0-1.0

//! The paging contract of `ListConnectInstanceIntegrations`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test, which the suite in
//! `connect_instance_integrations.rs` does not pin:
//!
//! * `maxResults` is accepted exactly over the range the model declares for it
//!   (`MaxResults`, range 1..50, from
//!   `smithy_query(query="operation", operation="ListConnectInstanceIntegrations")`)
//!   and caps the page at that many summaries; a value outside that range is refused
//!   with HTTP 400 `ValidationException`;
//! * `nextToken` is a cursor this service issued: any other string is refused with
//!   HTTP 400 `ValidationException`, whether it is within the modelled length bound
//!   (`NextToken`, len ..1000) or past it;
//! * a refused page changes nothing — the integrations the instance holds, and the
//!   page a well-formed call reads back, are the same before and after;
//! * a cursor already handed out stays usable when the record it names is deleted
//!   between two pages: the page after it is everything stored after that record.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};
use super::connect_instance_integrations::{list_integrations, onboard, put_integration, summaries};

/// The region every case here signs for; nothing in an integration record is
/// region-dependent — the ARNs it holds are the caller's — so one is enough.
const REGION: &str = "us-east-1";

/// The bounds the model declares for `maxResults` (range 1..50, from
/// `smithy_query(query="operation", operation="ListConnectInstanceIntegrations")`).
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 50;

/// The longest `nextToken` the model admits (`NextToken`, len ..1000).
const NEXT_TOKEN_MAX_LEN: usize = 1000;

/// `count` Lambda integrations of `instance_id`, each under a distinct function ARN,
/// put in order — so the stored order is the order this function put them in.
async fn seed(router: &MultiServiceRouter, instance_id: &str, count: usize) -> Vec<Value> {
    let configs: Vec<Value> = (0..count)
        .map(|n| {
            json!({"lambda": {
                "functionArn":
                    format!("arn:aws:lambda:us-east-1:000000000000:function:paging-handler-{n:02}"),
            }})
        })
        .collect();
    for config in &configs {
        let (status, body) = put_integration(router, instance_id, config).await;
        assert_eq!(
            status, 200,
            "case \"seed {config}\": expected HTTP 200 from the put, got {status} {body}"
        );
    }
    configs
}

/// `DeleteConnectInstanceIntegration` for the Lambda integration of `function_arn`.
async fn delete_lambda_integration(
    router: &MultiServiceRouter,
    instance_id: &str,
    function_arn: &str,
) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/connect-instance/{instance_id}/integrations/delete"),
        &json!({"integrationIdentifier": {"lambda": {"functionArn": function_arn}}}),
    )
    .await
}

#[tokio::test]
async fn max_results_is_accepted_exactly_over_its_modelled_range() {
    let router = router();
    let instance_id = "paging-max-results";
    onboard(&router, instance_id).await;
    let stored = seed(&router, instance_id, 4).await;

    // Each row is a `maxResults` the model admits: both published bounds and the
    // sizes between them that span the stored set — smaller than it, equal to it and
    // larger than it. The page is the first `maxResults` records in put order.
    let accepted: Vec<usize> = vec![
        MAX_RESULTS_MIN,
        2,
        stored.len(),
        stored.len() + 1,
        MAX_RESULTS_MAX,
    ];
    for page_size in accepted {
        let case = format!("maxResults={page_size}");
        let (status, body) =
            list_integrations(&router, instance_id, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for a maxResults inside the modelled range, \
             got {status} {body}"
        );
        let expected: Vec<Value> = stored
            .iter()
            .take(page_size.min(stored.len()))
            .cloned()
            .collect();
        assert_eq!(
            summaries(&body),
            expected,
            "case {case:?}: expected the first {} integrations in put order, got {body}",
            expected.len()
        );
        // A page that did not exhaust the stored set hands out a cursor; one that did
        // carries none, which is how a client knows to stop.
        let cursor = body.get("nextToken").and_then(Value::as_str);
        assert_eq!(
            cursor.is_some(),
            page_size < stored.len(),
            "case {case:?}: expected a cursor only for a page smaller than the stored \
             set of {}, got {body}",
            stored.len()
        );
    }

    // Each row is a `maxResults` outside that range — one below the minimum, one
    // above the maximum, far above it, and negative — and each is a bad request.
    for page_size in [
        (MAX_RESULTS_MIN as i64) - 1,
        (MAX_RESULTS_MAX as i64) + 1,
        1_000,
        -1,
    ] {
        let case = format!("maxResults={page_size}");
        let (status, body) =
            list_integrations(&router, instance_id, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a maxResults outside the modelled range, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );

        // The refusal reported nothing and changed nothing: the stored set reads back
        // whole.
        let (status, after) = list_integrations(&router, instance_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the list after the refusal, \
             got {status} {after}"
        );
        assert_eq!(
            summaries(&after),
            stored,
            "case {case:?}: a refused page must change nothing, got {after}"
        );
    }
}

#[tokio::test]
async fn only_a_cursor_this_service_issued_resumes_a_page() {
    let router = router();
    let instance_id = "paging-cursors";
    onboard(&router, instance_id).await;
    let stored = seed(&router, instance_id, 3).await;

    // Each row is a `nextToken` this service did not issue: a word, a decimal
    // position, hex that decodes to something that is not a sequence number, an empty
    // token, the longest token the model admits, and one character past that bound.
    let cases: Vec<(&str, String)> = vec![
        ("a word", "next-page".to_string()),
        ("a decimal position", "1".to_string()),
        ("hex of a non-number", ::hex::encode("one")),
        ("hex of a negative number", ::hex::encode("-1")),
        ("an empty token", String::new()),
        (
            "the longest token the model admits",
            "a".repeat(NEXT_TOKEN_MAX_LEN),
        ),
        (
            "one character past the modelled length",
            "a".repeat(NEXT_TOKEN_MAX_LEN + 1),
        ),
    ];

    for (case, token) in cases {
        let (status, body) =
            list_integrations(&router, instance_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a cursor this service did not issue, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );

        let (status, after) = list_integrations(&router, instance_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the list after the refusal, \
             got {status} {after}"
        );
        assert_eq!(
            summaries(&after),
            stored,
            "case {case:?}: a refused page must change nothing, got {after}"
        );
    }
}

#[tokio::test]
async fn a_cursor_survives_the_deletion_of_the_record_it_names() {
    let router = router();
    let instance_id = "paging-cursor-after-delete";
    onboard(&router, instance_id).await;
    let stored = seed(&router, instance_id, 5).await;

    // Take a cursor off a first page, so it names the record that page ended at.
    let (status, body) = list_integrations(&router, instance_id, "?maxResults=2").await;
    assert_eq!(
        status, 200,
        "case \"first page of two\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        stored[..2].to_vec(),
        "case \"first page of two\": expected the first two in put order, got {body}"
    );
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"first page of two\": expected a cursor, got {body}"))
        .to_string();

    // Delete the record the cursor names. The cursor is a position in the stored
    // order, not a reference to that record, so it stays usable.
    let ended_at = stored[1]["lambda"]["functionArn"]
        .as_str()
        .expect("the seeded config carries a function ARN")
        .to_string();
    let (status, body) = delete_lambda_integration(&router, instance_id, &ended_at).await;
    assert_eq!(
        status, 200,
        "case \"delete the record the cursor names\": expected HTTP 200, got {status} {body}"
    );

    let (status, body) =
        list_integrations(&router, instance_id, &format!("?maxResults=2&nextToken={token}")).await;
    assert_eq!(
        status, 200,
        "case \"page after the deleted record\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        stored[2..4].to_vec(),
        "case \"page after the deleted record\": expected the page to resume after the \
         deleted record, got {body}"
    );

    // And the rest of the walk still reaches the last record, with no cursor on the
    // final page.
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case \"page after the deleted record\": expected a cursor, got {body}")
        })
        .to_string();
    let (status, body) =
        list_integrations(&router, instance_id, &format!("?maxResults=2&nextToken={token}")).await;
    assert_eq!(
        status, 200,
        "case \"last page after the deleted record\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        summaries(&body),
        stored[4..].to_vec(),
        "case \"last page after the deleted record\": expected the last record, got {body}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "case \"last page after the deleted record\": the last page carries no cursor, \
         got {body}"
    );
}
