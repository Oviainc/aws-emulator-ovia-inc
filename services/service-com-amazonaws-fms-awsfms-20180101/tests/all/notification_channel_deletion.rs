// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteNotificationChannel`: what removing the Firewall
//! Manager SNS notification channel does to what a later caller sees.
//!
//! The rule asserted here, in one sentence: the notification channel is a SINGLE
//! configuration resource of the account/Region, so a delete removes whatever
//! channel was designated — leaving nothing for a following read to report and
//! the slot free to be designated again — while a delete that finds no channel
//! to remove answers the modelled `ResourceNotFoundException` and writes
//! nothing; the call is organisation-scoped, so with no Firewall Manager
//! administrator association it is refused before the channel is looked at and
//! the stored channel survives.
//!
//! Every case drives the service over the WIRE through the shared harness in
//! `policy_lifecycle` (an awsJson1_1 `POST /` with an `X-Amz-Target` header and a
//! SigV4 credential scope), and the delete is observed through
//! `GetNotificationChannel` — the implemented reader — as well as in the store,
//! so an assertion cannot pass on a value that never reached persisted state.
//!
//! The table in [`cases`] is the class the rule ranges over: the rows differ in
//! the length of the designated values at BOTH published bounds of
//! `com.amazonaws.fms#ResourceArn` (length 1..1024, from
//! `smithy_query(operation="PutNotificationChannel")`), in the service and the
//! Region segment of the topic ARN, in the role's path form, and in the
//! punctuation the pattern admits; one value past the upper bound is asserted
//! separately.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A second Region to sign a request for, so the administrator refusal is
/// asserted for more than one credential scope.
const OTHER_REGION: &str = "eu-central-1";

/// The published upper bound of `com.amazonaws.fms#ResourceArn` (length 1..1024,
/// pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), from
/// `smithy_query(operation="PutNotificationChannel")`.
const RESOURCE_ARN_MAX: usize = 1024;

/// One row of the table: the pair of values a client designates.
struct Case {
    /// Names the row in every assertion message.
    name: String,
    topic: String,
    role: String,
}

/// The channel class the rules are asserted over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "service-linked role, topic in the signed Region".to_string(),
            topic: format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms-alerts"),
            role: format!(
                "arn:aws:iam::{ACCOUNT}:role/aws-service-role/fms.amazonaws.com/\
                 AWSServiceRoleForFMS"
            ),
        },
        Case {
            name: "topic in another Region, role with a path".to_string(),
            topic: format!("arn:aws:sns:{OTHER_REGION}:{ACCOUNT}:a"),
            role: format!("arn:aws:iam::{ACCOUNT}:role/service-role/fms-sns-role"),
        },
        Case {
            name: "punctuation the ResourceArn pattern admits".to_string(),
            topic: format!("arn:aws:sns:{REGION}:{ACCOUNT}:fms.notifications-with_dots=and+at@"),
            role: format!("arn:aws:iam::{ACCOUNT}:role/fms"),
        },
        Case {
            // The published ResourceArn minimum: one character.
            name: "both values at the published minimum length".to_string(),
            topic: "t".to_string(),
            role: "r".to_string(),
        },
        Case {
            // The published ResourceArn maximum: 1024 characters.
            name: "both values at the published maximum length".to_string(),
            topic: long_value("arn:aws:sns:", RESOURCE_ARN_MAX),
            role: long_value("arn:aws:iam::", RESOURCE_ARN_MAX),
        },
    ]
}

/// A `ResourceArn`-shaped value of exactly `len` characters: `prefix` padded with
/// a character the published pattern admits.
fn long_value(prefix: &str, len: usize) -> String {
    let mut value = prefix.to_string();
    let padding = len.saturating_sub(value.chars().count());
    value.push_str(&"x".repeat(padding));
    value
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

/// Delete the channel over the wire, signed for `REGION`.
async fn delete_channel(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "DeleteNotificationChannel", &json!({})).await
}

/// The stored channel, as the pair of values it holds — read through the same
/// seam a handler uses, so a wire assertion is backed by persisted state.
fn stored_channel(engine: &std::sync::Arc<engine::InMemoryEngine>) -> Option<(String, String)> {
    with_store(engine, |store| {
        store
            .notification_channel
            .as_ref()
            .map(|channel| (channel.sns_topic_arn.clone(), channel.sns_role_name.clone()))
    })
}

