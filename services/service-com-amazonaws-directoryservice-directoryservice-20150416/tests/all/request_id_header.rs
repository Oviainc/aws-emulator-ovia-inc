// SPDX-License-Identifier: CC0-1.0

//! The request id a Directory Service response reports in its header, driven end to end
//! over HTTP.
//!
//! Every error shape this service models carries a `RequestId` member, and a refusal
//! states the id of the call it answers there and at the end of `Message`. AWS reports
//! that same id in the response's own `x-amzn-requestid` header — where an SDK's response
//! metadata reads a failed call's request id from — so a client correlating on the header
//! and one reading the document must see ONE value.
//!
//! The rule is the ENVELOPE's, not any one operation's, so it is asserted over a TABLE of
//! refusals spanning several operations and several of the modelled error shapes, each row
//! differing in which layer produced the refusal. Nothing here reaches into the state
//! struct: the rows go through the same front door a client uses.

use serde_json::{json, Value};

use crate::directory_lifecycle::{create_directory, minimal_create};
use crate::harness::{call_raw, ok, service};

/// The header AWS reports a response's own request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One refusal to assert the rule on: the call to make and what it is.
struct HeaderCase {
    case: String,
    operation: &'static str,
    body: Value,
}

#[tokio::test]
async fn a_refusal_reports_the_request_id_it_states_in_the_header_too() {
    let router = service();

    let directory_id =
        create_directory(&router, minimal_create("header.example.com"), "header").await;
    ok(
        &router,
        "CreateLogSubscription",
        json!({ "DirectoryId": &directory_id, "LogGroupName": "/aws/header" }),
        "header: subscribe",
    )
    .await;

    // The rows differ along the operation refused and the error shape it answers with: a
    // directory that does not exist, an entity that already exists, a subscription that
    // does not exist, a token this service did not issue and a resource id that names
    // nothing taggable.
    let cases = vec![
        HeaderCase {
            case: "CreateLogSubscription for an absent directory".to_string(),
            operation: "CreateLogSubscription",
            body: json!({ "DirectoryId": "d-0000000000", "LogGroupName": "nowhere" }),
        },
        HeaderCase {
            case: "CreateLogSubscription for a directory that already has one".to_string(),
            operation: "CreateLogSubscription",
            body: json!({ "DirectoryId": &directory_id, "LogGroupName": "/aws/second" }),
        },
        HeaderCase {
            case: "DeleteLogSubscription for an absent directory".to_string(),
            operation: "DeleteLogSubscription",
            body: json!({ "DirectoryId": "d-0000000000" }),
        },
        HeaderCase {
            case: "ListLogSubscriptions with a token this service did not issue".to_string(),
            operation: "ListLogSubscriptions",
            body: json!({ "NextToken": "not-a-token" }),
        },
        HeaderCase {
            case: "DescribeSnapshots for an absent directory".to_string(),
            operation: "DescribeSnapshots",
            body: json!({ "DirectoryId": "d-0000000000" }),
        },
        HeaderCase {
            case: "ListTagsForResource for an id that is not taggable".to_string(),
            operation: "ListTagsForResource",
            body: json!({ "ResourceId": "d-0000000000" }),
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for HeaderCase {
        case,
        operation,
        body,
    } in cases
    {
        let response = call_raw(&router, operation, body).await;
        let header = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .map(|value| value.to_str().expect("a textual header").to_string());
        let payload = response.into_body();
        let parsed: Value = serde_json::from_slice(&payload)
            .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {payload:?}"));
        let stated = parsed["RequestId"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected the modelled RequestId member, got {parsed}")
        });
        assert_eq!(
            header.as_deref(),
            Some(stated),
            "case {case:?}: expected the header to carry the id the document states \
             ({stated}), got {header:?}"
        );
        // The id names ONE call, so no two refusals report the same one.
        assert!(
            !seen.contains(&stated.to_string()),
            "case {case:?}: expected a request id not yet reported, got {stated} after {seen:?}"
        );
        seen.push(stated.to_string());
    }
}
