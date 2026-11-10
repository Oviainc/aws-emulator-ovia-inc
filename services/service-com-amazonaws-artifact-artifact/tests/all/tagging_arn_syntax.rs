// SPDX-License-Identifier: CC0-1.0

//! The ARN SYNTAX the tagging surface holds a caller-supplied `resourceArn` to, over HTTP.
//!
//! ONE rule, asserted over a SET of values rather than any single one: **a `resourceArn`
//! the tagging surface cannot read an ARN out of is refused with HTTP 400
//! `BadRequestException` — the fixed sentence `Invalid input resource arn: bad syntax` —
//! on every operation that binds that member, before anything resolves a target.** An ARN
//! the surface CAN read that names no resource of this account is a different judgement,
//! made against this service's state, and keeps the front door's 401
//! `UnauthorizedException`; that boundary is what the control rows below pin.
//!
//! WHY the two differ: the model gives `resourceArn` no identifier syntax
//! (`LongStringAttribute`, `@length(1..2048)`, `^[^<>]*$` — `smithy_query` shape
//! `ListTagsForResourceRequest`), so the boundary validator admits any text without angle
//! brackets and a value that is no ARN reaches the service. There it is a REQUEST-SHAPE
//! failure, stated in the request shape of the surface in front of this service: the status
//! class is 400 and not the 401 that surface answers a target it cannot authorize with — an
//! SDK retry or credential-refresh path keyed on 401 would re-sign and retry a request that
//! can never succeed, and the caller would be told its credentials are bad rather than that
//! its ARN is malformed.
//!
//! The rows differ along what the rule generalises over: which field of the ARN form is
//! missing or empty, how many colon-separated fields the value carries, the case of the
//! `arn` prefix, and whether the value is an identifier of this service or free text. Each
//! row is driven through all three operations that bind the member.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The sender, the emulator and the answer are the harness's
//! (`tests/all/harness.rs`), so this suite grows no second copy of the request builder.

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, Answer};

/// The status, wire error code and fixed sentence this surface refuses a `resourceArn` it
/// could not read with. Spelled out here rather than imported from the crate so a changed
/// constant fails this suite instead of redefining what it asserts.
const BAD_SYNTAX: (u16, &str, &str) = (
    400,
    "BadRequestException",
    "Invalid input resource arn: bad syntax",
);

/// What an ARN this surface CAN read, naming no resource of this account, is refused with.
const UNAUTHORIZED: (u16, &str) = (401, "UnauthorizedException");

/// The error body's `message`, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// `resourceArn` is an `@httpLabel` on `/tags/{resourceArn}`, and a non-greedy label is ONE
/// path segment: the `/` inside an ARN's resource field has to be written `%2F` or it would
/// split the segment.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// One tag call per operation that binds `resourceArn`, so every row is asserted on all
/// three: the two writes and the read.
async fn tag_calls(router: &MultiServiceRouter, resource_arn: &str) -> Vec<(&'static str, Answer)> {
    let written = label(resource_arn);
    vec![
        (
            "TagResource",
            call(
                router,
                "POST",
                &format!("/tags/{written}"),
                Bytes::from(json!({ "tags": { "team": "compliance" } }).to_string()),
            )
            .await,
        ),
        (
            "UntagResource",
            call(
                router,
                "DELETE",
                &format!("/tags/{written}?tagKeys=team"),
                Bytes::new(),
            )
            .await,
        ),
        (
            "ListTagsForResource",
            call(router, "GET", &format!("/tags/{written}"), Bytes::new()).await,
        ),
    ]
}

