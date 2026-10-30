// SPDX-License-Identifier: CC0-1.0

//! A configuration deletion that removed nothing writes nothing, end to end over the
//! wire.
//!
//! One rule, over every campaign deletion that takes ONE optional block out of an
//! optional container the record holds: the container is dropped only when the call
//! actually removed the last block it held. A container that held no block for the
//! config the request named — an EMPTY container included — is left exactly as it was,
//! which is what makes these operations idempotent, as the model declares.
//!
//! Both halves are asserted through `DescribeCampaign`, the projection of the stored
//! record, and over both paths a container reaches the record by (`CreateCampaign` and
//! the operation's own update), because the rule belongs to the record rather than to
//! the call that wrote it. The sibling `DeleteCampaignChannelSubtypeConfig` is driven
//! by the same table shape, so the rule is pinned where it holds and not only on the
//! one operation that broke it.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    CHANNEL_SUBTYPE_VALUES, COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES,
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES, INSTANCE_LIMITS_HANDLING_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, create_campaign_with, describe_campaign, router};

/// One `CommunicationLimit` entry: every member of the shape is `@required`.
fn limit(max_count_per_recipient: i64, frequency: i64, unit: &str) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        "unit": unit,
    })
}

/// The config block of one channel, in the minimal form its own shape requires:
/// `outboundMode` and `defaultOutboundConfig` on every channel block,
/// `connectContactFlowId` on `TelephonyOutboundConfig`, and
/// `connectSourcePhoneNumberArn` with `wisdomTemplateArn` on `SmsOutboundConfig`.
fn channel_block(channel_subtype: &str) -> Value {
    match channel_subtype {
        "TELEPHONY" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }),
        "SMS" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
            }
        }),
        other => panic!("{other} has no block in this suite"),
    }
}

/// `DeleteCampaignCommunicationLimits` for `id` and the `config` config type.
async fn delete_limits(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{id}/communication-limits?config={config}"),
        &Value::Null,
    )
    .await
}

/// `DeleteCampaignChannelSubtypeConfig` for `id` and the `channel_subtype` channel.
async fn delete_subtype(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    channel_subtype: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{id}/channel-subtype-config?channelSubtype={channel_subtype}"),
        &Value::Null,
    )
    .await
}

/// Assert the campaign projects `expected` for `member` — `Value::Null` meaning the
/// member is absent from the projection altogether, which is what a record holding no
/// container at all projects.
fn assert_member(campaign: &Value, member: &str, expected: &Value, case: &str) {
    let actual = campaign.get(member);
    if expected.is_null() {
        assert!(
            actual.is_none(),
            "case {case:?}: expected no {member}, got {actual:?}"
        );
    } else {
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: expected {expected}, got {actual:?}"
        );
    }
}

/// Assert `before` and `after` agree on every member except `member`.
fn assert_everything_else_untouched(before: &Value, after: &Value, member: &str, case: &str) {
    let mut before_rest = before.clone();
    let mut after_rest = after.clone();
    for projection in [&mut before_rest, &mut after_rest] {
        projection
            .as_object_mut()
            .expect("a campaign projection is an object")
            .remove(member);
    }
    assert_eq!(
        before_rest, after_rest,
        "case {case:?}: expected every other member untouched, got {after_rest} \
         (was {before_rest})"
    );
}