/// The rule, on every row: a designated channel round-trips, the delete removes
/// it so the reader reports nothing, a second delete finds nothing to remove,
/// and the slot is free to be designated again.
#[tokio::test]
async fn deleting_the_channel_removes_it_and_frees_the_slot() {
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        let name = &case.name;

        let (status, body) = put_channel(&router, &case.topic, &case.role).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected PutNotificationChannel to answer 200, got {status} with \
             {body}"
        );
        let (status, got) = get_channel(&router).await;
        assert_eq!(
            (status, got["SnsTopicArn"].as_str(), got["SnsRoleName"].as_str()),
            (200, Some(case.topic.as_str()), Some(case.role.as_str())),
            "case {name:?}: the designated channel must read back before the delete, got {status} \
             with {got}"
        );

        // The delete itself: a 200 with an empty document, and the record gone.
        let (status, body) = delete_channel(&router).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeleteNotificationChannel to answer 200, got {status} with \
             {body}"
        );
        let stored = stored_channel(&engine);
        assert_eq!(
            stored, None,
            "case {name:?}: the deleted channel must be gone from state, got {stored:?}"
        );
        let (status, body) = get_channel(&router).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name:?}: a deleted channel must not be readable, got {status} with {body}"
        );

        // Deleting what is already gone finds no channel resource, and changes
        // nothing: a refusal that had written something would answer differently
        // the next time.
        for attempt in 1..=2 {
            let (status, body) = delete_channel(&router).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name:?} attempt {attempt}: a channel cannot be deleted twice, got {status} \
                 with {body}"
            );
            let stored = stored_channel(&engine);
            assert_eq!(
                stored, None,
                "case {name:?} attempt {attempt}: a refused delete must write nothing, got \
                 {stored:?}"
            );
        }

        // The slot is free again: the delete removed the channel, it did not
        // retire the configuration.
        let (status, body) = put_channel(&router, &case.role, &case.topic).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the channel must be designatable again after a delete, got {status} \
             with {body}"
        );
        let stored = stored_channel(&engine);
        assert_eq!(
            stored,
            Some((case.role.clone(), case.topic.clone())),
            "case {name:?}: the re-designated channel must be the one just put, got {stored:?}"
        );
        let (status, body) = delete_channel(&router).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the re-designated channel must delete too, got {status} with {body}"
        );
        let stored = stored_channel(&engine);
        assert_eq!(
            stored, None,
            "case {name:?}: the re-designated channel must be gone after its delete, got \
             {stored:?}"
        );
    }
}

/// With no channel ever designated there is no channel resource to remove, so
/// the delete is refused — and the refusal leaves the account able to designate
/// one.
#[tokio::test]
async fn deleting_a_channel_that_was_never_designated_is_refused() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // The precondition, read through an implemented operation rather than
    // assumed.
    let (status, body) = get_channel(&router).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "no channel must be configured yet, got {status} with {body}"
    );

    let (status, body) = delete_channel(&router).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "deleting a channel that was never designated must be refused, got {status} with {body}"
    );
    let stored = stored_channel(&engine);
    assert_eq!(
        stored, None,
        "the refused delete must have written nothing, got {stored:?}"
    );

    // And nothing about the refusal stops a later designation.
    let topic = format!("arn:aws:sns:{REGION}:{ACCOUNT}:after-a-refused-delete");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/fms-sns");
    let (status, body) = put_channel(&router, &topic, &role).await;
    assert_eq!(
        status, 200,
        "a refused delete must not block a later designation, got {status} with {body}"
    );
    let (status, got) = get_channel(&router).await;
    assert_eq!(
        (status, got["SnsTopicArn"].as_str(), got["SnsRoleName"].as_str()),
        (200, Some(topic.as_str()), Some(role.as_str())),
        "the channel designated after a refused delete must read back, got {status} with {got}"
    );
}

/// The delete is organisation-scoped: with no Firewall Manager administrator
/// association it is refused with the service's authorization refusal — never
/// with the scaffold's 501 and never with a channel-specific error — whichever
/// Region the request is signed for, and the stored channel survives untouched.
#[tokio::test]
async fn deleting_the_channel_is_administrator_gated() {
    for region in [REGION, OTHER_REGION] {
        for case in cases() {
            let (router, engine) = emulator_with_engine();
            let name = format!("{region} / {}", case.name);

            // Seeded through the store, because designating a channel is gated
            // by the very association this case withholds.
            with_store(&engine, |store| {
                store.notification_channel = Some(state::NotificationChannelRecord {
                    sns_topic_arn: case.topic.clone(),
                    sns_role_name: case.role.clone(),
                    configured_at: SystemTime::now(),
                });
            });

            let (status, body) =
                call_in(&router, "DeleteNotificationChannel", &json!({}), region).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {name:?}: an ungated delete must be refused, got {status} with {body}"
            );
            let stored = stored_channel(&engine);
            assert_eq!(
                stored,
                Some((case.topic.clone(), case.role.clone())),
                "case {name:?}: the refused delete must leave the channel stored, got {stored:?}"
            );

            // With the association opened, the same call removes it.
            associate_admin(&router).await;
            let (status, body) =
                call_in(&router, "DeleteNotificationChannel", &json!({}), region).await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected the gated delete to succeed once an administrator is \
                 associated, got {status} with {body}"
            );
            let stored = stored_channel(&engine);
            assert_eq!(
                stored, None,
                "case {name:?}: the channel must be gone after the delete, got {stored:?}"
            );
        }
    }
}

/// One character past the published `ResourceArn` maximum is refused by the
/// modelled constraint before any handler runs, so no channel is designated and
/// the delete still has nothing to remove.
#[tokio::test]
async fn a_value_past_the_published_bound_designates_no_channel_to_delete() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let too_long = long_value("arn:aws:sns:", RESOURCE_ARN_MAX + 1);
    let role = format!("arn:aws:iam::{ACCOUNT}:role/fms-sns");
    let (status, body) = put_channel(&router, &too_long, &role).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a SnsTopicArn of {} characters is past the published maximum, got {status} with {body}",
        too_long.chars().count()
    );
    let stored = stored_channel(&engine);
    assert_eq!(
        stored, None,
        "the refused designation must have stored no channel, got {stored:?}"
    );
    let (status, body) = delete_channel(&router).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "with nothing designated the delete must find no channel, got {status} with {body}"
    );
}
