// SPDX-License-Identifier: CC0-1.0

//! `DeleteCampaignChannelSubtypeConfig`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call removes the channel-subtype config block the campaign `{id}` names
//!   holds for the ONE channel `channelSubtype` names, and nothing else: every other
//!   channel's block stays as it was, and so does every other member of the campaign
//!   — which is what every later read (`DescribeCampaign`, and `ListCampaigns`'
//!   derived `channelSubtypes`) projects;
//! * a config left holding no channel at all is no config: the campaign carries no
//!   `channelSubtypeConfig` once its last block goes;
//! * the operation is idempotent (`@idempotent` in the model, "This API is
//!   idempotent" in its documentation): a channel that holds no block, and a campaign
//!   carrying no channel subtype config at all, are a 200 that removes nothing;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` and
//!   removes nothing;
//! * a `channelSubtype` outside the model's `ChannelSubtype` catalog is HTTP 400
//!   `ValidationException` and removes nothing;
//! * the deletion turns on the campaign existing, not on its lifecycle state: this
//!   operation models no `InvalidCampaignStateException`, so every state the campaign
//!   lifecycle reaches takes it.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::CHANNEL_SUBTYPE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, CAMPAIGN_NOT_FOUND_MESSAGE, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// The `ChannelSubtypeConfig` member that carries the block of one `ChannelSubtype`
/// value.
///
/// Source: the `ChannelSubtypeConfig` shape, whose per-channel members are
/// `telephony`, `sms`, `email` and `whatsApp`.
fn member_of(channel_subtype: &str) -> &'static str {
    match channel_subtype {
        "EMAIL" => "email",
        "SMS" => "sms",
        "TELEPHONY" => "telephony",
        "WHATSAPP" => "whatsApp",
        other => panic!("{other} is not a ChannelSubtype value"),
    }
}

/// The config block of one channel, in a form that varies with `shape`: which
/// outbound mode a telephony block runs, whether it carries the optional `capacity`
/// (`Capacity`, range 0.01..1) and which optional members of its
/// `defaultOutboundConfig` it fills.
///
/// The required members come from each channel's own shape:
/// `outboundMode` and `defaultOutboundConfig` on every channel block;
/// `connectContactFlowId` on `TelephonyOutboundConfig`; `connectSourcePhoneNumberArn`
/// and `wisdomTemplateArn` on `SmsOutboundConfig` and `WhatsAppOutboundConfig`;
/// `connectSourceEmailAddress` and `wisdomTemplateArn` on `EmailOutboundConfig`.
fn block(channel_subtype: &str, shape: usize) -> Value {
    let mut block = match channel_subtype {
        "TELEPHONY" => {
            // `TelephonyOutboundMode` is a union: one of `agentless`, `progressive`,
            // `predictive` or `preview`. Each row drives a different member.
            let outbound_mode = match shape % 4 {
                0 => json!({"agentless": {}}),
                1 => json!({"progressive": {"bandwidthAllocation": 0.5}}),
                2 => json!({"predictive": {"bandwidthAllocation": 0.25}}),
                _ => json!({
                    "preview": {
                        "bandwidthAllocation": 0.75,
                        "timeoutConfig": {"durationInSeconds": 30}
                    }
                }),
            };
            let mut telephony = json!({
                "outboundMode": outbound_mode,
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            });
            if shape % 2 == 1 {
                telephony["connectQueueId"] = json!("queue-0001");
                telephony["defaultOutboundConfig"]["ringTimeout"] = json!(45);
                telephony["defaultOutboundConfig"]["answerMachineDetectionConfig"] =
                    json!({"enableAnswerMachineDetection": true});
            }
            telephony
        }
        "SMS" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
            }
        }),
        "EMAIL" => {
            let mut email = json!({
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {
                    "connectSourceEmailAddress": "campaigns@example.com",
                    "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-2"
                }
            });
            if shape.is_multiple_of(3) {
                email["defaultOutboundConfig"]["sourceEmailAddressDisplayName"] =
                    json!("Campaigns");
            }
            email
        }
        "WHATSAPP" => json!({
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-2",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-3"
            }
        }),
        other => panic!("{other} is not a ChannelSubtype value"),
    };
    // `capacity` is optional on every channel block, inside `Capacity`'s 0.01..1
    // range; the rows differ in whether it is set and at what value.
    if shape % 3 == 1 {
        block["capacity"] = json!(0.01);
    } else if shape % 3 == 2 {
        block["capacity"] = json!(0.99);
    }
    block
}

