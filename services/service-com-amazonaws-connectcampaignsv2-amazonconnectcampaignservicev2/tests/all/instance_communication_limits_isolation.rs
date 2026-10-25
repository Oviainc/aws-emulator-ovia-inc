// SPDX-License-Identifier: CC0-1.0

//! `PutInstanceCommunicationLimits` against everything else this service holds for a
//! Connect instance, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together.
//!
//! The rule under test: the instance-wide communication limits are one member of what
//! this service holds for a Connect instance, so writing them replaces that member
//! alone and leaves every other resource and member of the instance — its onboarding
//! job status, its `InstanceConfig`, its integrations and each of its campaigns' own
//! `communicationLimitsOverride` — exactly as they were; and, the other way round, a
//! write of a campaign's own override leaves the instance-wide limits alone. The two
//! configs are separate shapes on separate operations
//! (`smithy_query(query="operation", operation="PutInstanceCommunicationLimits,UpdateCampaignCommunicationLimits")`:
//! `InstanceCommunicationLimitsConfig` on the instance, `CommunicationLimitsConfig` on
//! the campaign), and neither operation is modelled as touching the other's resource.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES, INSTANCE_LIMITS_HANDLING_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, router};

/// One `CommunicationLimit` entry.
fn limit(max_count_per_recipient: i64, frequency: i64, unit: &str) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        "unit": unit,
    })
}

/// The one `CommunicationLimitTimeUnit` the model's catalog declares.
fn day() -> &'static str {
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0]
}

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

/// Onboard `instance` and wait out the span the clock owes its onboarding job, so the
/// instance is configurable and a campaign target by the time this returns. Nothing in
/// the emulator sleeps for that span — only a caller that wants the settled state does.
async fn onboard(router: &MultiServiceRouter, region: &str, instance: &str) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
}

/// `PutInstanceCommunicationLimits` for `instance`, asserting it was served (an
/// implemented operation answers its modelled 200, never the scaffold's 501).
async fn put_instance_limits(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    config: &Value,
    case: &str,
) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &json!({"communicationLimitsConfig": config}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutInstanceCommunicationLimits expected 200, got {status} {body}"
    );
    assert!(
        body.get("__type").is_none(),
        "case {case:?}: expected a served response carrying no error, got {body}"
    );
}

/// The `communicationLimitsConfig` `GetInstanceCommunicationLimits` projects for
/// `instance`, asserting the read succeeded. `Value::Null` means the instance holds
/// none.
async fn instance_limits(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetInstanceCommunicationLimits expected 200, got {status} {body}"
    );
    body.get("communicationLimitsConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Everything else this service reports about `instance`: its onboarding job status,
/// its `InstanceConfig` and its integrations, as one value to compare before and
/// after.
async fn instance_surroundings(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    case: &str,
) -> Value {
    let mut reported = Vec::new();
    for (what, path) in [
        ("onboarding job status", "onboarding"),
        ("instance config", "config"),
        ("integrations", "integrations"),
    ] {
        let (status, body) = call(
            router,
            region,
            "GET",
            &format!("/v2/connect-instance/{instance}/{path}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the {what} expected 200, got {status} {body}"
        );
        reported.push(json!({what: body}));
    }
    Value::Array(reported)
}

/// Create a campaign on an onboarded `instance`, optionally already carrying
/// `limits` as its `communicationLimitsOverride`, and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    limits: Option<&Value>,
) -> String {
    let mut body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_config(),
    });
    if let Some(limits) = limits {
        body["communicationLimitsOverride"] = limits.clone();
    }
    let (status, response) = call(router, region, "PUT", "/v2/campaigns", &body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCampaign expected 200, got {status} {response}"
    );
    response["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCampaign returned no id: {response}"))
        .to_string()
}

/// The `campaign` object `DescribeCampaign` projects for `id`, asserting the read
/// succeeded.
async fn describe(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCampaign expected 200, got {status} {body}"
    );
    body["campaign"].clone()
}

