// SPDX-License-Identifier: CC0-1.0

//! The ARN-addressed not-found refusal, asserted to be ONE rule rather than one per
//! operation.
//!
//! Every operation addressed by a `ResourceARN` resolves that ARN the same way and, when
//! it resolves to no rescore execution plan, refuses the call with the same modelled
//! error carrying the same sentence. This suite drives that shared path through each of
//! the three ARN-addressed operations and asserts their refusals agree with each other
//! and with `state::resource_unavailable_message`, so a change made in one place can no
//! longer leave the others stating something else.

use serde_json::{json, Value};

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use super::harness::{
    call, create_plan_with_tags, emulator, error_code, error_message, sleep_past_the_transition,
};

/// Every ARN-addressed operation of this service, with a request body against `arn`
/// that reaches the ARN lookup: one row per handler that used to build this refusal
/// itself.
fn arn_addressed_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "team", "Value": "search" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["team"] }),
        ),
        ("ListTagsForResource", json!({ "ResourceARN": arn })),
    ]
}

#[tokio::test]
async fn every_arn_addressed_operation_states_the_same_unavailable_refusal() {
    let router = emulator();

    // A live plan to mint realistic ARNs from, and a deleted one whose ARN names no
    // resource any more.
    let (live_id, live_arn) =
        create_plan_with_tags(&router, "single-source-live", json!([{ "Key": "team", "Value": "search" }]))
            .await;
    let (gone_id, gone_arn) = create_plan_with_tags(&router, "single-source-doomed", Value::Null).await;
    // A freshly created plan is `CREATING`; it can only be deleted once the clock has
    // made it `ACTIVE`.
    sleep_past_the_transition();
    let (status, answer) = call(&router, "DeleteRescoreExecutionPlan", json!({ "Id": gone_id })).await;
    assert_eq!(status, 200, "delete the doomed plan: got {answer}");

    // The classes of `ResourceARN` that resolve to no plan in this request's region:
    // the rule generalises over the FORM of the ARN, so the rows differ in form, not in
    // a name.
    let unknown_plan = live_arn.replace(&live_id, "11111111-1111-4111-8111-111111111111");
    let other_region = live_arn.replace("us-east-1", "ap-south-1");
    let other_service = live_arn.replace("kendra-ranking", "kendra");
    let other_resource_type = live_arn.replace("rescore-execution-plan/", "index/");
    let arn_shaped_nothing = format!("arn:aws:kendra-ranking:us-east-1:{}", "z".repeat(64));
    let cases: Vec<(&str, &str)> = vec![
        ("a plan id this store never minted", unknown_plan.as_str()),
        ("a deleted plan's ARN", gone_arn.as_str()),
        ("the live plan's ARN in another region", other_region.as_str()),
        ("another service's ARN", other_service.as_str()),
        (
            "another resource type in this service",
            other_resource_type.as_str(),
        ),
        (
            "an ARN-shaped string naming nothing",
            arn_shaped_nothing.as_str(),
        ),
        ("a string that is not an ARN at all", "not-an-arn"),
        ("a single character", "x"),
    ];

    for (case, arn) in cases {
        // What the crate's single construction site says for this ARN. Every operation
        // is held to it, so the three handlers cannot drift apart — and the sentence
        // that site builds is itself held to the service's wording by
        // `the_unavailable_sentence_is_the_service_s_own_wording` below, so a rewording
        // cannot pass by changing both sides of this comparison.
        let expected = state::resource_unavailable_message(arn);
        for (operation, body) in arn_addressed_calls(arn) {
            let (status, answer) = call(&router, operation, body).await;
            // This RPC service answers every client-side failure with the protocol's
            // client-error status; the error TYPE says which failure it was.
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceUnavailableException",
                "case {case:?} on {operation}: expected ResourceUnavailableException, got {code:?} in {answer}"
            );
            let message = error_message(&answer);
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_unavailable_sentence_is_the_service_s_own_wording() {
    let router = emulator();

    // The wording itself, over the wire, independent of the constructor the handlers
    // call: the resource-unavailable sentence names the ARN the caller supplied and then
    // states what to check. Spelled out literally here so a rewording of the single
    // construction site fails this suite instead of silently changing both sides of the
    // agreement assertion above.
    for arn in [
        "arn:aws:kendra-ranking:us-east-1:000000000000:rescore-execution-plan/22222222-2222-4222-8222-222222222222",
        "not-an-arn",
    ] {
        for (operation, body) in arn_addressed_calls(arn) {
            let (_, answer) = call(&router, operation, body).await;
            let message = error_message(&answer);
            let expected = format!(
                "Resource {arn} was not found. Please check you have provided the correct \
                 resource information and try again."
            );
            assert_eq!(
                message, expected,
                "{operation} on {arn:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_arn_that_resolves_is_served_by_every_arn_addressed_operation() {
    let router = emulator();

    // The other side of the shared lookup: an ARN that DOES resolve reaches each
    // handler's own work, so centralising the refusal did not change a served call.
    let (_, arn) = create_plan_with_tags(&router, "single-source-served", Value::Null).await;

    for (operation, body) in arn_addressed_calls(&arn) {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} on an ARN that resolves: expected 200, got {status} {answer}"
        );
    }

    // `TagResource` wrote the tag and `UntagResource` removed it again, in that order,
    // so the plan ends with none: the shared resolver handed each call the same plan.
    let (status, answer) = call(&router, "ListTagsForResource", json!({ "ResourceARN": arn })).await;
    assert_eq!(status, 200, "list the served plan's tags: got {answer}");
    let tags = answer["Tags"].as_array().cloned().unwrap_or_default();
    assert!(
        tags.is_empty(),
        "after a tag and an untag of the same key: expected no tags, got {tags:?}"
    );
}
