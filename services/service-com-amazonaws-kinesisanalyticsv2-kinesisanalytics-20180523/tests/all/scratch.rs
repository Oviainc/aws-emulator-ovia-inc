// SPDX-License-Identifier: CC0-1.0

//! A probe of the SCOPE of this service's request-deserialization layer
//! (`src/service/request_layer.rs`): which members it decides the JSON class of, and which
//! it leaves to the framework's own codec.
//!
//! The layer exists for one rule — an `epoch-seconds` timestamp travels as a JSON number,
//! which `body_timestamp_class.rs` asserts over every operation that binds one. What is
//! asserted HERE is the other half of that rule: a member the layer does NOT govern is
//! answered exactly as it was before the layer existed, so adding it widened nothing. A
//! member of another modelled type, and a timestamp carried by an operation that binds
//! none, must therefore never earn the layer's protocol-level refusal.

use serde_json::json;

use crate::harness::{call, create, error_code, error_message, router};

/// The sentence the request layer refuses an unreadable timestamp with; no member it does
/// not govern may be answered with it.
const TIMESTAMP_REFUSAL_TAIL: &str = "can not be converted to milliseconds since epoch";

/// A member whose modelled type is not a timestamp, and a timestamp-looking key on an
/// operation whose input binds no timestamp, are both left to the framework: whatever it
/// answers, it is not this layer's `SerializationException` about an epoch timestamp.
#[tokio::test]
async fn the_request_layer_governs_only_the_timestamp_members_it_names() {
    let router = router();
    create(&router, "layer-scope", "FLINK-1_20").await;

    // (case, operation, a request carrying a value of the wrong class for a member this
    //  layer does NOT govern)
    let cases = [
        (
            "integer-member",
            "ListApplications",
            json!({"Limit": "10"}),
        ),
        (
            "string-member",
            "DescribeApplication",
            json!({"ApplicationName": 7}),
        ),
        (
            "boolean-member",
            "DescribeApplication",
            json!({"ApplicationName": "layer-scope", "IncludeAdditionalDetails": "yes"}),
        ),
        (
            "timestamp-shaped-key-on-an-operation-binding-none",
            "DescribeApplication",
            json!({"ApplicationName": "layer-scope", "CreateTimestamp": "1700000000"}),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, answered) = call(&router, operation, &body).await;
        assert!(
            !error_message(&answered).contains(TIMESTAMP_REFUSAL_TAIL),
            "case {case}: a member this layer does not govern must not earn its refusal, \
             got {status} {answered}"
        );
        assert_ne!(
            error_code(&answered),
            "SerializationException",
            "case {case}: expected the framework's own answer, got {status} {answered}"
        );
    }
}
