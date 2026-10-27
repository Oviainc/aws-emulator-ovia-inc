// SPDX-License-Identifier: CC0-1.0

//! Identity notification topics, end to end over the wire.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request: form-urlencoded body in, XML out, one shared engine behind
//! them all. What is asserted is the RULE the two operations follow over a SET of
//! inputs, not one recorded request:
//!
//! * `SetIdentityNotificationTopic` stores the topic on the named identity's
//!   record under the notification type the request names, leaving the other two
//!   types alone, and `GetIdentityNotificationAttributes` reports exactly what was
//!   stored;
//! * an identity is named either by its name or by an ARN naming an identity of
//!   this account in this Region, and both resolve to the same record, which is
//!   the key the report is under; any other ARN names nothing here;
//! * an omitted, empty or blank `SnsTopic` clears the topic and so disables
//!   publishing for that type;
//! * a `SnsTopic` that is not the ARN of an Amazon SNS topic of this account in
//!   this Region is refused with HTTP 400 `InvalidParameterValue` and the stored
//!   topic is left as it was;
//! * headers in notifications are set per type, and only for a type that has a
//!   topic; feedback forwarding is turned off only with a bounce AND a complaint
//!   topic;
//! * an identity this account has not verified is refused and nothing is stored;
//! * only identities in the list are reported, and a call may ask about at most
//!   100 of them;
//! * `GetIdentityNotificationAttributes` computes every member it reports for an
//!   identity from that identity's own stored record, so one call reporting many
//!   differently configured identities answers each of them from its own
//!   settings, and reporting changes nothing a later reader sees.

use crate::harness::{call, element, emulator, field, map_entries, Field};
use service_com_amazonaws_ses_simpleemailservice::ops::NOTIFICATION_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the ARNs in this suite name; nothing here is Region-specific.
const REGION: &str = "us-east-1";

/// `VerifyEmailIdentity`, which is what puts an address in the identity list.
async fn verify_email(router: &MultiServiceRouter, address: &str) {
    let (status, body) = call(
        router,
        "VerifyEmailIdentity",
        &[field("EmailAddress", address)],
    )
    .await;
    assert_eq!(
        status, 200,
        "VerifyEmailIdentity({address:?}): expected 200, got {status} with {body}"
    );
}

/// `VerifyDomainIdentity`, the same for a domain.
async fn verify_domain(router: &MultiServiceRouter, domain: &str) {
    let (status, body) = call(router, "VerifyDomainIdentity", &[field("Domain", domain)]).await;
    assert_eq!(
        status, 200,
        "VerifyDomainIdentity({domain:?}): expected 200, got {status} with {body}"
    );
}

/// `SetIdentityNotificationTopic`, as `(status, body)`. `topic` is `None` when the
/// request leaves `SnsTopic` out altogether.
async fn set_topic(
    router: &MultiServiceRouter,
    identity: &str,
    notification_type: &str,
    topic: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<Field> = vec![
        field("Identity", identity),
        field("NotificationType", notification_type),
    ];
    if let Some(topic) = topic {
        params.push(field("SnsTopic", topic));
    }
    call(router, "SetIdentityNotificationTopic", &params).await
}

/// `SetIdentityNotificationTopic`, asserting it was accepted.
async fn set_topic_ok(
    router: &MultiServiceRouter,
    identity: &str,
    notification_type: &str,
    topic: Option<&str>,
) {
    let (status, body) = set_topic(router, identity, notification_type, topic).await;
    assert_eq!(
        status, 200,
        "case SetIdentityNotificationTopic({identity:?}, {notification_type}, {topic:?}): \
         expected 200, got {status} with {body}"
    );
}

/// `GetIdentityNotificationAttributes`, as `(identity, the attributes element's
/// inner XML)` for every identity the report carries.
async fn notification_attributes(
    router: &MultiServiceRouter,
    identities: &[&str],
) -> Vec<(String, String)> {
    let params: Vec<Field> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| field(format!("Identities.member.{}", index + 1), *identity))
        .collect();
    let (status, body) = call(router, "GetIdentityNotificationAttributes", &params).await;
    assert_eq!(
        status, 200,
        "GetIdentityNotificationAttributes({identities:?}): expected 200, got {status} with {body}"
    );
    map_entries(&body)
}

/// One identity's attributes element, or `None` when the report does not carry it.
async fn attributes_of(router: &MultiServiceRouter, identity: &str) -> Option<String> {
    notification_attributes(router, &[identity])
        .await
        .into_iter()
        .find(|(key, _)| key == identity)
        .map(|(_, attributes)| attributes)
}

/// One member of an attributes element. A member the codec wrote empty reads back
/// as the empty string, which is what "no topic" looks like on the wire.
fn member(attributes: &str, name: &str) -> String {
    element(attributes, name).unwrap_or_default()
}