/// `DeleteCampaignCommunicationLimits` collapses the campaign's
/// `communicationLimitsOverride` only when it REMOVED the last member that override
/// held; an override holding no block for the config the request named survives the
/// call untouched.
///
/// The rows span what the rule generalises over: how many members the stored override
/// holds (none, one, both), which member that is, which `InstanceLimitsHandling` value
/// it carries (source: `INSTANCE_LIMITS_HANDLING_VALUES`, both entries driven), how many
/// `communicationLimitsList` entries the block carries (one, and the `@length` maximum
/// of 2 at both bounds of `frequency`'s `@range` 1..30) — and, for each row, both paths
/// the override reaches the record by, against every value of the model's
/// `CommunicationLimitsConfigType` catalog
/// (`COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES`).
#[tokio::test]
async fn a_limits_deletion_collapses_the_override_only_when_it_removed_a_block() {
    let region = "us-west-2";
    let router = router();
    let instance = "deletion-writes-only-a-removal-instance";

    let day = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let opt_in = INSTANCE_LIMITS_HANDLING_VALUES[0];
    // The catalog entry the issue did not cite.
    let opt_out = INSTANCE_LIMITS_HANDLING_VALUES[1];

    // (case, the override stored before the deletion, what the record must hold after
    // the named config type is deleted — `Value::Null` for an absent member)
    let rows: Vec<(&str, Value, Value)> = vec![
        // Nothing to remove: the record is left exactly as it was.
        ("an override with no member at all", json!({}), json!({})),
        (
            "instanceLimitsHandling alone",
            json!({"instanceLimitsHandling": opt_in}),
            json!({"instanceLimitsHandling": opt_in}),
        ),
        (
            "instanceLimitsHandling alone, the other catalog value",
            json!({"instanceLimitsHandling": opt_out}),
            json!({"instanceLimitsHandling": opt_out}),
        ),
        // A removal that empties the override collapses it.
        (
            "the named block alone, one entry",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(5, 7, day)]}}),
            Value::Null,
        ),
        (
            "the named block alone, two entries at both bounds of frequency's 1..30",
            json!({"allChannelSubtypes": {
                "communicationLimitsList": [limit(1, 1, day), limit(10, 30, day)]
            }}),
            Value::Null,
        ),
        // A removal that leaves a member behind keeps the override.
        (
            "the named block beside instanceLimitsHandling",
            json!({
                "allChannelSubtypes": {"communicationLimitsList": [limit(3, 2, day)]},
                "instanceLimitsHandling": opt_out
            }),
            json!({"instanceLimitsHandling": opt_out}),
        ),
    ];

    for (row, (shape, stored, expected)) in rows.iter().enumerate() {
        for target in COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES {
            // Both paths an override reaches the record by: the create that takes it
            // verbatim, and the update that replaces whatever the record held.
            for through_update in [false, true] {
                let case = format!(
                    "{shape}, delete {target}, stored {}",
                    if through_update {
                        "through UpdateCampaignCommunicationLimits"
                    } else {
                        "at CreateCampaign"
                    }
                );
                let name = format!("limits-{row}-{target}-{through_update}");
                let extra = if through_update {
                    json!({})
                } else {
                    json!({"communicationLimitsOverride": stored})
                };
                let id = create_campaign_with(&router, region, instance, &name, extra).await;
                if through_update {
                    let (status, body) = call(
                        &router,
                        region,
                        "POST",
                        &format!("/v2/campaigns/{id}/communication-limits"),
                        &json!({"communicationLimitsOverride": stored}),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: UpdateCampaignCommunicationLimits expected 200, \
                         got {status} {body}"
                    );
                }

                // The override really is on the record before the deletion, empty or
                // not — the round trip the rule is about.
                let before = describe_campaign(&router, region, &id, &case).await;
                assert_member(&before, "communicationLimitsOverride", stored, &case);

                // The same deletion twice: idempotent, and the second call sees the
                // record the first one left.
                for attempt in ["the deletion", "a repeated deletion"] {
                    let (status, body) = delete_limits(&router, region, &id, target).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: {attempt} expected 200, got {status} {body}"
                    );
                    let after = describe_campaign(&router, region, &id, &case).await;
                    assert_member(&after, "communicationLimitsOverride", expected, &case);
                    assert_everything_else_untouched(
                        &before,
                        &after,
                        "communicationLimitsOverride",
                        &case,
                    );
                }
            }
        }
    }
}

/// The same rule on the sibling deletion: `DeleteCampaignChannelSubtypeConfig` drops the
/// campaign's `channelSubtypeConfig` only when it REMOVED the last channel block that
/// config held.
///
/// The rows differ along which channel the config holds, which channel the request
/// names and whether the config holds that channel at all; the channels come from the
/// model's `ChannelSubtype` catalog (`CHANNEL_SUBTYPE_VALUES`).
#[tokio::test]
async fn a_channel_subtype_deletion_drops_the_config_only_when_it_removed_a_block() {
    let region = "eu-central-1";
    let router = router();
    let instance = "subtype-deletion-writes-only-a-removal-instance";

    let telephony = CHANNEL_SUBTYPE_VALUES
        .iter()
        .find(|subtype| **subtype == "TELEPHONY")
        .expect("TELEPHONY is a ChannelSubtype value");
    let sms = CHANNEL_SUBTYPE_VALUES
        .iter()
        .find(|subtype| **subtype == "SMS")
        .expect("SMS is a ChannelSubtype value");

    // (case, the config stored at create, the channel the request names, what the record
    // must hold afterwards)
    let rows: Vec<(&str, Value, &str, Value)> = vec![
        (
            "a config with no channel block at all",
            json!({}),
            telephony,
            json!({}),
        ),
        (
            "a config holding another channel's block",
            json!({"telephony": channel_block("TELEPHONY")}),
            sms,
            json!({"telephony": channel_block("TELEPHONY")}),
        ),
        (
            "a config holding only the named channel's block",
            json!({"telephony": channel_block("TELEPHONY")}),
            telephony,
            Value::Null,
        ),
        (
            "a config holding the named channel beside another",
            json!({"telephony": channel_block("TELEPHONY"), "sms": channel_block("SMS")}),
            sms,
            json!({"telephony": channel_block("TELEPHONY")}),
        ),
    ];

    for (row, (shape, stored, target, expected)) in rows.iter().enumerate() {
        let case = format!("{shape}, delete {target}");
        let name = format!("subtype-{row}-{target}");
        let id = create_campaign_with(
            &router,
            region,
            instance,
            &name,
            json!({"channelSubtypeConfig": stored}),
        )
        .await;

        let before = describe_campaign(&router, region, &id, &case).await;
        assert_member(&before, "channelSubtypeConfig", stored, &case);

        for attempt in ["the deletion", "a repeated deletion"] {
            let (status, body) = delete_subtype(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {attempt} expected 200, got {status} {body}"
            );
            let after = describe_campaign(&router, region, &id, &case).await;
            assert_member(&after, "channelSubtypeConfig", expected, &case);
            assert_everything_else_untouched(&before, &after, "channelSubtypeConfig", &case);
        }
    }
}
