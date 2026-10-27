// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a paged listing of this service refuses a `NextToken` it did
//! not issue, driven over the wire through the shared harness in [`super::support`].
//!
//! The rule under test: **a pagination token a paged listing did not issue — one that
//! does not parse as a cursor, or one that no longer continues anything — is refused at
//! HTTP 400 with `InvalidParameterValue` and ONE fixed sentence, "Invalid or expired next
//! token.", which names no parameter and echoes no part of the request.** A paginating
//! client recognises that sentence to restart a listing from its first page, so the
//! wording is part of the contract; and because nothing the caller supplied comes back,
//! a log or a comparison on the message cannot key on a token value the service never
//! issued.
//!
//! The tables span what the rule generalises over: the OPERATION (both listings this
//! service models as `paginated`), the FORM the rejected token took (not a number, empty,
//! signed, fractional, whitespace-padded, hexadecimal, over-long, a count past the end of
//! the listing) and HOW it came to continue nothing (forged outright, or issued by a
//! listing that has since shrunk — the "expired" leg). The sibling page-size refusal is
//! driven alongside, because it uses the OTHER template of the same family: it names its
//! parameter, so the two must not be collapsed into one wording.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    invalid_parameter_message_without_value, INVALID_NEXT_TOKEN_MESSAGE,
    LIST_QUEUES_MAX_RESULTS_RANGE,
};

use super::support::{call, create_ok, emulator, error_code, expected_arn, expected_url, ok, redrives_to};

/// Every listing this service models as paginated, from
/// `smithy_query(query="operation", operation="ListQueues,ListDeadLetterSourceQueues",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`
/// — the two operations carrying
/// `paginated {inputToken: NextToken, outputToken: NextToken}`. The token rule is the
/// same for both, so every row below is driven through each of them.
const PAGINATED_LISTINGS: &[&str] = &["ListQueues", "ListDeadLetterSourceQueues"];

/// The sentence Amazon SQS answers an unissued or expired pagination token with, written
/// out here rather than taken from the service, so this suite pins the WORDING and not
/// merely the fact that the service is self-consistent.
const EXPECTED_TOKEN_REFUSAL: &str = "Invalid or expired next token.";

/// The request each paged listing takes before the token under test is added:
/// `ListDeadLetterSourceQueues` is addressed at one queue, `ListQueues` at the region.
fn base_request(operation: &str, dead_letter_url: &str) -> Value {
    match operation {
        "ListDeadLetterSourceQueues" => json!({"QueueUrl": dead_letter_url}),
        _ => json!({}),
    }
}

