// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every SCALAR enum member of this service is weighed by, driven end to
//! end over the wire through each operation that carries one.
//!
//! A member whose shape is an enumerated type admits only the values that enum
//! declares, so a request carrying any other value is refused with HTTP 400
//! `ValidationException` naming the member and the value set it failed — in the
//! validation envelope every constraint refusal in this emulator is worded with — and
//! nothing is written. The rule is one rule, so every operation that carries such a
//! member is held to it together rather than to a copy of it:
//!
//! * `DeleteCampaignChannelSubtypeConfig`'s `channelSubtype`,
//! * the `config` of `DeleteCampaignCommunicationLimits`,
//! * the `config` of `DeleteCampaignCommunicationTime`,
//! * `DeleteConnectInstanceConfig`'s `campaignDeletionPolicy`, which the model leaves
//!   OPTIONAL, so a request that sends no value at all has nothing to weigh,
//! * the `encryptionType` of `StartInstanceOnboardingJob`'s encryption config, a
//!   member nested inside a structure and likewise optional.
//!
//! Two further halves of the same rule are asserted here: the value is weighed before
//! the resource the request names is resolved, so an id nothing answers to is still
//! the bad request the value makes it; and every value the model's own catalogs hold is
//! SERVED, so the rule refuses only what a catalog leaves out. The last case holds the
//! handlers' own shared check — the backstop behind the boundary walk, which the five
//! operations now reach the same way — to that same rule for all five members.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    CAMPAIGN_DELETION_POLICY_VALUES, CHANNEL_SUBTYPE_VALUES,
    COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES, COMMUNICATION_TIME_CONFIG_TYPE_VALUES,
    ENCRYPTION_TYPE_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::enum_value_violation;

use super::common::{call, create_campaign, error_code, onboard_instance, router, Harness};

/// The region every case signs for; nothing in this rule is region-dependent, so one
/// is enough.
const REGION: &str = "us-west-2";

/// A KMS key ARN, which `EncryptionConfig`'s own documentation requires whenever
/// encryption is enabled — so a case about the TYPE is not refused for a missing key.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The most of a caller's value a refusal echoes, pinned to the ceiling the emulator's
/// refusals share (`MAX_ECHOED_VALUE` in `aws_smithy_schema_server::constraints`), so a
/// huge member cannot be amplified into a huge response.
const MAX_ECHOED_VALUE: usize = 256;

/// Which resource an operation's path names, and so what a case's target id has to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    Campaign,
    ConnectInstance,
}

/// One scalar enum member this service holds to a catalog of the model's own: the
/// operation it arrives on, the resource that operation's path names, the member path a
/// refusal names, and the member's catalog in both the forms this suite needs.
///
/// `allowed` is the catalog as this crate's `ops::types` renders it, which is what the
/// handlers hold a value against; `declared` is the same set in the order the model
/// declares it (`smithy_query(query="shape", shape="ChannelSubtype,...")`, whose
/// `values` this mirrors), which is the order a refusal lists the value set in. The
/// member paths are the ones `smithy_query(query="operation", operation="...")` reports
/// for each request.
#[derive(Debug, Clone, Copy)]
struct EnumMember {
    operation: &'static str,
    resource: Resource,
    member: &'static str,
    allowed: &'static [&'static str],
    declared: &'static [&'static str],
}

/// Every operation of this service that carries a scalar enum member of its own.
const ENUM_MEMBERS: &[EnumMember] = &[
    EnumMember {
        operation: "DeleteCampaignChannelSubtypeConfig",
        resource: Resource::Campaign,
        member: "channelSubtype",
        allowed: CHANNEL_SUBTYPE_VALUES,
        declared: &["TELEPHONY", "SMS", "EMAIL", "WHATSAPP"],
    },
    EnumMember {
        operation: "DeleteCampaignCommunicationLimits",
        resource: Resource::Campaign,
        member: "config",
        allowed: COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES,
        declared: &["ALL_CHANNEL_SUBTYPES"],
    },
    EnumMember {
        operation: "DeleteCampaignCommunicationTime",
        resource: Resource::Campaign,
        member: "config",
        allowed: COMMUNICATION_TIME_CONFIG_TYPE_VALUES,
        declared: &["TELEPHONY", "SMS", "EMAIL", "WHATSAPP"],
    },
    EnumMember {
        operation: "DeleteConnectInstanceConfig",
        resource: Resource::ConnectInstance,
        member: "campaignDeletionPolicy",
        allowed: CAMPAIGN_DELETION_POLICY_VALUES,
        declared: &["RETAIN_ALL", "DELETE_ALL"],
    },
    EnumMember {
        operation: "StartInstanceOnboardingJob",
        resource: Resource::ConnectInstance,
        member: "encryptionConfig.encryptionType",
        allowed: ENCRYPTION_TYPE_VALUES,
        declared: &["KMS"],
    },
];

