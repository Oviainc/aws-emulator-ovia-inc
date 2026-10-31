// SPDX-License-Identifier: CC0-1.0

//! A refusal is never worded as an error shape the model documents as superseded.
//!
//! The rule under test: where a service's model marks an error shape deprecated and its
//! documentation names the shape that replaces it, the refusal that shape used to carry
//! is answered as the REPLACEMENT — so the deprecated shape is bound by the operation
//! but never put on the wire. Two of this service's error shapes say so of themselves
//! (`smithy_query(query="shape",shape="TagPolicyException,TooManyTagsException",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`),
//! and both are bound to the tagging surface: `TagPolicyException` ("is deprecated. See
//! `ValidationException` instead") and `TooManyTagsException` ("is deprecated. See
//! `ServiceQuotaExceededException` instead").
//!
//! So the two refusals those shapes describe are asserted in their replacement's form:
//!
//! * a tag key in AWS's reserved `aws:` namespace — what the tag-policy shape describes
//!   — is a `ValidationException`, stating the reason the model declares for a failed
//!   field check and listing the member that carried the key, exactly as
//!   `CreateEnvironment` already reports it;
//! * a resource left with more tags than one carries — what the too-many-tags shape
//!   describes — is a `ServiceQuotaExceededException`.
//!
//! The rows span what the rule generalises over: both deprecated shapes, every operation
//! of this service that takes tag keys from the request (`TagResource`,
//! `UntagResource`, `CreateEnvironment`) with the member each carries them in, the
//! spelling of the offending key (the catalog `reserved_tag_keys.rs` owns), and — for
//! the count bound — a request that lands exactly ON the bound, one that leaves the
//! count where it was, and one that goes past it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice::state::{
    REASON_FIELD_VALIDATION_FAILED, RESERVED_TAG_KEY_PREFIX,
};

use crate::environment_lifecycle::{call, emulator, error_type, Emulator};
use crate::reserved_tag_keys::{
    reserved_keys, starting_tags, stored_tags, tag_map, tagged_environment,
};

/// The error shapes this service's model documents as superseded, each beside the shape
/// its own documentation names as the replacement — the pairs the rule is stated over.
///
/// Source: the `@documentation` of each shape
/// (`smithy_query(query="shape",shape="TagPolicyException,TooManyTagsException",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`).
const SUPERSEDED_ERRORS: [(&str, &str); 2] = [
    ("TagPolicyException", "ValidationException"),
    ("TooManyTagsException", "ServiceQuotaExceededException"),
];

/// The most tags one resource carries, from the bound the modelled tag maps declare
/// (`RequestTagMap`/`TagKeys` len 1..200,
/// `smithy_query(query="operation",operation="TagResource")`) and the one the superseded
/// too-many-tags shape reports.
const MAX_TAGS_PER_RESOURCE: usize = 200;

/// Assert that no superseded shape is what answered, whatever did.
fn assert_no_superseded_shape(case: &str, body: &Value) {
    let answered = error_type(body);
    for (superseded, replacement) in SUPERSEDED_ERRORS {
        assert_ne!(
            answered, superseded,
            "case {case:?}: {superseded} is documented as superseded by {replacement}, so it \
             is never what answers; got {body}"
        );
    }
}