/// The topic reported for `notification_type`, as the member that type's topic
/// lives in (`IdentityNotificationAttributes$BounceTopic` and its two peers).
fn reported_topic(attributes: &str, notification_type: &str) -> String {
    member(attributes, &format!("{notification_type}Topic"))
}

/// The three topics an identity reports, in the order
/// [`NOTIFICATION_TYPE_VALUES`] lists the types.
async fn reported_topics(router: &MultiServiceRouter, identity: &str) -> Vec<String> {
    let attributes = attributes_of(router, identity)
        .await
        .unwrap_or_else(|| panic!("{identity:?} must be reported"));
    NOTIFICATION_TYPE_VALUES
        .iter()
        .map(|kind| reported_topic(&attributes, kind))
        .collect()
}

/// A topic ARN of the emulator's own account and `REGION`.
fn topic_arn(name: &str) -> String {
    engine::emulator_arn("sns", REGION, name)
}

/// The identity ARN form the `Identity` member documents:
/// `arn:aws:ses:<region>:<account>:identity/<name>`.
fn identity_arn(name: &str) -> String {
    engine::emulator_arn("ses", REGION, &format!("identity/{name}"))
}

#[tokio::test]
async fn a_topic_is_stored_per_notification_type_and_read_back() {
    let router = emulator();
    let identity = "notify@example.com";
    verify_email(&router, identity).await;

    // A freshly verified identity publishes nowhere, and SES forwards its
    // feedback as email until an SNS topic is set for both Bounce and Complaint.
    let fresh = attributes_of(&router, identity)
        .await
        .expect("a verified identity is reported");
    assert_eq!(
        member(&fresh, "ForwardingEnabled"),
        "true",
        "case fresh identity: expected feedback forwarding enabled, got {fresh:?}"
    );
    for kind in NOTIFICATION_TYPE_VALUES {
        assert_eq!(
            reported_topic(&fresh, kind),
            "",
            "case fresh identity ({kind}): expected no topic, got {fresh:?}"
        );
    }

    // One type at a time: every type takes its own topic, and setting one leaves
    // the types set before it alone.
    let mut expected: Vec<String> = NOTIFICATION_TYPE_VALUES.iter().map(|_| String::new()).collect();
    for (index, kind) in NOTIFICATION_TYPE_VALUES.iter().enumerate() {
        let topic = topic_arn(&format!("ses-{}", kind.to_lowercase()));
        set_topic_ok(&router, identity, kind, Some(&topic)).await;
        expected[index] = topic;
        assert_eq!(
            reported_topics(&router, identity).await,
            expected,
            "case {kind}: expected the stored topics {expected:?}"
        );
    }

    // And a later call REPLACES that type's topic rather than adding to it.
    let replacement = topic_arn("ses-bounce-replacement");
    set_topic_ok(&router, identity, "Bounce", Some(&replacement)).await;
    expected[0] = replacement.clone();
    assert_eq!(
        reported_topics(&router, identity).await,
        expected,
        "case replace Bounce: expected {replacement:?} to replace the stored topic"
    );
}