/// The message of a refusal, however the wire names the member.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The urls one listing reply handed over, under whichever member the operation carries
/// them in (`QueueUrls` for the queue listing, `queueUrls` for the source listing).
fn listed_urls(reply: &Value) -> Vec<String> {
    reply
        .get("QueueUrls")
        .or_else(|| reply.get("queueUrls"))
        .and_then(Value::as_array)
        .map(|urls| {
            urls.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A token no page of either listing could have handed out is refused with the one fixed
/// sentence, whatever form it took and whichever listing it was presented to.
#[tokio::test]
async fn a_token_no_page_issued_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    // A dead-letter queue with two sources, so both listings hold more than nothing and a
    // cursor past their end is genuinely past it.
    let dead_letter = create_ok(&router, "paging-dlq", &json!({})).await;
    let dead_letter_arn = expected_arn("paging-dlq");
    for index in 0..2 {
        redrives_to(&router, &format!("paging-source-{index}"), &dead_letter_arn).await;
    }

    let over_long = "t".repeat(512);
    // `(case, the token the request carries)`
    let rows: Vec<(&str, &str)> = vec![
        ("not a number at all", "not-a-token"),
        ("an empty token", ""),
        ("a negative count", "-1"),
        ("a fractional count", "1.5"),
        ("a count with leading whitespace", " 1"),
        ("a count with trailing whitespace", "1 "),
        ("a hexadecimal count", "0x1"),
        ("a count far past the end of the listing", "99999"),
        ("an over-long opaque token", &over_long),
    ];

    for operation in PAGINATED_LISTINGS {
        for (case, token) in &rows {
            let mut request = base_request(operation, &dead_letter);
            request["NextToken"] = json!(token);
            let (status, parsed) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameterValue"),
                "case {operation} {case}: expected 400 InvalidParameterValue, \
                 got {status} with {parsed}"
            );
            let reported = message(&parsed);
            assert_eq!(
                reported, EXPECTED_TOKEN_REFUSAL,
                "case {operation} {case}: expected {EXPECTED_TOKEN_REFUSAL:?}, \
                 got {reported:?}"
            );
            assert!(
                token.is_empty() || !reported.contains(*token),
                "case {operation} {case}: the refusal must echo no part of the request, \
                 got {reported:?}"
            );
        }
    }
}

/// The service's own constant is that sentence, so every listing that refuses a token
/// answers the wording this suite pins.
#[test]
fn the_service_words_the_token_refusal_as_the_fixed_sentence() {
    assert_eq!(
        INVALID_NEXT_TOKEN_MESSAGE, EXPECTED_TOKEN_REFUSAL,
        "case the shared wording: expected {EXPECTED_TOKEN_REFUSAL:?}, \
         got {INVALID_NEXT_TOKEN_MESSAGE:?}"
    );
}

/// A token that DID continue a listing stops continuing one once the listing has shrunk
/// past it, and is then refused exactly as a forged one is — the "expired" half of the
/// sentence — while the same token is honoured for as long as the listing still reaches
/// it.
#[tokio::test]
async fn a_token_whose_listing_has_since_shrunk_is_refused_the_same_way() {
    let router = emulator();

    // The queue listing: three queues under one prefix, walked one per page.
    let names: Vec<String> = (0..3).map(|index| format!("shrink-{index}")).collect();
    for name in &names {
        create_ok(&router, name, &json!({})).await;
    }
    let paging = json!({"QueueNamePrefix": "shrink-", "MaxResults": 1});
    let first = ok(&router, "ListQueues", paging.clone()).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case ListQueues first page: expected a NextToken in {first}"))
        .to_string();

    let mut resumed = paging.clone();
    resumed["NextToken"] = json!(token);
    let second = ok(&router, "ListQueues", resumed.clone()).await;
    assert_eq!(
        listed_urls(&second),
        vec![expected_url(&names[1])],
        "case ListQueues resumed while the listing still reaches the cursor: got {second}"
    );

    for name in &names {
        ok(
            &router,
            "DeleteQueue",
            json!({"QueueUrl": expected_url(name)}),
        )
        .await;
    }
    let (status, parsed) = call(&router, "ListQueues", &resumed).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidParameterValue"),
        "case ListQueues resumed after the listing emptied: expected a refusal, \
         got {status} with {parsed}"
    );
    assert_eq!(
        message(&parsed),
        EXPECTED_TOKEN_REFUSAL,
        "case ListQueues resumed after the listing emptied: got {parsed}"
    );

    // The source listing: the same walk, shrunk by re-pointing the policies away.
    let dead_letter = create_ok(&router, "shrink-dlq", &json!({})).await;
    let dead_letter_arn = expected_arn("shrink-dlq");
    let sources: Vec<String> = (0..2)
        .map(|index| format!("shrink-source-{index}"))
        .collect();
    for name in &sources {
        redrives_to(&router, name, &dead_letter_arn).await;
    }
    let paging = json!({"QueueUrl": dead_letter, "MaxResults": 1});
    let first = ok(&router, "ListDeadLetterSourceQueues", paging.clone()).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case ListDeadLetterSourceQueues first page: expected a NextToken in {first}")
        })
        .to_string();

    let mut resumed = paging.clone();
    resumed["NextToken"] = json!(token);
    let second = ok(&router, "ListDeadLetterSourceQueues", resumed.clone()).await;
    assert_eq!(
        listed_urls(&second),
        vec![expected_url(&sources[1])],
        "case ListDeadLetterSourceQueues resumed while the listing still reaches the \
         cursor: got {second}"
    );

    for name in &sources {
        ok(
            &router,
            "DeleteQueue",
            json!({"QueueUrl": expected_url(name)}),
        )
        .await;
    }
    let (status, parsed) = call(&router, "ListDeadLetterSourceQueues", &resumed).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "InvalidParameterValue"),
        "case ListDeadLetterSourceQueues resumed after the listing emptied: expected a \
         refusal, got {status} with {parsed}"
    );
    assert_eq!(
        message(&parsed),
        EXPECTED_TOKEN_REFUSAL,
        "case ListDeadLetterSourceQueues resumed after the listing emptied: got {parsed}"
    );
}

/// The sibling page-size refusal keeps its OWN template — it names `MaxResults` and the
/// range that member publishes — and a page size inside the range still answers with the
/// listing itself, so the token wording above replaced nothing else.
#[tokio::test]
async fn the_page_size_refusal_keeps_its_own_template() {
    let router = emulator();
    let url = create_ok(&router, "bounded-listing", &json!({})).await;

    let (min, max) = LIST_QUEUES_MAX_RESULTS_RANGE;
    let range = invalid_parameter_message_without_value(
        "MaxResults",
        &format!("MaxResults must be an integer between {min} and {max}."),
    );
    for outside in [min - 1, max + 1] {
        let request = json!({"QueueNamePrefix": "bounded-listing", "MaxResults": outside});
        let (status, parsed) = call(&router, "ListQueues", &request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "InvalidParameterValue"),
            "case MaxResults {outside}: expected a refusal, got {status} with {parsed}"
        );
        let reported = message(&parsed);
        assert_eq!(
            reported, range,
            "case MaxResults {outside}: expected {range:?}, got {reported:?}"
        );
        assert_ne!(
            reported, EXPECTED_TOKEN_REFUSAL,
            "case MaxResults {outside}: the page-size refusal is not the token refusal"
        );
    }

    let reply = ok(
        &router,
        "ListQueues",
        json!({"QueueNamePrefix": "bounded-listing", "MaxResults": max}),
    )
    .await;
    assert_eq!(
        listed_urls(&reply),
        vec![url],
        "case MaxResults {max}: expected the one created queue, got {reply}"
    );
    assert!(
        reply.get("NextToken").is_none(),
        "case MaxResults {max}: a listing that reached its end carries no NextToken, \
         got {reply}"
    );
}