/// A `channelSubtypeConfig` configuring each channel in `channels` (each a
/// `ChannelSubtype` value), every block in a form of its own.
fn subtype_config(channels: &[&str]) -> Value {
    let mut config = json!({});
    for (shape, channel) in channels.iter().enumerate() {
        config[member_of(channel)] = block(channel, shape);
    }
    config
}

/// Onboard `instance` and wait out the span the clock owes its onboarding job, so the
/// instance is a campaign target by the time this returns: `CreateCampaign` mints a
/// campaign only for an instance whose job has SUCCEEDED. Nothing in the emulator
/// sleeps for that span — only a caller that wants the settled state does.
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

/// Create a campaign on an onboarded `instance`, optionally carrying
/// `channel_subtype_config`, and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    channel_subtype_config: Option<&Value>,
) -> String {
    let mut body = json!({"name": name, "connectInstanceId": instance});
    if let Some(config) = channel_subtype_config {
        body["channelSubtypeConfig"] = config.clone();
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

/// What the campaign SHOULD now project for `channelSubtypeConfig`: the created
/// config minus the deleted channel's block, and nothing at all once no block is
/// left.
fn expected_config(created: &Value, deleted: &[&str]) -> Value {
    let mut expected = created.clone();
    let members = expected
        .as_object_mut()
        .expect("a channel subtype config is an object");
    let mut removed_any = false;
    for channel in deleted {
        removed_any |= members.remove(member_of(channel)).is_some();
    }
    if removed_any && members.is_empty() {
        Value::Null
    } else {
        expected
    }
}

/// Assert the campaign projects `expected` for `channelSubtypeConfig` — `Value::Null`
/// meaning the member is absent altogether.
fn assert_config(campaign: &Value, expected: &Value, case: &str) {
    if expected.is_null() {
        assert!(
            campaign.get("channelSubtypeConfig").is_none(),
            "case {case:?}: expected no channelSubtypeConfig, got {:?}",
            campaign.get("channelSubtypeConfig")
        );
    } else {
        assert_eq!(
            &campaign["channelSubtypeConfig"], expected,
            "case {case:?}: expected {expected}, got {}",
            campaign["channelSubtypeConfig"]
        );
    }
}

/// Deleting one channel's block removes exactly that block, over a TABLE spanning the
/// whole `ChannelSubtype` catalog (`CHANNEL_SUBTYPE_VALUES`, taken from the model)
/// against configs that differ in which channels they configure and in the form of
/// each block.
#[tokio::test]
async fn deleting_one_channel_subtype_leaves_every_other_part_of_the_campaign() {
    let region = "us-west-2";
    let router = router();
    let instance = "channel-subtype-instance";
    onboard(&router, region, instance).await;

    // (case, the channels the created config configures)
    let configurations: Vec<(&str, Vec<&str>)> = vec![
        ("every channel configured", CHANNEL_SUBTYPE_VALUES.to_vec()),
        ("two channels configured", vec!["TELEPHONY", "WHATSAPP"]),
        ("one channel configured", vec!["SMS"]),
        ("one channel configured, another form", vec!["TELEPHONY"]),
        ("a config with no channel in it", Vec::new()),
    ];

    for (shape, (configuration, channels)) in configurations.iter().enumerate() {
        // Every catalog value is deleted against every configuration, including the
        // ones that never configured it.
        for target in CHANNEL_SUBTYPE_VALUES {
            let case = format!("{configuration}, delete {target}");
            let created = subtype_config(channels);
            let id = create_campaign(
                &router,
                region,
                instance,
                &format!("campaign-{shape}-{target}"),
                Some(&created),
            )
            .await;
            let before = describe(&router, region, &id, &case).await;
            assert_eq!(
                before["channelSubtypeConfig"], created,
                "case {case:?}: expected the created config stored, got {}",
                before["channelSubtypeConfig"]
            );

            let (status, body) = delete_subtype(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteCampaignChannelSubtypeConfig expected 200, \
                 got {status} {body}"
            );

            let expected = expected_config(&created, &[target]);
            let after = describe(&router, region, &id, &case).await;
            assert_config(&after, &expected, &case);

            // Idempotent: the same deletion again is a 200 and the same config.
            let (status, body) = delete_subtype(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated deletion expected 200, got {status} {body}"
            );
            let repeated = describe(&router, region, &id, &case).await;
            assert_config(&repeated, &expected, &case);

            // The call changes the channel subtype config and NOTHING else about the
            // campaign.
            let mut before_rest = before.clone();
            let mut after_rest = repeated.clone();
            for projection in [&mut before_rest, &mut after_rest] {
                projection
                    .as_object_mut()
                    .expect("a campaign projection is an object")
                    .remove("channelSubtypeConfig");
            }
            assert_eq!(
                before_rest, after_rest,
                "case {case:?}: expected every other member untouched, got {after_rest} \
                 (was {before_rest})"
            );
        }
    }
}

/// `ListCampaigns`' `channelSubtypes` is derived from the stored config, so a
/// deletion is visible there too: the catalog minus the channels deleted so far, in
/// catalog order. Deleting every channel in turn ends with a campaign that carries no
/// channel subtype config and lists no channel subtype.
#[tokio::test]
async fn the_summary_lists_only_the_channel_subtypes_still_configured() {
    let region = "eu-west-1";
    // Each row deletes the catalog in a different order, so the surviving set is not
    // an artefact of one sequence.
    let orders: &[&[&str]] = &[
        &["EMAIL", "SMS", "TELEPHONY", "WHATSAPP"],
        &["WHATSAPP", "TELEPHONY", "SMS", "EMAIL"],
        &["SMS", "WHATSAPP", "EMAIL", "TELEPHONY"],
    ];

    for order in orders {
        // One campaign per row, in its own store, so the single summary returned is
        // the one under test.
        let router = router();
        let instance = "channel-subtype-summary-instance";
        onboard(&router, region, instance).await;
        let created = subtype_config(CHANNEL_SUBTYPE_VALUES);
        let id = create_campaign(&router, region, instance, "summarised", Some(&created)).await;

        let mut deleted: Vec<&str> = Vec::new();
        for target in order.iter() {
            let case = format!("deleted {deleted:?} then {target}");
            let (status, body) = delete_subtype(&router, region, &id, target).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            deleted.push(target);

            let (status, listed) =
                call(&router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListCampaigns expected 200, got {status} {listed}"
            );
            let expected: Vec<&str> = CHANNEL_SUBTYPE_VALUES
                .iter()
                .copied()
                .filter(|channel| !deleted.contains(channel))
                .collect();
            let actual: Vec<&str> = listed["campaignSummaryList"][0]["channelSubtypes"]
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected channelSubtypes {expected:?}, got {actual:?}"
            );

            let campaign = describe(&router, region, &id, &case).await;
            assert_config(&campaign, &expected_config(&created, &deleted), &case);
        }
    }
}

/// A campaign created with no channel subtype config at all holds no block for any
/// channel, so every deletion is a 200 that stores nothing — the config stays absent
/// rather than being minted by the delete.
#[tokio::test]
async fn deleting_from_a_campaign_with_no_config_stores_nothing() {
    let region = "eu-central-1";
    let router = router();
    let instance = "channel-subtype-absent-instance";
    onboard(&router, region, instance).await;
    let id = create_campaign(&router, region, instance, "no-config", None).await;

    for target in CHANNEL_SUBTYPE_VALUES {
        let case = format!("no config at all, delete {target}");
        let (status, body) = delete_subtype(&router, region, &id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert!(
            campaign.get("channelSubtypeConfig").is_none(),
            "case {case:?}: expected no channelSubtypeConfig, got {:?}",
            campaign.get("channelSubtypeConfig")
        );
    }
}

/// An id the store does not hold is a 404 carrying this service's fixed not-found
/// text, and removes nothing — asserted over a table of well-formed `CampaignId`s the
/// store never held, plus one it held until `DeleteCampaign`.
#[tokio::test]
async fn an_unknown_campaign_is_not_found_and_loses_no_config() {
    let region = "ap-northeast-1";
    let router = router();
    let instance = "channel-subtype-404-instance";
    onboard(&router, region, instance).await;

    // A campaign that stays in the store, so the 404s below are the lookup failing
    // and not the operation being broken.
    let kept = subtype_config(CHANNEL_SUBTYPE_VALUES);
    let control = create_campaign(&router, region, instance, "control", Some(&kept)).await;

    // One campaign created and then deleted: its id was valid a moment ago.
    let deleted = create_campaign(&router, region, instance, "doomed", Some(&kept)).await;
    let (status, body) = call(
        &router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{deleted}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete the doomed campaign\": expected 200, got {status} {body}"
    );

    // (case, id) — ids spanning `CampaignId` (len 1..256, pattern ^[-:/a-zA-Z0-9]+$).
    let cases: Vec<(String, String)> = vec![
        ("never created".to_string(), "missing".to_string()),
        ("single character".to_string(), "0".to_string()),
        (
            // `/` is left out: a `@httpLabel` is one path segment, so an id
            // carrying one does not address this operation's URI at all.
            "every character class the pattern admits".to_string(),
            "A-b:C1".to_string(),
        ),
        ("the 256-character maximum".to_string(), "c".repeat(256)),
        ("created and then deleted".to_string(), deleted.clone()),
    ];

    for (case, id) in cases {
        let (status, body) = delete_subtype(&router, region, &id, "TELEPHONY").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body["message"].as_str(),
            Some(CAMPAIGN_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the fixed not-found text, got {:?}",
            body["message"]
        );

        // A refused call removes nothing: the campaign that does exist keeps every
        // block it was created with.
        let campaign = describe(&router, region, &control, &case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], kept,
            "case {case:?}: expected the control campaign's config untouched, got {}",
            campaign["channelSubtypeConfig"]
        );
    }
}

/// A `channelSubtype` outside the model's `ChannelSubtype` catalog names no channel,
/// so it is refused as a bad request and removes nothing.
///
/// The rows differ along what the rule generalises over: a plausible channel name the
/// catalog does not hold, a catalog value in the wrong case, and an empty value. A
/// request carrying no `channelSubtype` at all is left out: the generated routing
/// table resolves this route by its required query parameter, so such a request names
/// no operation and never reaches this handler.
///
/// The status and the error shape are what every row asserts, not a message: an
/// off-catalog enum value is refused by the boundary validator before the handler
/// runs, so which of the two layers composed the text is not part of the rule.
#[tokio::test]
async fn a_channel_subtype_outside_the_catalog_is_refused() {
    let region = "us-east-1";
    let router = router();
    let instance = "channel-subtype-validation-instance";
    onboard(&router, region, instance).await;

    let kept = subtype_config(CHANNEL_SUBTYPE_VALUES);
    let id = create_campaign(&router, region, instance, "validated", Some(&kept)).await;

    // (case, the value the `channelSubtype` query parameter carries)
    let cases: Vec<(&str, &str)> = vec![
        ("a value the catalog does not hold", "VOICE"),
        ("a catalog value in the wrong case", "email"),
        ("an empty value", ""),
    ];

    for (case, value) in cases {
        let (status, response) = delete_subtype(&router, region, &id, value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&response)
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], kept,
            "case {case:?}: expected the refusal to leave the config alone, got {}",
            campaign["channelSubtypeConfig"]
        );
    }
}