#[tokio::test]
async fn an_identity_is_named_by_its_name_or_by_its_arn() {
    // The `Identity` member's own documentation: "You can specify an identity by
    // using its name or by using its Amazon Resource Name (ARN). The following
    // examples are all valid identities: sender@example.com, example.com,
    // arn:aws:ses:us-east-1:123456789012:identity/example.com."
    struct Case {
        label: &'static str,
        /// The identity as it is verified, which is the key it is reported under.
        name: &'static str,
        /// A domain identity rather than an email address.
        domain: bool,
        /// Whether the Set call names it by ARN.
        set_by_arn: bool,
        /// Whether the Get call names it by ARN.
        get_by_arn: bool,
    }
    let cases = [
        Case {
            label: "an address, by name, read by name",
            name: "plain@example.com",
            domain: false,
            set_by_arn: false,
            get_by_arn: false,
        },
        Case {
            label: "an address, by ARN, read by name",
            name: "arn-set@example.com",
            domain: false,
            set_by_arn: true,
            get_by_arn: false,
        },
        Case {
            label: "an address, by name, read by ARN",
            name: "arn-read@example.com",
            domain: false,
            set_by_arn: false,
            get_by_arn: true,
        },
        Case {
            label: "a domain, by ARN, read by ARN",
            name: "example.com",
            domain: true,
            set_by_arn: true,
            get_by_arn: true,
        },
        Case {
            label: "a subdomain, by ARN, read by name",
            name: "mail.corp.example.net",
            domain: true,
            set_by_arn: true,
            get_by_arn: false,
        },
    ];

    for case in cases {
        let router = emulator();
        if case.domain {
            verify_domain(&router, case.name).await;
        } else {
            verify_email(&router, case.name).await;
        }
        let arn = identity_arn(case.name);
        let set_as = if case.set_by_arn { &arn } else { case.name };
        let get_as = if case.get_by_arn { &arn } else { case.name };
        let topic = topic_arn("ses-identity-forms");
        set_topic_ok(&router, set_as, "Complaint", Some(&topic)).await;

        let reported = notification_attributes(&router, &[get_as]).await;
        // Whichever form the request used, the record is reported under the
        // identity's own name: one identity, one entry.
        assert_eq!(
            reported.len(),
            1,
            "case {}: expected exactly one entry, got {reported:?}",
            case.label
        );
        let (key, attributes) = &reported[0];
        assert_eq!(
            key, case.name,
            "case {}: expected the entry keyed by the identity name, got {key:?}",
            case.label
        );
        assert_eq!(
            reported_topic(attributes, "Complaint"),
            topic,
            "case {}: expected the stored topic, got {attributes:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn an_omitted_or_blank_topic_clears_that_types_topic() {
    // `SnsTopic`'s own documentation: "If the parameter is omitted from the
    // request or a null value is passed, SnsTopic is cleared and publishing is
    // disabled." On this wire a null value arrives as an empty field.
    let clearing_values = [
        ("omitted", None),
        ("empty", Some("")),
        ("blank", Some("   ")),
    ];

    for (label, cleared_with) in clearing_values {
        let router = emulator();
        let identity = "clears@example.com";
        verify_email(&router, identity).await;

        // Every type carries a topic, so clearing one is visible against the
        // other two staying put.
        let topics: Vec<String> = NOTIFICATION_TYPE_VALUES
            .iter()
            .map(|kind| topic_arn(&format!("ses-clear-{}", kind.to_lowercase())))
            .collect();
        for (kind, topic) in NOTIFICATION_TYPE_VALUES.iter().zip(&topics) {
            set_topic_ok(&router, identity, kind, Some(topic)).await;
        }

        for (index, kind) in NOTIFICATION_TYPE_VALUES.iter().enumerate() {
            set_topic_ok(&router, identity, kind, cleared_with).await;
            let mut expected = topics.clone();
            for slot in expected.iter_mut().take(index + 1) {
                slot.clear();
            }
            assert_eq!(
                reported_topics(&router, identity).await,
                expected,
                "case {label} ({kind}): expected the cleared topics {expected:?}"
            );
        }

        // Clearing is idempotent, and a cleared type takes a topic again.
        set_topic_ok(&router, identity, "Bounce", cleared_with).await;
        let reclaimed = topic_arn("ses-clear-reclaimed");
        set_topic_ok(&router, identity, "Bounce", Some(&reclaimed)).await;
        assert_eq!(
            reported_topics(&router, identity).await[0],
            reclaimed,
            "case {label}: a cleared type must take a topic again"
        );
    }
}

#[tokio::test]
async fn only_an_sns_topic_arn_of_this_account_and_region_is_accepted_as_a_topic() {
    // The Amazon SNS topic ARN form, which `SnsTopic` is documented as carrying
    // ("The Amazon Resource Name (ARN) of the Amazon SNS topic"):
    // `arn:<partition>:sns:<region>:<account-id>:<topic-name>` — six
    // colon-separated fields, an AWS partition, the vendor `sns`, a Region label,
    // a twelve-digit account id, and the topic NAME alone as the resource (a
    // subscription ARN carries a further `:<subscription-id>`). Amazon SNS
    // documents the name itself: "Topic names must be made up of only uppercase
    // and lowercase ASCII letters, numbers, underscores, and hyphens, and must be
    // between 1 and 256 characters long." The topic also has to be one SES could
    // publish to for this caller: in the Region the request is scoped to, and in
    // the one account this emulator serves.
    let other_account = "123456789012";
    let accepted = [
        ("a topic of this account", topic_arn("ses-accepted")),
        (
            "a topic name of digits, underscores and hyphens",
            topic_arn("ses_notifications-2024"),
        ),
        (
            "a topic name of one character",
            topic_arn("t"),
        ),
        (
            "a topic name of the longest documented length",
            topic_arn(&"t".repeat(256)),
        ),
    ];
    let refused = [
        ("not an ARN at all", "ses-notifications".to_string()),
        ("an ARN of another vendor", {
            engine::emulator_arn("sqs", REGION, "ses-notifications")
        }),
        (
            "an ARN of this service",
            identity_arn("notify@example.com"),
        ),
        (
            "a subscription ARN",
            format!("{}:7b0b2b1e-1111-2222-3333-444455556666", topic_arn("ses-notifications")),
        ),
        (
            "an ARN with no topic name",
            format!("arn:aws:sns:{REGION}:{other_account}:"),
        ),
        (
            "an ARN with no Region",
            format!("arn:aws:sns::{other_account}:ses-notifications"),
        ),
        (
            "an ARN with no account",
            format!("arn:aws:sns:{REGION}::ses-notifications"),
        ),
        (
            "an ARN with too few fields",
            format!("arn:aws:sns:{REGION}:{other_account}"),
        ),
        (
            "a topic name that is not an ARN",
            "sns:ses-notifications".to_string(),
        ),
        (
            "an ARN naming a path under a topic",
            format!("arn:aws:sns:{REGION}:{other_account}:team/ses-notifications"),
        ),
        (
            "an ARN of no AWS partition",
            format!("arn:bogus:sns:{REGION}:{other_account}:ses-notifications"),
        ),
        (
            "an account id that is not twelve digits",
            format!("arn:aws:sns:{REGION}:12345678901:ses-notifications"),
        ),
        (
            "an account id that is not digits",
            format!("arn:aws:sns:{REGION}:my-account:ses-notifications"),
        ),
        (
            "a Region that is no Region label",
            format!("arn:aws:sns:US-East-1:{other_account}:ses-notifications"),
        ),
        (
            "a topic name carrying a space",
            format!("arn:aws:sns:{REGION}:{other_account}:ses notifications"),
        ),
        (
            "a topic name carrying a dot",
            format!("arn:aws:sns:{REGION}:{other_account}:ses.notifications"),
        ),
        (
            "a topic name one character past the documented length",
            format!("arn:aws:sns:{REGION}:{other_account}:{}", "t".repeat(257)),
        ),
        (
            "a topic of another account",
            format!("arn:aws:sns:{REGION}:{other_account}:ses-notifications"),
        ),
        (
            "a topic of another Region",
            engine::emulator_arn("sns", "eu-west-3", "ses-notifications"),
        ),
        (
            "a topic of another partition",
            format!(
                "arn:aws-cn:sns:cn-north-1:{}:ses-notifications",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];

    let router = emulator();
    let identity = "topics@example.com";
    verify_email(&router, identity).await;

    for (label, topic) in &accepted {
        set_topic_ok(&router, identity, "Delivery", Some(topic)).await;
        assert_eq!(
            reported_topics(&router, identity).await[2],
            *topic,
            "case {label} ({topic:?}): expected the topic to be stored"
        );
    }

    // The last accepted topic is what every refusal must leave in place.
    let kept = accepted
        .last()
        .map(|(_, topic)| topic.clone())
        .expect("the accepted table is not empty");
    for (label, topic) in &refused {
        let (status, body) = set_topic(&router, identity, "Delivery", Some(topic)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {label} ({topic:?}): expected 400 InvalidParameterValue, got {status} with \
             {body}"
        );
        assert_eq!(
            reported_topics(&router, identity).await[2],
            kept,
            "case {label} ({topic:?}): a refused request must leave the stored topic alone"
        );
    }
}

#[tokio::test]
async fn an_identity_this_account_has_not_verified_is_refused() {
    let router = emulator();
    // One identity IS verified, so the refusals below are about the identity the
    // request names and not about an empty store.
    verify_email(&router, "verified@example.com").await;
    // And one is verified and then deleted, which must read as unverified again.
    verify_email(&router, "deleted@example.com").await;
    let (status, body) = call(
        &router,
        "DeleteIdentity",
        &[field("Identity", "deleted@example.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteIdentity: expected 200, got {status} with {body}"
    );

    let unverified = [
        ("an address never verified", "stranger@example.com".to_string()),
        ("a domain never verified", "example.org".to_string()),
        (
            "an address verified then deleted",
            "deleted@example.com".to_string(),
        ),
        (
            "an ARN naming an unverified identity",
            identity_arn("stranger@example.com"),
        ),
        (
            "a subdomain of no verified domain",
            "mail.example.org".to_string(),
        ),
    ];

    for (label, identity) in &unverified {
        let topic = topic_arn("ses-unverified");
        let (status, body) = set_topic(&router, identity, "Bounce", Some(&topic)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {label} ({identity:?}): expected 400 InvalidParameterValue, got {status} with \
             {body}"
        );
        let reported = notification_attributes(&router, &[identity]).await;
        assert!(
            reported.is_empty(),
            "case {label} ({identity:?}): nothing may be stored for an unverified identity, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_notification_type_outside_the_models_value_set_is_refused() {
    let router = emulator();
    let identity = "types@example.com";
    verify_email(&router, identity).await;
    let stored = topic_arn("ses-type-set");
    set_topic_ok(&router, identity, "Bounce", Some(&stored)).await;

    // Values differing from `NOTIFICATION_TYPE_VALUES` in case, in spelling and
    // in being absent from it at all. The model declares the set, so such a
    // value may be refused at the boundary with the protocol's own code; what
    // this pins is that it is refused and stores nothing.
    let outside = ["bounce", "BOUNCE", "Open", "Bounce,Complaint", ""];
    for value in outside {
        let topic = topic_arn("ses-type-refused");
        let (status, body) = set_topic(&router, identity, value, Some(&topic)).await;
        assert_eq!(
            status, 400,
            "case NotificationType={value:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            reported_topics(&router, identity).await,
            vec![stored.clone(), String::new(), String::new()],
            "case NotificationType={value:?}: a refused request must store nothing"
        );
    }
}

/// `SetIdentityHeadersInNotificationsEnabled`, as `(status, body)`.
async fn set_headers_enabled(
    router: &MultiServiceRouter,
    identity: &str,
    notification_type: &str,
    enabled: bool,
) -> (u16, String) {
    call(
        router,
        "SetIdentityHeadersInNotificationsEnabled",
        &[
            field("Identity", identity),
            field("NotificationType", notification_type),
            field("Enabled", if enabled { "true" } else { "false" }),
        ],
    )
    .await
}

/// `SetIdentityFeedbackForwardingEnabled`, as `(status, body)`.
async fn set_forwarding_enabled(
    router: &MultiServiceRouter,
    identity: &str,
    enabled: bool,
) -> (u16, String) {
    call(
        router,
        "SetIdentityFeedbackForwardingEnabled",
        &[
            field("Identity", identity),
            field("ForwardingEnabled", if enabled { "true" } else { "false" }),
        ],
    )
    .await
}

/// The header flag reported for `notification_type`
/// (`IdentityNotificationAttributes$HeadersInBounceNotificationsEnabled` and its
/// two peers).
fn reported_headers_flag(attributes: &str, notification_type: &str) -> String {
    member(
        attributes,
        &format!("HeadersIn{notification_type}NotificationsEnabled"),
    )
}

#[tokio::test]
async fn headers_in_notifications_are_set_per_type_and_only_where_a_topic_is_set() {
    let router = emulator();
    let identity = "headers@example.com";
    verify_email(&router, identity).await;

    // "This value can only be set when NotificationType is already set to use a
    // particular Amazon SNS topic" (the `Enabled` member's own documentation):
    // with no topic for that type, both values are refused and nothing is
    // stored.
    for kind in NOTIFICATION_TYPE_VALUES {
        for enabled in [true, false] {
            let (status, body) = set_headers_enabled(&router, identity, kind, enabled).await;
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("InvalidParameterValue")),
                "case {kind} enabled={enabled} with no topic: expected 400 \
                 InvalidParameterValue, got {status} with {body}"
            );
        }
        let attributes = attributes_of(&router, identity)
            .await
            .expect("a verified identity is reported");
        assert_eq!(
            reported_headers_flag(&attributes, kind),
            "false",
            "case {kind} with no topic: the flag must stay off, got {attributes:?}"
        );
    }

    // With a topic set for one type, that type's flag follows the request and the
    // other two stay off.
    for kind in NOTIFICATION_TYPE_VALUES {
        let topic = topic_arn(&format!("ses-headers-{}", kind.to_lowercase()));
        set_topic_ok(&router, identity, kind, Some(&topic)).await;
        for enabled in [true, false, true] {
            let (status, body) = set_headers_enabled(&router, identity, kind, enabled).await;
            assert_eq!(
                status, 200,
                "case {kind} enabled={enabled}: expected 200, got {status} with {body}"
            );
            let attributes = attributes_of(&router, identity)
                .await
                .expect("a verified identity is reported");
            let expected: Vec<String> = NOTIFICATION_TYPE_VALUES
                .iter()
                .map(|other| {
                    let on = *other == *kind && enabled;
                    // Only the type just written is on: a type whose turn has
                    // not come has no topic yet, and one already done was left
                    // on `true` by its last row.
                    let done = NOTIFICATION_TYPE_VALUES
                        .iter()
                        .position(|candidate| candidate == other)
                        < NOTIFICATION_TYPE_VALUES
                            .iter()
                            .position(|candidate| candidate == kind);
                    if on || done { "true" } else { "false" }.to_string()
                })
                .collect();
            let reported: Vec<String> = NOTIFICATION_TYPE_VALUES
                .iter()
                .map(|other| reported_headers_flag(&attributes, other))
                .collect();
            assert_eq!(
                reported, expected,
                "case {kind} enabled={enabled}: expected the flags {expected:?}"
            );
        }
    }
}

#[tokio::test]
async fn feedback_forwarding_is_disabled_only_with_a_bounce_and_a_complaint_topic() {
    // "Feedback forwarding can only be disabled when Amazon Simple Notification
    // Service (Amazon SNS) topics are specified for both bounces and complaints"
    // (the operation's own documentation).
    struct Case {
        label: &'static str,
        /// Which notification types carry a topic before forwarding is set off.
        topics: &'static [&'static str],
        /// Whether turning forwarding off is accepted.
        accepted: bool,
    }
    let cases = [
        Case {
            label: "no topic at all",
            topics: &[],
            accepted: false,
        },
        Case {
            label: "a bounce topic only",
            topics: &["Bounce"],
            accepted: false,
        },
        Case {
            label: "a complaint topic only",
            topics: &["Complaint"],
            accepted: false,
        },
        Case {
            label: "a delivery topic only",
            topics: &["Delivery"],
            accepted: false,
        },
        Case {
            label: "a bounce and a complaint topic",
            topics: &["Bounce", "Complaint"],
            accepted: true,
        },
        Case {
            label: "a topic for every type",
            topics: &["Bounce", "Complaint", "Delivery"],
            accepted: true,
        },
    ];

    for case in cases {
        let router = emulator();
        let identity = "forwarding@example.com";
        verify_email(&router, identity).await;
        for kind in case.topics {
            let topic = topic_arn(&format!("ses-forwarding-{}", kind.to_lowercase()));
            set_topic_ok(&router, identity, kind, Some(&topic)).await;
        }

        // Turning forwarding ON is always allowed: it needs no topic.
        let (status, body) = set_forwarding_enabled(&router, identity, true).await;
        assert_eq!(
            status, 200,
            "case {}: enabling forwarding must be accepted, got {status} with {body}",
            case.label
        );

        let (status, body) = set_forwarding_enabled(&router, identity, false).await;
        let attributes = attributes_of(&router, identity)
            .await
            .expect("a verified identity is reported");
        let reported = member(&attributes, "ForwardingEnabled");
        if case.accepted {
            assert_eq!(
                (status, reported.as_str()),
                (200, "false"),
                "case {}: expected forwarding off, got {status} with {body}",
                case.label
            );
            // And back on again, from the stored state.
            let (status, body) = set_forwarding_enabled(&router, identity, true).await;
            let attributes = attributes_of(&router, identity)
                .await
                .expect("a verified identity is reported");
            assert_eq!(
                (status, member(&attributes, "ForwardingEnabled").as_str()),
                (200, "true"),
                "case {}: expected forwarding on again, got {status} with {body}",
                case.label
            );
        } else {
            assert_eq!(
                (status, element(&body, "Code").as_deref()),
                (400, Some("InvalidParameterValue")),
                "case {}: expected 400 InvalidParameterValue, got {status} with {body}",
                case.label
            );
            assert_eq!(
                reported, "true",
                "case {}: a refused request must leave forwarding on, got {attributes:?}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn the_notification_setters_refuse_an_unverified_identity() {
    let router = emulator();
    verify_email(&router, "verified@example.com").await;
    let stranger = "stranger@example.com";

    let (status, body) = set_headers_enabled(&router, stranger, "Bounce", true).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("InvalidParameterValue")),
        "case SetIdentityHeadersInNotificationsEnabled({stranger:?}): expected 400 \
         InvalidParameterValue, got {status} with {body}"
    );
    let (status, body) = set_forwarding_enabled(&router, stranger, true).await;
    assert_eq!(
        (status, element(&body, "Code").as_deref()),
        (400, Some("InvalidParameterValue")),
        "case SetIdentityFeedbackForwardingEnabled({stranger:?}): expected 400 \
         InvalidParameterValue, got {status} with {body}"
    );
    let reported = notification_attributes(&router, &[stranger]).await;
    assert!(
        reported.is_empty(),
        "case an unverified identity: nothing may be stored, got {reported:?}"
    );
}

#[tokio::test]
async fn an_arn_naming_no_identity_of_this_account_is_refused() {
    // An ARN names one account's resource in one Region, and this emulator serves
    // a single account, so an `Identity` given as an ARN has to name an identity
    // of THIS account in the Region the request is scoped to. Anything else names
    // nothing here — and must not be read as a bare identity name either, or a
    // foreign ARN would silently reach the local identity it happens to spell.
    let router = emulator();
    let identity = "arns@example.com";
    verify_email(&router, identity).await;
    let stored = topic_arn("ses-arn-identity");
    set_topic_ok(&router, identity, "Bounce", Some(&stored)).await;

    let refused = [
        (
            "an identity ARN of another account",
            format!("arn:aws:ses:{REGION}:123456789012:identity/{identity}"),
        ),
        (
            "an identity ARN of another Region",
            engine::emulator_arn("ses", "eu-west-3", &format!("identity/{identity}")),
        ),
        (
            "an identity ARN of another partition",
            format!(
                "arn:aws-cn:ses:cn-north-1:{}:identity/{identity}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "an ARN of another vendor naming the identity",
            engine::emulator_arn("sqs", REGION, &format!("identity/{identity}")),
        ),
        (
            "an ARN of another resource of this service",
            engine::emulator_arn("ses", REGION, &format!("configuration-set/{identity}")),
        ),
        (
            "an identity ARN naming no identity",
            engine::emulator_arn("ses", REGION, "identity/"),
        ),
    ];

    for (label, named) in &refused {
        let topic = topic_arn("ses-arn-refused");
        let (status, body) = set_topic(&router, named, "Bounce", Some(&topic)).await;
        assert_eq!(
            (status, element(&body, "Code").as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {label} ({named:?}): expected 400 InvalidParameterValue, got {status} with \
             {body}"
        );
        assert_eq!(
            reported_topics(&router, identity).await[0],
            stored,
            "case {label} ({named:?}): a refused request must leave the stored topic alone"
        );
        // And the report carries no entry for it, the same as for an identity
        // that is not in the list at all.
        let reported = notification_attributes(&router, &[named]).await;
        assert!(
            reported.is_empty(),
            "case {label} ({named:?}): expected no attributes, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_reported_member_comes_from_the_named_identitys_own_record() {
    /// One identity, configured through the three setters, and what the report
    /// therefore has to say about it.
    struct ReportCase {
        label: &'static str,
        /// The identity as it is verified, which is the key it is reported under.
        name: &'static str,
        /// A domain identity rather than an email address.
        domain: bool,
        /// The notification types a topic is stored for.
        topics: &'static [&'static str],
        /// The header flag written per type, in order: a type written twice keeps
        /// the value written LAST, and a type absent here keeps its default.
        headers: &'static [(&'static str, bool)],
        /// What feedback forwarding is left set to.
        forwarding: bool,
    }

    /// The topic this suite stores for one identity's `kind` notifications.
    fn stored_topic(identity: &str, kind: &str) -> String {
        topic_arn(&format!(
            "ses-report-{}-{}",
            identity.replace(['@', '.'], "-"),
            kind.to_lowercase()
        ))
    }

    /// Every member of `IdentityNotificationAttributes`, read off a reported
    /// attributes element as `(member name, value)`.
    fn reported_members(attributes: &str) -> Vec<(String, String)> {
        let mut read = vec![(
            "ForwardingEnabled".to_string(),
            member(attributes, "ForwardingEnabled"),
        )];
        for kind in NOTIFICATION_TYPE_VALUES {
            read.push((format!("{kind}Topic"), reported_topic(attributes, kind)));
            read.push((
                format!("HeadersIn{kind}NotificationsEnabled"),
                reported_headers_flag(attributes, kind),
            ));
        }
        read
    }

    /// The same members, as the case's stored settings require them to read.
    fn expected_members(case: &ReportCase) -> Vec<(String, String)> {
        let mut expected = vec![(
            "ForwardingEnabled".to_string(),
            case.forwarding.to_string(),
        )];
        for kind in NOTIFICATION_TYPE_VALUES {
            let topic = if case.topics.contains(kind) {
                stored_topic(case.name, kind)
            } else {
                String::new()
            };
            let flag = case
                .headers
                .iter()
                .filter(|(written, _)| written == kind)
                .map(|(_, enabled)| *enabled)
                .next_back()
                .unwrap_or(false);
            expected.push((format!("{kind}Topic"), topic));
            expected.push((
                format!("HeadersIn{kind}NotificationsEnabled"),
                flag.to_string(),
            ));
        }
        expected
    }

    let cases = [
        ReportCase {
            label: "an address nobody configured",
            name: "fresh@example.com",
            domain: false,
            topics: &[],
            headers: &[],
            forwarding: true,
        },
        ReportCase {
            label: "an address publishing bounces, with their headers on",
            name: "bounces@example.com",
            domain: false,
            topics: &["Bounce"],
            headers: &[("Bounce", true)],
            forwarding: true,
        },
        ReportCase {
            label: "an address publishing every class, forwarding off",
            name: "every-class@example.com",
            domain: false,
            topics: &["Bounce", "Complaint", "Delivery"],
            headers: &[("Complaint", true), ("Delivery", true)],
            forwarding: false,
        },
        ReportCase {
            label: "a domain publishing complaints, headers turned back off",
            name: "example.com",
            domain: true,
            topics: &["Complaint"],
            headers: &[("Complaint", true), ("Complaint", false)],
            forwarding: true,
        },
        ReportCase {
            label: "a subdomain publishing deliveries, with their headers on",
            name: "mail.corp.example.net",
            domain: true,
            topics: &["Delivery"],
            headers: &[("Delivery", true)],
            forwarding: true,
        },
        ReportCase {
            label: "an address with every topic and every header flag on",
            name: "loud@example.com",
            domain: false,
            topics: &["Bounce", "Complaint", "Delivery"],
            headers: &[("Bounce", true), ("Complaint", true), ("Delivery", true)],
            forwarding: true,
        },
    ];

    let router = emulator();
    for case in &cases {
        if case.domain {
            verify_domain(&router, case.name).await;
        } else {
            verify_email(&router, case.name).await;
        }
        for kind in case.topics {
            set_topic_ok(
                &router,
                case.name,
                kind,
                Some(&stored_topic(case.name, kind)),
            )
            .await;
        }
        for (kind, enabled) in case.headers {
            let (status, body) = set_headers_enabled(&router, case.name, kind, *enabled).await;
            assert_eq!(
                status, 200,
                "case {}: SetIdentityHeadersInNotificationsEnabled({kind}, {enabled}) must be \
                 accepted, got {status} with {body}",
                case.label
            );
        }
        let (status, body) = set_forwarding_enabled(&router, case.name, case.forwarding).await;
        assert_eq!(
            status, 200,
            "case {}: SetIdentityFeedbackForwardingEnabled({}) must be accepted, got {status} \
             with {body}",
            case.label, case.forwarding
        );
    }

    // ONE call naming every configured identity — each of them twice, once by
    // name and once by the ARN form `Identity` documents — beside two identities
    // this account never verified. One identity, one entry, from its own record.
    let mut named: Vec<String> = Vec::new();
    for case in &cases {
        named.push(case.name.to_string());
        named.push(identity_arn(case.name));
    }
    named.push("stranger@example.com".to_string());
    named.push("example.org".to_string());
    let listed: Vec<&str> = named.iter().map(String::as_str).collect();

    let reported = notification_attributes(&router, &listed).await;
    assert_eq!(
        reported.len(),
        cases.len(),
        "case the whole report: expected one entry per verified identity, got {reported:?}"
    );
    for case in &cases {
        let (_, attributes) = reported
            .iter()
            .find(|(key, _)| key == case.name)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected an entry for {:?}, got {reported:?}",
                    case.label, case.name
                )
            });
        let expected = expected_members(case);
        assert_eq!(
            reported_members(attributes),
            expected,
            "case {}: expected the stored settings {expected:?}, got {attributes:?}",
            case.label
        );
    }

    // Reporting is a READ: asking again answers the same, whether the call names
    // every identity or just one of them.
    let sorted = |mut entries: Vec<(String, String)>| {
        entries.sort();
        entries
    };
    let again = notification_attributes(&router, &listed).await;
    assert_eq!(
        sorted(again.clone()),
        sorted(reported.clone()),
        "case a second report: expected the first report unchanged, got {again:?}"
    );
    for case in &cases {
        let alone = attributes_of(&router, case.name)
            .await
            .unwrap_or_else(|| panic!("case {}: expected {:?} to be reported", case.label, case.name));
        let expected = expected_members(case);
        assert_eq!(
            reported_members(&alone),
            expected,
            "case {}: expected one identity's report to still read {expected:?}, got {alone:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn only_identities_in_the_list_are_reported_and_at_most_a_hundred_per_call() {
    let router = emulator();
    let with_topic = "reported@example.com";
    let without_topic = "quiet@example.com";
    verify_email(&router, with_topic).await;
    verify_email(&router, without_topic).await;
    let topic = topic_arn("ses-reported");
    set_topic_ok(&router, with_topic, "Bounce", Some(&topic)).await;

    let reported = notification_attributes(
        &router,
        &[with_topic, without_topic, "unknown@example.com", "example.net"],
    )
    .await;
    let keys: Vec<&str> = reported.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys.len(),
        2,
        "case mixed identities: expected only the two verified identities, got {keys:?}"
    );
    assert!(
        keys.contains(&with_topic) && keys.contains(&without_topic),
        "case mixed identities: expected both verified identities, got {keys:?}"
    );

    // The verified identity nobody set a topic for is reported with its
    // defaults, not omitted.
    let quiet = attributes_of(&router, without_topic)
        .await
        .expect("a verified identity with no topics is still reported");
    assert_eq!(
        (
            reported_topic(&quiet, "Bounce"),
            member(&quiet, "ForwardingEnabled")
        ),
        (String::new(), "true".to_string()),
        "case no topics: expected an empty topic and forwarding enabled, got {quiet:?}"
    );

    // "Can only get … attributes for up to 100 identities at a time" (each
    // operation's own documentation): the bound itself is served, one past it is
    // refused — and both operations that report per-identity attributes answer
    // that one refusal with the same code, from the one shared bound.
    let many: Vec<String> = (0..101).map(|index| format!("bulk{index}@example.com")).collect();
    for action in [
        "GetIdentityNotificationAttributes",
        "GetIdentityVerificationAttributes",
    ] {
        for (count, expected_status) in [(100usize, 200u16), (101, 400)] {
            let params: Vec<Field> = many
                .iter()
                .take(count)
                .enumerate()
                .map(|(index, identity)| {
                    field(format!("Identities.member.{}", index + 1), identity)
                })
                .collect();
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, expected_status,
                "case {action} with {count} identities: expected {expected_status}, got {status} \
                 with {body}"
            );
            if expected_status == 400 {
                assert_eq!(
                    element(&body, "Code").as_deref(),
                    Some("InvalidParameterValue"),
                    "case {action} with {count} identities: expected InvalidParameterValue, got \
                     {body}"
                );
            }
        }
    }
}
