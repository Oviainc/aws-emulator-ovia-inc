// SPDX-License-Identifier: CC0-1.0

//! A PROBE of the awsQuery response envelope the notification operations answer
//! in.
//!
//! The behaviour suite beside this one reads members out of a response; this one
//! probes the envelope those members arrive in. Every operation of this service
//! answers `POST /` with an XML document named after its own action — SES's
//! `awsQuery` protocol (`OPERATIONS.md`: protocol `awsQuery`) — carrying a
//! `<RequestId>` in its `<ResponseMetadata>`.
//!
//! It is a table over the notification operations, so a change that renamed one
//! action's envelope, or dropped the metadata from it, fails here rather than in
//! a client. The `z` prefix keeps this service-wide suite sorted after the
//! per-operation ones.

use crate::harness::{call, element, emulator, field, Field};

#[tokio::test]
async fn every_notification_operation_answers_its_own_query_envelope() {
    let router = emulator();
    let identity = "envelope@example.com";
    let topic = engine::emulator_arn("sns", "us-east-1", "ses-envelope");

    // In order: the identity has to exist before a topic can be set on it, and a
    // topic has to be set before headers or forwarding can be.
    let calls: Vec<(&str, Vec<Field>)> = vec![
        ("VerifyEmailIdentity", vec![field("EmailAddress", identity)]),
        (
            "SetIdentityNotificationTopic",
            vec![
                field("Identity", identity),
                field("NotificationType", "Bounce"),
                field("SnsTopic", topic.clone()),
            ],
        ),
        (
            "SetIdentityHeadersInNotificationsEnabled",
            vec![
                field("Identity", identity),
                field("NotificationType", "Bounce"),
                field("Enabled", "true"),
            ],
        ),
        (
            "SetIdentityFeedbackForwardingEnabled",
            vec![
                field("Identity", identity),
                field("ForwardingEnabled", "true"),
            ],
        ),
        (
            "GetIdentityNotificationAttributes",
            vec![field("Identities.member.1", identity)],
        ),
    ];

    for (action, params) in calls {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {action}: expected 200, got {status} with {body}"
        );
        assert!(
            element(&body, &format!("{action}Response")).is_some(),
            "case {action}: expected a <{action}Response> envelope, got {body}"
        );
        assert!(
            element(&body, "ResponseMetadata")
                .and_then(|metadata| element(&metadata, "RequestId"))
                .is_some_and(|request_id| !request_id.is_empty()),
            "case {action}: expected a RequestId in the response metadata, got {body}"
        );
    }
}
