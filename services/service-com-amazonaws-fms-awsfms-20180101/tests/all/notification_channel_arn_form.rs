// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ARN FORM of the two caller-supplied `ResourceArn`s a
//! notification-channel designation carries.
//!
//! The rule asserted here, in one sentence: every caller-supplied member this
//! service models as `com.amazonaws.fms#ResourceArn` and documents as an Amazon
//! Resource Name is checked for the ARN form before the request is acted on, so a
//! value that is not an ARN is refused with a 400 `ValidationException` naming the
//! one condition it breaks and nothing is stored — `SnsTopicArn` and
//! `SnsRoleName` (`smithy_query(operation="PutNotificationChannel")` reports both
//! as required `ResourceArn`s, documented as the ARN of the SNS topic and of the
//! IAM role) exactly as much as the `ResourceArn` the tagging operations read.
//!
//! Every case drives the service over the WIRE through the shared harness in
//! `policy_lifecycle` (an awsJson1_1 `POST /` with an `X-Amz-Target` header and a
//! SigV4 credential scope), and every wire assertion is backed by the persisted
//! record read through `notification_channel_deletion::stored_channel` and by the
//! implemented reader `GetNotificationChannel`.
//!
//! The table is the SHARED ARN-form class in `arn_form_single_rule::cases` — the
//! same rows the tagging operations and the resource-set membership classifier are
//! asserted over, differing in which condition of the form they break (no prefix,
//! another prefix, another case of the prefix, a value at the published minimum
//! length of the modelled shape, too few fields at three counts, an empty
//! partition, service or resource) and, for the rows that ARE ARNs, in
//! partition, in whether the Region and account fields are filled and in whether
//! the resource segment carries colons of its own. Each row is sent in the topic
//! member, in the role member and in both, so neither member is checked at the
//! expense of the other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, error_message};
use crate::arn_form_single_rule::cases;
use crate::notification_channel_deletion::stored_channel;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A topic ARN that satisfies the form, used to fill the member a row is not
/// testing.
fn good_topic() -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms-arn-form")
}

/// A role ARN that satisfies the form, used to fill the member a row is not
/// testing. IAM is global, so its Region field is empty.
fn good_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/fms-arn-form")
}

/// Which of the request's two `ResourceArn` members carries the row's value.
#[derive(Clone, Copy)]
enum Placement {
    Topic,
    Role,
    Both,
}

impl Placement {
    /// Every placement, so no row is asserted on one member only.
    const ALL: [Placement; 3] = [Placement::Topic, Placement::Role, Placement::Both];

    /// Names the placement in every assertion message.
    fn label(self) -> &'static str {
        match self {
            Placement::Topic => "in SnsTopicArn",
            Placement::Role => "in SnsRoleName",
            Placement::Both => "in both members",
        }
    }

    /// The pair of values a request carries when `value` is placed here.
    fn pair(self, value: &str) -> (String, String) {
        match self {
            Placement::Topic => (value.to_string(), good_role()),
            Placement::Role => (good_topic(), value.to_string()),
            Placement::Both => (value.to_string(), value.to_string()),
        }
    }
}

/// Designate the channel over the wire.
async fn put_channel(router: &MultiServiceRouter, topic: &str, role: &str) -> (u16, Value) {
    call(
        router,
        "PutNotificationChannel",
        &json!({ "SnsTopicArn": topic, "SnsRoleName": role }),
    )
    .await
}

/// Read the channel back over the wire.
async fn get_channel(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GetNotificationChannel", &json!({})).await
}