/// The `?<member>=<value>` a query-bound member travels in, and nothing at all when the
/// request leaves the member out.
fn query(member: &str, value: Option<&str>) -> String {
    match value {
        Some(value) => format!("?{member}={value}"),
        None => String::new(),
    }
}

/// The message a response carries.
fn message_of(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The whole sentence a value outside `allowed` at `member` is refused with: the
/// validation envelope, carrying the one clause for that member, the value as sent and
/// the value set it failed.
///
/// A value past [`MAX_ECHOED_VALUE`] is echoed as a prefix of that many bytes followed
/// by the value's own length, which is how every refusal in this emulator shows an
/// over-long value.
fn expected_message(member: &str, allowed: &[&str], value: &str) -> String {
    let echoed = if value.len() <= MAX_ECHOED_VALUE {
        value.to_string()
    } else {
        format!("{}... [{} bytes]", &value[..MAX_ECHOED_VALUE], value.len())
    };
    format!(
        "1 validation error detected: Value '{echoed}' at '{member}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        allowed.join(", ")
    )
}

/// Drive `member`'s operation against `target` — a campaign id or a Connect instance
/// id, whichever its path names — carrying `value` for the enum member, or leaving that
/// member out when `value` is `None`.
///
/// The method and URI of each request are the ones the model binds its operation to.
async fn drive(
    router: &Harness,
    target: &str,
    member: &EnumMember,
    value: Option<&str>,
) -> (u16, Value) {
    let parameter = query(member.member, value);
    match member.operation {
        "DeleteCampaignChannelSubtypeConfig" => {
            call(
                router,
                REGION,
                "DELETE",
                &format!("/v2/campaigns/{target}/channel-subtype-config{parameter}"),
                &Value::Null,
            )
            .await
        }
        "DeleteCampaignCommunicationLimits" => {
            call(
                router,
                REGION,
                "DELETE",
                &format!("/v2/campaigns/{target}/communication-limits{parameter}"),
                &Value::Null,
            )
            .await
        }
        "DeleteCampaignCommunicationTime" => {
            call(
                router,
                REGION,
                "DELETE",
                &format!("/v2/campaigns/{target}/communication-time{parameter}"),
                &Value::Null,
            )
            .await
        }
        "DeleteConnectInstanceConfig" => {
            call(
                router,
                REGION,
                "DELETE",
                &format!("/v2/connect-instance/{target}/config{parameter}"),
                &Value::Null,
            )
            .await
        }
        "StartInstanceOnboardingJob" => {
            // `encryptionConfig` is required, and its `enabled` member is what decides
            // whether a key is needed; the TYPE is the member under test, so a case
            // that sends no type leaves exactly that member out.
            let mut config = json!({"enabled": true, "keyArn": KMS_KEY_ARN});
            if let Some(value) = value {
                config["encryptionType"] = json!(value);
            }
            call(
                router,
                REGION,
                "PUT",
                &format!("/v2/connect-instance/{target}/onboarding"),
                &json!({"encryptionConfig": config}),
            )
            .await
        }
        other => panic!("{other} carries no scalar enum member this suite drives"),
    }
}

/// How a value can miss a catalog, built FROM that catalog so every row holds for every
/// member rather than for one member's spelling.
///
/// The rows differ along what the rule generalises over: a value no catalog of this
/// service holds, the case of a declared value, a prefix of one, a suffix on one, a
/// declared value sent twice, a value declared by a different enum of this service
/// (`DayOfWeek`'s `MONDAY`, which none of these catalogs holds), and a value far past
/// the length a refusal echoes.
fn off_catalog(allowed: &[&str]) -> Vec<(&'static str, String)> {
    let first = allowed[0];
    vec![
        ("a value no catalog of this service holds", "NOT_MODELLED".to_string()),
        ("a declared value in lower case", first.to_lowercase()),
        ("a prefix of a declared value", first[..1].to_string()),
        ("a declared value with a suffix", format!("{first}_AND_MORE")),
        ("a declared value sent twice", format!("{first},{first}")),
        ("a value declared by another enum of this service", "MONDAY".to_string()),
        ("a value longer than a refusal echoes", "x".repeat(300)),
    ]
}

/// Every scalar enum member is refused the same way for a value its catalog does not
/// hold: HTTP 400 `ValidationException` with the one sentence, whether or not the
/// resource the request names exists — the value is weighed before anything is resolved
/// — and the refusal writes nothing.
#[tokio::test]
async fn a_value_outside_its_catalog_is_refused_with_one_sentence() {
    let router = router();
    let instance = "enum-catalog-one-rule";
    // Onboards `instance` on the way, so both a campaign and an instance holding a
    // configuration are in the store for the "a resource the store holds" rows.
    let campaign = create_campaign(&router, REGION, instance, "enum-catalog-one-rule", None).await;

    for member in ENUM_MEMBERS {
        let held = match member.resource {
            Resource::Campaign => campaign.as_str(),
            Resource::ConnectInstance => instance,
        };
        // Within `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and `InstanceId`'s
        // `^[-_.a-zA-Z0-9]+$`, so the id is well formed for either path and the only
        // thing wrong with the request is the enum value.
        let targets = [
            ("a resource the store holds", held),
            ("a resource nothing answers to", "no-such-resource"),
        ];

        for (how, value) in off_catalog(member.allowed) {
            for (named, target) in targets {
                let case = format!("{} with {how} on {named}", member.operation);
                let (status, body) = drive(&router, target, member, Some(&value)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body}"
                );
                assert!(
                    error_code(&body).ends_with("ValidationException"),
                    "case {case:?}: expected ValidationException, got {}",
                    error_code(&body)
                );
                assert_eq!(
                    message_of(&body),
                    expected_message(member.member, member.declared, &value),
                    "case {case:?}: the refusal was worded {:?}",
                    message_of(&body)
                );
            }
        }
    }

    // Not one of those refusals wrote anything: the campaign is still there to read and
    // the instance still holds the configuration its onboarding wrote.
    let (status, body) = call(
        &router,
        REGION,
        "GET",
        &format!("/v2/campaigns/{campaign}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the campaign after every refusal\": expected HTTP 200, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/config"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the instance configuration after every refusal\": expected HTTP 200, \
         got {status} {body}"
    );
}

/// A router holding exactly what `member`'s operation needs to be SERVED, and the
/// target id to drive it against: a campaign for the campaign-configuration deletions,
/// an instance whose onboarding job has finished for the configuration deletion, and a
/// Connect instance the account holds for the onboarding start — which is what there is
/// to onboard.
async fn served_fixture(member: &EnumMember) -> (Harness, String) {
    let router = router();
    let instance = "enum-catalog-served";
    let target = match member.operation {
        "StartInstanceOnboardingJob" => {
            router.register_connect_instance(REGION, instance);
            instance.to_string()
        }
        _ if member.resource == Resource::ConnectInstance => {
            onboard_instance(&router, REGION, instance).await;
            instance.to_string()
        }
        _ => create_campaign(&router, REGION, instance, "enum-catalog-served", None).await,
    };
    (router, target)
}

/// Every value a catalog DOES hold is served, so the shared check refuses only what the
/// catalog leaves out — asserted on every value of every one of the five catalogs,
/// including the ones no refusal case above cited. Where the model leaves the member
/// optional, a request that sends no value at all is served too.
///
/// Each row runs against its own fixture, so a served call's own effect (a block
/// removed, a configuration deleted, a job started) cannot decide the next row. What
/// each of those effects is belongs to that operation's own suite.
#[tokio::test]
async fn every_value_a_catalog_holds_is_served() {
    for member in ENUM_MEMBERS {
        for value in member.allowed {
            let case = format!("{} with the declared value {value:?}", member.operation);
            let (router, target) = served_fixture(member).await;
            let (status, body) = drive(&router, &target, member, Some(value)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
        }

        // The two optional members: a request that carries no value has nothing to
        // weigh, so leaving the member out is served as well. The required members
        // (`channelSubtype`, and `config` on either deletion) address no route at all
        // without their query parameter, so there is no such request to drive.
        if member.member == "campaignDeletionPolicy" || member.member.ends_with("encryptionType") {
            let case = format!("{} with the member left out", member.operation);
            let (router, target) = served_fixture(member).await;
            let (status, body) = drive(&router, &target, member, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
        }
    }
}

/// The ONE refusal the five handlers now compose, asserted directly on the shared check
/// each of them reaches it through.
///
/// Over the wire the boundary walk answers first — every one of these members targets a
/// modelled enum — so the handlers' own check is the backstop that keeps each
/// operation's contract if that walk ever stops covering a member. This case is what
/// holds that backstop to the rule for all five members at once: a value its catalog
/// holds is nothing to refuse, a member the request left out is nothing to weigh, and
/// anything else is the one validation-envelope sentence naming the member and its
/// value set, with an over-long value echoed as a bounded prefix.
#[test]
fn the_shared_check_refuses_exactly_what_a_catalog_leaves_out() {
    for member in ENUM_MEMBERS {
        let absent = enum_value_violation(member.member, member.allowed, None);
        assert!(
            absent.is_none(),
            "case {:?}: a member the request left out is nothing to weigh, got {absent:?}",
            format!("{} with no value", member.operation)
        );

        for value in member.allowed {
            let served = enum_value_violation(member.member, member.allowed, Some(value));
            assert!(
                served.is_none(),
                "case {:?}: expected the declared value to pass, got {served:?}",
                format!("{} with the declared value {value:?}", member.operation)
            );
        }

        for (how, value) in off_catalog(member.allowed) {
            let case = format!("{} with {how}", member.operation);
            let refusal = enum_value_violation(member.member, member.allowed, Some(&value));
            assert_eq!(
                refusal.as_deref(),
                Some(expected_message(member.member, member.allowed, &value).as_str()),
                "case {case:?}: the refusal was worded {refusal:?}"
            );
        }
    }
}