/// The values this suite refuses: everything the ARN form admits no reading of. The form is
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`, with the partition, the vendor
/// and the resource non-empty and the Region and account allowed to be empty — the one
/// parse `services/common`'s `arn` module states for every service that binds a
/// caller-supplied ARN.
fn malformed() -> Vec<(&'static str, String)> {
    vec![
        ("free text", "tagged-thing".to_string()),
        (
            "an inquiry id, not its ARN",
            "compliance-inquiry-0123456789abcdef".to_string(),
        ),
        ("a single percent-encoded space", "%20".to_string()),
        ("the prefix alone", "arn".to_string()),
        ("the prefix and a colon", "arn:".to_string()),
        (
            "five fields, one short",
            "arn:aws:artifact:eu-west-1:000000000000".to_string(),
        ),
        (
            "an empty partition",
            "arn::artifact:eu-west-1:000000000000:compliance-inquiry/i".to_string(),
        ),
        (
            "an empty vendor",
            "arn:aws::eu-west-1:000000000000:compliance-inquiry/i".to_string(),
        ),
        (
            "an empty resource",
            "arn:aws:artifact:eu-west-1:000000000000:".to_string(),
        ),
        (
            "a prefix that is not literally arn",
            "ARN:aws:artifact:eu-west-1:000000000000:compliance-inquiry/i".to_string(),
        ),
    ]
}

/// The ARN of the inquiry a row creates, read off the create's own answer so the control
/// rows resolve the ARN the service minted rather than one this suite built.
async fn created_inquiry_arn(router: &MultiServiceRouter, name: &str) -> String {
    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(
            json!({
                "name": name,
                "inquiryContent": { "query": "Where is customer data stored?" },
                "tags": { "team": "compliance" }
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    created.body["complianceInquirySummary"]["arn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the create carries the inquiry's ARN, got {:?}",
                created.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_refused_as_a_bad_request() {
    let router = emulator();
    let (status, code, sentence) = BAD_SYNTAX;

    for (case, resource_arn) in malformed() {
        for (operation, answer) in tag_calls(&router, &resource_arn).await {
            let case = format!("{case} on {operation}");
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected __type {code}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type, code,
                "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                message(&answer),
                sentence,
                "case {case:?}: expected the fixed sentence {sentence:?}, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn an_arn_the_surface_can_read_keeps_the_state_judgement_it_had() {
    let router = emulator();
    let held = created_inquiry_arn(&router, "a tagged inquiry").await;

    // Control rows, all of them well-formed ARNs: the syntax rule passes them through, so
    // what answers is the lookup — the one this account holds is served, and every other is
    // the front door's authorization refusal. The rows differ along which field names
    // nothing.
    let readable: Vec<(&str, String)> = vec![
        (
            "an inquiry id this account never minted",
            "arn:aws:artifact:eu-west-1:000000000000:compliance-inquiry/\
             compliance-inquiry-0000000000000000"
                .to_string(),
        ),
        (
            "another service's resource",
            "arn:aws:s3:eu-west-1:000000000000:bucket/compliance".to_string(),
        ),
        (
            "an empty region and account, which an ARN may carry",
            "arn:aws:artifact:::compliance-inquiry/compliance-inquiry-0000000000000000"
                .to_string(),
        ),
        (
            "a resource type this service does not hold",
            "arn:aws:artifact:eu-west-1:000000000000:report/report-0123456789abcdef".to_string(),
        ),
    ];
    for (case, resource_arn) in readable {
        for (operation, answer) in tag_calls(&router, &resource_arn).await {
            let case = format!("{case} on {operation}");
            assert_eq!(
                answer.status,
                UNAUTHORIZED.0,
                "case {case:?}: expected HTTP {}, got {} with {:?}",
                UNAUTHORIZED.0,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.error_code(),
                UNAUTHORIZED.1,
                "case {case:?}: expected {}, got {:?}",
                UNAUTHORIZED.1,
                answer.body
            );
        }
    }

    // And the positive control: the resource this account does hold is still served, so the
    // syntax rule refuses values rather than the operation.
    let listed = call(
        &router,
        "GET",
        &format!("/tags/{}", label(&held)),
        Bytes::new(),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "the held resource must still be served, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["tags"],
        json!({ "team": "compliance" }),
        "the held resource must report the tags its create persisted, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_refused_tag_call_writes_nothing() {
    let router = emulator();
    let held = created_inquiry_arn(&router, "a tagged inquiry").await;

    // A malformed ARN is refused before any target resolves, so no tag set can have moved —
    // asserted on the one resource this account holds, through both writes and the read.
    for (case, resource_arn) in malformed() {
        for (operation, answer) in tag_calls(&router, &resource_arn).await {
            assert_eq!(
                answer.status, BAD_SYNTAX.0,
                "case {case:?} on {operation}: expected HTTP {}, got {} with {:?}",
                BAD_SYNTAX.0, answer.status, answer.body
            );
        }
    }

    let listed = call(
        &router,
        "GET",
        &format!("/tags/{}", label(&held)),
        Bytes::new(),
    )
    .await;
    assert_eq!(
        listed.body["tags"],
        json!({ "team": "compliance" }),
        "the refused calls must leave the held resource's tag set as it was, got {:?}",
        listed.body
    );
}