/// A write of the instance-wide limits changes those limits and nothing else about the
/// instance, over a TABLE of configs.
///
/// The rows differ along what the rule generalises over: how many entries the
/// `communicationLimitsList` carries (its `@length` maximum is 2, and it has no
/// minimum), the `unit` (source: `COMMUNICATION_LIMIT_TIME_UNIT_VALUES`, the model's
/// `CommunicationLimitTimeUnit` catalog), both published bounds of `frequency`'s
/// `@range` 1..30 with `maxCountPerRecipient`'s minimum of 1, a config naming no member
/// at all, and a config repeated so the idempotent write is covered too.
#[tokio::test]
async fn an_instance_limits_write_replaces_only_the_instance_limits() {
    let region = "us-east-1";
    let router = router();
    let instance = "isolation-instance";
    onboard(&router, region, instance).await;

    // An integration on the instance, so the comparison below spans more than the
    // onboarding record.
    let (status, body) = call(
        &router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/integrations"),
        &json!({"integrationConfig": {"lambda": {
            "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:isolation"
        }}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the instance's integration\": expected 200, got {status} {body}"
    );

    // Two campaigns on the instance: one carrying its own override, one carrying none.
    let overridden_at_create = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(4, 4, day())]},
        "instanceLimitsHandling": INSTANCE_LIMITS_HANDLING_VALUES[0]
    });
    let with_override = create_campaign(
        &router,
        region,
        instance,
        "isolation-with-override",
        Some(&overridden_at_create),
    )
    .await;
    let without_override =
        create_campaign(&router, region, instance, "isolation-no-override", None).await;

    let surroundings_before = instance_surroundings(&router, region, instance, "before").await;
    let with_before = describe(&router, region, &with_override, "before").await;
    let without_before = describe(&router, region, &without_override, "before").await;

    let unit = day();
    let cases: Vec<(&str, Value)> = vec![
        (
            "one limit",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(5, 7, unit)]}}),
        ),
        (
            "two entries, the @length maximum of communicationLimitsList",
            json!({
                "allChannelSubtypes": {
                    "communicationLimitsList": [limit(2, 1, unit), limit(10, 30, unit)]
                }
            }),
        ),
        (
            "the published minimum of frequency's @range 1..30 and the minimum count",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(1, 1, unit)]}}),
        ),
        (
            "the published maximum of frequency's @range 1..30",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(1, 30, unit)]}}),
        ),
        (
            "an empty list: communicationLimitsList has no @length minimum",
            json!({"allChannelSubtypes": {"communicationLimitsList": []}}),
        ),
        (
            "an empty config: the shape has no required member",
            json!({}),
        ),
        (
            "the same config again: the write is idempotent",
            json!({}),
        ),
    ];

    for (case, config) in &cases {
        put_instance_limits(&router, region, instance, config, case).await;

        let stored = instance_limits(&router, region, instance, case).await;
        assert_eq!(
            &stored, config,
            "case {case:?}: expected the instance to hold {config}, got {stored}"
        );

        let surroundings_after = instance_surroundings(&router, region, instance, case).await;
        assert_eq!(
            surroundings_after, surroundings_before,
            "case {case:?}: expected the instance's other configuration to be untouched \
             ({surroundings_before}), got {surroundings_after}"
        );

        let with_after = describe(&router, region, &with_override, case).await;
        assert_eq!(
            with_after, with_before,
            "case {case:?}: expected the campaign carrying its own override to be untouched \
             ({with_before}), got {with_after}"
        );
        let without_after = describe(&router, region, &without_override, case).await;
        assert_eq!(
            without_after, without_before,
            "case {case:?}: expected the campaign carrying no override to be untouched \
             ({without_before}), got {without_after}"
        );
        assert!(
            without_after.get("communicationLimitsOverride").is_none(),
            "case {case:?}: expected the instance-wide write to give no campaign an override, \
             got {without_after}"
        );
    }
}

/// A write of a campaign's own override leaves the instance-wide limits alone, over a
/// TABLE of overrides, and a later instance-wide write leaves the campaign's override
/// alone.
///
/// The rows differ along what the rule generalises over: how many entries the
/// override's `communicationLimitsList` carries, which `instanceLimitsHandling` value
/// it names (source: `INSTANCE_LIMITS_HANDLING_VALUES`, the model's
/// `InstanceLimitsHandling` catalog) and whether it names one at all, and an override
/// naming no member.
#[tokio::test]
async fn a_campaign_override_write_leaves_the_instance_limits_alone() {
    let region = "eu-central-1";
    let router = router();
    let instance = "campaign-override-instance";
    onboard(&router, region, instance).await;

    let unit = day();
    let held = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(6, 6, unit)]}
    });
    put_instance_limits(&router, region, instance, &held, "the instance's own limits").await;

    let id = create_campaign(&router, region, instance, "override-campaign", None).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "one limit and no handling value",
            json!({"allChannelSubtypes": {"communicationLimitsList": [limit(3, 2, unit)]}}),
        ),
        (
            "the first instanceLimitsHandling value alone",
            json!({"instanceLimitsHandling": INSTANCE_LIMITS_HANDLING_VALUES[0]}),
        ),
        (
            "the second instanceLimitsHandling value with two entries",
            json!({
                "allChannelSubtypes": {
                    "communicationLimitsList": [limit(1, 1, unit), limit(2, 30, unit)]
                },
                "instanceLimitsHandling": INSTANCE_LIMITS_HANDLING_VALUES[1]
            }),
        ),
        (
            "an override naming no member",
            json!({}),
        ),
    ];

    for (case, override_config) in &cases {
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/communication-limits"),
            &json!({"communicationLimitsOverride": override_config}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignCommunicationLimits expected 200, got {status} {body}"
        );

        let campaign = describe(&router, region, &id, case).await;
        let projected = campaign
            .get("communicationLimitsOverride")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            &projected, override_config,
            "case {case:?}: expected the campaign to hold {override_config}, got {projected}"
        );

        let stored = instance_limits(&router, region, instance, case).await;
        assert_eq!(
            stored, held,
            "case {case:?}: expected the instance-wide limits to stay {held}, got {stored}"
        );
    }

    // And the instance-wide write still only writes its own member: the campaign keeps
    // the override the last update gave it.
    let last = cases
        .last()
        .map(|(_case, config)| config.clone())
        .expect("the table has at least one row");
    let replacement = json!({
        "allChannelSubtypes": {"communicationLimitsList": [limit(9, 14, unit)]}
    });
    put_instance_limits(
        &router,
        region,
        instance,
        &replacement,
        "a later instance-wide write",
    )
    .await;

    let stored = instance_limits(&router, region, instance, "a later instance-wide write").await;
    assert_eq!(
        stored, replacement,
        "case \"a later instance-wide write\": expected {replacement} stored, got {stored}"
    );
    let campaign = describe(&router, region, &id, "a later instance-wide write").await;
    let projected = campaign
        .get("communicationLimitsOverride")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        projected, last,
        "case \"a later instance-wide write\": expected the campaign to keep {last}, \
         got {projected}"
    );
}