/// On every row of the shared form table, in every placement: a value that is
/// not an ARN is refused with 400 `ValidationException` naming the condition it
/// breaks and designates NOTHING, while a value that IS an ARN is designated and
/// reads back through `GetNotificationChannel` and in state.
#[tokio::test]
async fn designating_a_channel_refuses_exactly_the_values_that_are_not_arns() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // What the store holds as the loop runs: `None` until the first row is
    // designated, and unchanged across every refusal — so a refusal that had
    // written something would be caught by the next assertion.
    let mut designated: Option<(String, String)> = None;

    for case in cases() {
        for placement in Placement::ALL {
            let label = format!("{} / {}", case.name, placement.label());
            let (topic, role) = placement.pair(&case.value);
            let (status, answered) = put_channel(&router, &topic, &role).await;

            match case.defect {
                Some(defect) => {
                    assert_eq!(
                        (status, error_code(&answered).as_str()),
                        (400, "ValidationException"),
                        "case {label:?}: expected {:?} to be refused as not an ARN, got {status} \
                         with {answered}",
                        case.value
                    );
                    let message = error_message(&answered);
                    assert!(
                        message.contains(defect) && message.contains(&case.value),
                        "case {label:?}: expected the refusal to name {defect:?} and the value, \
                         got {message:?}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {label:?}: {:?} is an ARN, so the designation must be accepted, got \
                         {status} with {answered}",
                        case.value
                    );
                    designated = Some((topic.clone(), role.clone()));
                }
            }

            // The store, after either answer: what the last accepted designation
            // wrote, and nothing else.
            let stored = stored_channel(&engine, REGION);
            assert_eq!(
                stored, designated,
                "case {label:?}: the store must hold exactly what the last accepted designation \
                 wrote, got {stored:?}"
            );

            // And the reader agrees with the store: before the first accepted
            // designation there is no channel resource to report.
            let (status, got) = get_channel(&router).await;
            match &designated {
                Some((expected_topic, expected_role)) => assert_eq!(
                    (
                        status,
                        got["SnsTopicArn"].as_str(),
                        got["SnsRoleName"].as_str()
                    ),
                    (
                        200,
                        Some(expected_topic.as_str()),
                        Some(expected_role.as_str())
                    ),
                    "case {label:?}: the reader must report the last accepted designation, got \
                     {status} with {got}"
                ),
                None => assert_eq!(
                    (status, error_code(&got).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {label:?}: with every designation so far refused there must be no \
                     channel to report, got {status} with {got}"
                ),
            }
        }
    }
}

/// There is ONE form rule: the values the channel designation refuses on form are
/// exactly the values a tagging operation refuses on form, whatever either of
/// them says about any single value.
#[tokio::test]
async fn the_channel_designation_and_the_tagging_check_call_the_same_values_arns() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    let mut refused_by_designation: Vec<String> = Vec::new();
    let mut refused_by_tagging: Vec<String> = Vec::new();
    for case in cases() {
        let (topic, role) = Placement::Topic.pair(&case.value);
        let (_, answered) = put_channel(&router, &topic, &role).await;
        if error_code(&answered) == "ValidationException" {
            refused_by_designation.push(case.value.clone());
        }

        let (_, answered) = call(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceArn": &case.value }),
        )
        .await;
        if error_code(&answered) == "ValidationException" {
            refused_by_tagging.push(case.value.clone());
        }
    }

    assert_eq!(
        refused_by_designation, refused_by_tagging,
        "the two doors onto a caller-supplied ResourceArn must call the same values ARNs: the \
         channel designation refused {refused_by_designation:?}, the tagging check refused \
         {refused_by_tagging:?}"
    );
    assert!(
        !refused_by_designation.is_empty() && refused_by_designation.len() < cases().len(),
        "the table must span both answers, but {} of {} rows were refused",
        refused_by_designation.len(),
        cases().len()
    );
}

/// Authorization is decided BEFORE the payload: with no administrator associated,
/// a designation carrying a value that is not an ARN is refused with the service's
/// authorization refusal — the same answer a well-formed one gets — so the refusal
/// tells an unauthorized caller nothing about its payload, and it stores nothing.
#[tokio::test]
async fn an_unauthorized_designation_is_refused_before_its_arns_are_weighed() {
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        let (topic, role) = Placement::Both.pair(&case.value);
        let (status, body) = put_channel(&router, &topic, &role).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: an ungated designation must be refused on authorization whatever its \
             payload, got {status} with {body}",
            case.name
        );
        let stored = stored_channel(&engine, REGION);
        assert_eq!(
            stored, None,
            "case {:?}: the refused designation must have stored nothing, got {stored:?}",
            case.name
        );
    }
}
