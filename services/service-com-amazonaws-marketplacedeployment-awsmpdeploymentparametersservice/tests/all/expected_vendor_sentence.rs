// SPDX-License-Identifier: CC0-1.0

//! The ONE sentence this service refuses a foreign-vendor `resourceArn` with.
//!
//! The rule under test: a tagging operation handed a `resourceArn` whose vendor field
//! is not this service's own ARN namespace is refused with one sentence that names the
//! vendor that was EXPECTED and nothing of the ARN it was handed — the same bytes for
//! every operation that resolves a caller-supplied resource ARN, and the same bytes the
//! other services of this workspace refuse a foreign ARN with, because all of them
//! render it through the shared `service_common::validation_message` helper rather than
//! a `format!` of their own. `state::validate_resource_arn` is the one place this
//! service makes that judgement, so folding its local copy onto the shared helper is
//! what keeps the wording from drifting.
//!
//! The three operations that bind `resourceArn` as a required `@httpLabel`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`:
//! `GET /tags/{resourceArn}`, `POST /tags/{resourceArn}`, `DELETE /tags/{resourceArn}`)
//! — every caller of that check — are each driven over the same table, through the
//! production `MultiServiceRouter` and restJson1 serialization, so what is asserted is
//! the bytes a signed SDK request observes.
//!
//! The sibling `resource_arn_validation` suite owns the classification of the refusal
//! (its status, its wire code, and which values land on the vendor condition rather
//! than the member's own syntax); this suite pins the rendered SENTENCE.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use serde_json::Value;
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::ARN_NAMESPACE;
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, list_tags, message, tag, tag_map, untag, ACCOUNT, REGION, SIGNING_NAME,
};

/// The sentence a foreign-vendor resource ARN is refused with, in full.
///
/// Written out here rather than read from the crate, so the assertion is not the
/// implementation compared with itself: this is the rendering of the shared
/// resource-tagging sentence (`service_common::validation_message::expected_vendor`)
/// for this service's own ARN namespace.
const EXPECTED_SENTENCE: &str = "Invalid input resource arn: expected vendor aws-marketplace";

/// The wire error code that refusal carries, which the `resource_arn_classification`
/// and `resource_arn_validation` suites own in full.
const EXPECTED_CODE: &str = "BadRequestException";

/// Every operation that resolves a caller-supplied `resourceArn`, and so reaches the
/// one vendor check.
const TAGGING_OPERATIONS: [&str; 3] = ["ListTagsForResource", "TagResource", "UntagResource"];

/// Call `operation` with `resource_arn`, as a signed SDK request would.
async fn call_tagging(
    router: &MultiServiceRouter,
    operation: &str,
    resource_arn: &str,
) -> (u16, Value) {
    match operation {
        "ListTagsForResource" => list_tags(router, resource_arn).await,
        "TagResource" => tag(router, resource_arn, &tag_map(&[("owner", "suite")])).await,
        "UntagResource" => untag(router, resource_arn, &["owner".to_string()]).await,
        other => panic!("the suite drives only the tagging operations, got {other:?}"),
    }
}

#[tokio::test]
async fn every_tagging_operation_refuses_a_foreign_vendor_with_the_one_shared_sentence() {
    assert!(
        EXPECTED_SENTENCE.ends_with(ARN_NAMESPACE),
        "the sentence names this service's own namespace {ARN_NAMESPACE:?}, got \
         {EXPECTED_SENTENCE:?}"
    );
    let (router, _engine) = emulator();

    // The class the rule generalises over: a `resourceArn` inside the member's own
    // syntax whose VENDOR field is not this service's. The rows differ along how the
    // vendor differs — another service, a vendor this namespace is a prefix of, a
    // different casing, an empty field, another partition, a colon-form resource, a
    // value that is no ARN at all — not merely along the resource id.
    let foreign: Vec<(&str, String)> = vec![
        (
            "another service's namespace",
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret/dp-0000000000000001"),
        ),
        (
            "a vendor this namespace is a prefix of",
            format!("arn:aws:{SIGNING_NAME}-catalog:{REGION}:{ACCOUNT}:entity/entity-1"),
        ),
        (
            "this namespace in another case",
            format!("arn:aws:AWS-MARKETPLACE:{REGION}:{ACCOUNT}:deployment-parameter/dp-1"),
        ),
        (
            "an empty vendor field",
            format!("arn:aws::{REGION}:{ACCOUNT}:deployment-parameter/dp-1"),
        ),
        (
            "another vendor in another partition",
            format!("arn:aws-cn:launchwizard:cn-north-1:{ACCOUNT}:deployment/dp-1"),
        ),
        (
            "a colon-form ARN of another vendor",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:machine:run"),
        ),
        (
            "a value that is not an ARN at all",
            "dp-0000000000000001".to_string(),
        ),
        (
            "an ARN-shaped value whose first field is not arn",
            format!("urn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-1"),
        ),
    ];

    for (case, resource_arn) in foreign {
        for operation in TAGGING_OPERATIONS {
            let (status, answered) = call_tagging(&router, operation, &resource_arn).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                EXPECTED_CODE,
                "case {case:?}/{operation}: expected code {EXPECTED_CODE}, got {answered:?}"
            );
            let reported = message(&answered);
            assert_eq!(
                reported, EXPECTED_SENTENCE,
                "case {case:?}/{operation}: expected the one shared sentence \
                 {EXPECTED_SENTENCE:?}, got {reported:?}"
            );
            // The sentence states what was expected, never the value it was handed.
            assert!(
                !reported.contains(resource_arn.as_str()),
                "case {case:?}/{operation}: the refusal echoes no ARN, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_shared_vendor_sentence_is_reserved_for_the_vendor_condition() {
    let (router, _engine) = emulator();

    // Neither of the other two answers a tagging operation can give is worded with the
    // vendor sentence: an ARN that IS of this vendor and names nothing is the store's
    // not-found, and a value outside the member's own syntax states that syntax.
    let other_answers: Vec<(&str, String, u16)> = vec![
        (
            "an ARN of this vendor that names no stored record",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffff"),
            404,
        ),
        (
            "a value outside the member's own character class",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment parameter/dp-1"),
            400,
        ),
    ];

    for (case, resource_arn, expected) in other_answers {
        for operation in TAGGING_OPERATIONS {
            let (status, answered) = call_tagging(&router, operation, &resource_arn).await;
            assert_eq!(
                status, expected,
                "case {case:?}/{operation}: expected HTTP {expected}, got {status} {answered:?}"
            );
            let reported = message(&answered);
            assert_ne!(
                reported, EXPECTED_SENTENCE,
                "case {case:?}/{operation}: this answer is not the vendor refusal, got \
                 {reported:?}"
            );
        }
    }
}