/// The deletion turns on the campaign existing, not on where its lifecycle is: this
/// operation models no `InvalidCampaignStateException`, so the table walks the
/// campaign through every state its lifecycle calls reach and deletes one channel in
/// each.
#[tokio::test]
async fn every_state_the_lifecycle_reaches_takes_the_deletion() {
    let region = "ap-south-1";
    let router = router();
    let instance = "channel-subtype-state-instance";
    onboard(&router, region, instance).await;
    let created = subtype_config(CHANNEL_SUBTYPE_VALUES);
    let id = create_campaign(&router, region, instance, "stateful", Some(&created)).await;

    // Each lifecycle action the emulator's state machine offers, the state it leaves
    // the campaign in, and the channel deleted there.
    let walk: &[(&str, CampaignState, &str)] = &[
        ("", CampaignState::Initialized, "EMAIL"),
        ("start", CampaignState::Running, "SMS"),
        ("pause", CampaignState::Paused, "TELEPHONY"),
        ("stop", CampaignState::Stopped, "WHATSAPP"),
    ];
    let mut deleted: Vec<&str> = Vec::new();
    for (action, state, target) in walk {
        if !action.is_empty() {
            let (status, body) = call(
                &router,
                region,
                "POST",
                &format!("/v2/campaigns/{id}/{action}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {action:?}: expected the lifecycle action to succeed, got {status} {body}"
            );
        }
        let case = format!("in state {}", state.as_wire());
        let (status, read) = call(
            &router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}/state"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCampaignState expected 200, got {status} {read}"
        );
        assert_eq!(
            read["state"].as_str(),
            Some(state.as_wire()),
            "case {case:?}: expected state {:?}, got {:?}",
            state.as_wire(),
            read["state"]
        );

        let (status, body) = delete_subtype(&router, region, &id, target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deletion to be accepted, got {status} {body}"
        );
        deleted.push(target);
        let campaign = describe(&router, region, &id, &case).await;
        assert_config(&campaign, &expected_config(&created, &deleted), &case);
    }

    // Every channel's block is gone, so the campaign carries no channel subtype
    // config at all rather than an empty one.
    let campaign = describe(&router, region, &id, "every channel deleted").await;
    assert!(
        campaign.get("channelSubtypeConfig").is_none(),
        "expected no channelSubtypeConfig after every channel went, got {:?}",
        campaign.get("channelSubtypeConfig")
    );
}