/// Assert one refusal is the named replacement shape: HTTP 400 (every modelled client
/// error of this awsJson1_0 service renders as 400 — `tests/all/error_wire_status.rs`),
/// that shape in `__type`, and no superseded shape anywhere.
fn assert_replacement_shape(case: &str, status: u16, body: &Value, replacement: &str) {
    assert_eq!(
        status,
        400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert_eq!(
        error_type(body),
        replacement,
        "case {case:?}: expected a {replacement}, got {} {body}",
        error_type(body)
    );
    assert_no_superseded_shape(case, body);
}

/// One request refused for a reserved tag key, and the member that carried it.
struct ReservedKeyRow {
    case: String,
    operation: &'static str,
    body: Value,
    member: &'static str,
    key: String,
}

/// Every operation that takes tag keys from the request, crossed with every spelling of
/// a reserved key — one row per pair, each with the member the offending key travels in.
fn reserved_key_rows(router: &Emulator, resource_arn: &str) -> Vec<ReservedKeyRow> {
    let mut rows = Vec::new();
    for (index, (why, key)) in reserved_keys().into_iter().enumerate() {
        rows.push(ReservedKeyRow {
            case: format!("TagResource with {why}"),
            operation: "TagResource",
            body: json!({
                "resourceArn": resource_arn,
                "tags": tag_map(&[(key.as_str(), "x")]),
            }),
            member: "tags",
            key: key.clone(),
        });
        rows.push(ReservedKeyRow {
            case: format!("UntagResource with {why}"),
            operation: "UntagResource",
            body: json!({
                "resourceArn": resource_arn,
                "tagKeys": [key.as_str()],
            }),
            member: "tagKeys",
            key: key.clone(),
        });
        let mut create = router.create_request(Some(&format!("superseded-create-{index}")));
        create["tags"] = tag_map(&[(key.as_str(), "x")]);
        rows.push(ReservedKeyRow {
            case: format!("CreateEnvironment with {why}"),
            operation: "CreateEnvironment",
            body: create,
            member: "tags",
            key,
        });
    }
    rows
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_as_the_shape_that_replaces_the_tag_policy_error() {
    let router = emulator();
    let arn = tagged_environment(&router, "superseded-tag-policy").await;

    for row in reserved_key_rows(&router, &arn) {
        let ReservedKeyRow {
            case,
            operation,
            body: request,
            member,
            key,
        } = row;
        let (status, body) = call(&router, operation, &request).await;
        assert_replacement_shape(&case, status, &body, "ValidationException");

        // The replacement shape carries what the superseded one could not: WHY the
        // request failed, and WHICH member a client must correct.
        let reason = body["reason"].as_str().unwrap_or_default();
        assert_eq!(
            reason, REASON_FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {REASON_FIELD_VALIDATION_FAILED:?}, got \
             {reason:?} in {body}"
        );
        let names: BTreeSet<&str> = body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"))
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from([member]),
            "case {case:?}: expected the refusal to name the {member:?} member, got \
             {names:?} in {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&key) && message.contains(RESERVED_TAG_KEY_PREFIX),
            "case {case:?}: the refusal must name the key {key:?} and the reserved prefix \
             {RESERVED_TAG_KEY_PREFIX:?}, got {message:?}"
        );
    }
}

/// How many tags the resource the ARN names carries.
async fn stored_tag_count(router: &Emulator, case: &str, resource_arn: &str) -> usize {
    stored_tags(router, case, resource_arn)
        .await
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the reported tags are a map"))
        .len()
}

#[tokio::test]
async fn the_tag_count_bound_is_refused_as_the_shape_that_replaces_the_too_many_tags_error() {
    let router = emulator();
    let arn = tagged_environment(&router, "superseded-too-many-tags").await;
    let held = starting_tags()
        .as_object()
        .expect("the starting tags are a map")
        .len();

    // ON the bound: a request that leaves the resource with exactly the most tags one
    // carries is accepted, so what refuses below is the count and not the request's size.
    let case = "TagResource up to the bound exactly";
    let filling: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE - held)
        .map(|index| (format!("filler-{index}"), index.to_string()))
        .collect();
    let entries: Vec<(&str, &str)> = filling
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": tag_map(&entries) }),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    assert_eq!(
        stored_tag_count(&router, case, &arn).await,
        MAX_TAGS_PER_RESOURCE,
        "case {case:?}: expected the resource to carry {MAX_TAGS_PER_RESOURCE} tags"
    );

    // A request that moves the count nowhere is accepted at the bound too: the rule is
    // about what the resource is LEFT with, not about the write.
    let case = "TagResource re-writing a key the resource already carries";
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": tag_map(&[("filler-0", "0")]) }),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );

    // One PAST the bound, in rows differing by how many tags the request would add: each
    // is the quota refusal, and none writes anything.
    for extra in [1usize, 2, 50] {
        let case = format!("TagResource leaving the resource with {} tags", 200 + extra);
        let adding: Vec<(String, String)> = (0..extra)
            .map(|index| (format!("past-{index}"), index.to_string()))
            .collect();
        let entries: Vec<(&str, &str)> = adding
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": tag_map(&entries) }),
        )
        .await;
        assert_replacement_shape(&case, status, &body, "ServiceQuotaExceededException");
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&MAX_TAGS_PER_RESOURCE.to_string()),
            "case {case:?}: the refusal must name the bound \
             {MAX_TAGS_PER_RESOURCE}, got {message:?}"
        );
        assert_eq!(
            stored_tag_count(&router, &case, &arn).await,
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: a refused TagResource must write nothing"
        );
    }
}
