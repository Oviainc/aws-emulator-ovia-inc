// SPDX-License-Identifier: CC0-1.0

//! The wording of the refusal a recommendation reader answers an instance ARN that is not
//! one.
//!
//! `GetEC2InstanceRecommendations` takes "the Amazon Resource Name (ARN) of the instances for
//! which to return recommendations", and anything else in that list is refused with the
//! service's OWN constant sentence for the condition: one text for every value in the class,
//! naming neither the input member it arrived on nor the value the request carried, and down
//! to the way it words "arn".
//!
//! The suite drives the class over a TABLE spanning what it generalises over — every form a
//! string that is not an EC2 instance ARN takes, crossed with the shape of the list it arrives
//! in — and asserts the SAME sentence every time; the recorded case is one row. The
//! preference refusals of `PutRecommendationPreferences` are in
//! `tests/all/preference_resource_type_support.rs`, the service's other constant refusals in
//! `tests/all/client_refusal_wording.rs`.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what is asserted is what
//! a client sees.

use serde_json::json;

use crate::harness::{
    auto_scaling_group_arn, call, emulator, error_code, instance_arn, opt_in, ACCOUNT,
};

/// The generic wording this service must never answer: a refusal built from a member name and
/// the value the request carried. Asserted absent on every row below, so a refusal that falls
/// back to a parameter template fails whatever its remaining text says.
const GENERIC_PARAMETER_PREFIX: &str = "The value supplied for parameter";

/// An `instanceArns` entry that is not an Amazon EC2 instance ARN is refused `400
/// InvalidParameterValueException` with the service's one instance-ARN sentence, whatever
/// form the value took and whatever else the request carried, and the rejected value is never
/// echoed.
#[tokio::test]
async fn an_instance_arn_that_is_not_one_is_refused_with_the_services_instance_arn_sentence() {
    let router = emulator();
    opt_in(&router).await;

    // The service's sentence for the condition: all of it is fixed text, down to the way it
    // words "arn".
    let expected = "Invalid instance arn.";

    // The forms a string that is not an EC2 instance ARN takes: not an ARN at all, an empty
    // value, a truncated one, a bare instance id, and well-formed ARNs of other services and
    // other resource kinds — including the Auto Scaling group ARN this service accepts
    // elsewhere.
    let rejected: Vec<String> = vec![
        "not-an-arn".to_string(),
        String::new(),
        "arn:aws".to_string(),
        "arn:aws:ec2:us-east-1".to_string(),
        "i-0123456789abcdef0".to_string(),
        format!("arn:aws:ec2:us-east-1:{ACCOUNT}:volume/vol-0123456789abcdef0"),
        format!("arn:aws:lambda:us-east-1:{ACCOUNT}:function:summarise"),
        auto_scaling_group_arn("batch-tier"),
    ];

    for value in &rejected {
        // Rows differ in the rest of the request too — the list's length and whether a
        // well-formed instance ARN came first — so the refusal cannot depend on either.
        let lists = [
            json!([value]),
            json!([instance_arn("i-0123456789abcdef0"), value]),
        ];
        for instance_arns in lists {
            let case = format!("instanceArns {instance_arns}");
            let (status, body) = call(
                &router,
                "GetEC2InstanceRecommendations",
                json!({"instanceArns": instance_arns}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the request to be refused with 400, got {status}: \
                 {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterValueException"),
                "case {case:?}: expected InvalidParameterValueException, got {body}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?}: expected the service's sentence {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(GENERIC_PARAMETER_PREFIX),
                "case {case:?}: the refusal must not be built from a parameter template, got \
                 {message:?}"
            );
            assert!(
                value.is_empty() || !message.contains(value.as_str()),
                "case {case:?}: the refusal must not echo the rejected value {value:?}, got \
                 {message:?}"
            );
        }
    }

    // An `instanceArns` list of instance ARNs is still answered, so the rows above pin the
    // wording of a refusal rather than a reader that refuses every request.
    let (status, body) = call(
        &router,
        "GetEC2InstanceRecommendations",
        json!({"instanceArns": [instance_arn("i-0123456789abcdef0")]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an instance ARN is answered, got {status}: {body}"
    );
}
