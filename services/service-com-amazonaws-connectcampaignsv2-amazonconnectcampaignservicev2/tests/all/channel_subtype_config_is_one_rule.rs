// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every operation that accepts a `ChannelSubtypeConfig` judges one by,
//! driven end to end over the wire through each of those operations.
//!
//! `CreateCampaign` is born with a `channelSubtypeConfig` and
//! `UpdateCampaignChannelSubtypeConfig` replaces one, and both are held to the same
//! rule rather than to one copy of it:
//!
//! * a config is taken only when EVERY leaf it carries satisfies the constraint the
//!   shape it targets declares — including the leaves behind the `outboundMode` union
//!   of each channel block and behind the `PacingStrategy` union one level deeper,
//!   which is where this config's ranges, list length, enum catalogs, string pattern
//!   and required members live;
//! * a config carrying a leaf outside its shape is HTTP 400 `ValidationException`
//!   NAMING that leaf's path through the config, and writes nothing: the create mints
//!   no campaign and the update leaves the stored config exactly as it was, so no
//!   record comes to hold — and no read to project — a `Campaign$channelSubtypeConfig`
//!   the model's own shape forbids;
//! * a config sitting exactly ON a published bound is taken and stored by both, so the
//!   refusal is the constraint and not an operation refusing configs at large.
//!
//! The constraints the rows are drawn from come from the
//! `UpdateCampaignChannelSubtypeConfig` closure, which both operations share through
//! `ChannelSubtypeConfig`: `Capacity` range 0.01..1; `BandwidthAllocation` range 0..2;
//! `TimeoutDuration` range 1..300; `PacingStrategyList` length 1..1; `TargetRate` range
//! 0..1; `AbandonmentRatePacingConfig$connectionThresholdSeconds` min 1;
//! `ConnectionStartPoint` values `CONNECTED_TO_SYSTEM` | `GREETING_START` |
//! `GREETING_END`; `EvaluationWindow` length ..5 with pattern
//! `^PT([1-9]|1[0-9]|2[0-4])H$`; `AgentAction` values `DISCARD`; `AgentActions`
//! `@uniqueItems`; and the `@required` members `ProgressiveConfig$bandwidthAllocation`,
//! `PreviewConfig$timeoutConfig` and `AbandonmentRatePacingConfig$evaluationWindow`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, create_campaign_with, describe_campaign, error_code, onboard_instance, router, Harness,
};

/// A telephony block running `outbound_mode`, with the one `@required` member of its
/// `defaultOutboundConfig` (`TelephonyOutboundConfig$connectContactFlowId`) and no
/// optional member set, so the only leaf a row varies is the one it is about.
fn telephony(outbound_mode: Value) -> Value {
    json!({
        "telephony": {
            "outboundMode": outbound_mode,
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

/// An `abandonmentRate` pacing strategy — the one member of the `PacingStrategy` union
/// — carrying every member `AbandonmentRatePacingConfig` requires.
fn abandonment_rate(target_rate: f64, start_point: &str, threshold: i64, window: &str) -> Value {
    json!({
        "abandonmentRate": {
            "targetRate": target_rate,
            "connectionStartPoint": start_point,
            "connectionThresholdSeconds": threshold,
            "evaluationWindow": window
        }
    })
}

/// A `predictive` telephony block pacing itself by `strategies`.
fn predictive(bandwidth: f64, strategies: Value) -> Value {
    telephony(json!({
        "predictive": {"bandwidthAllocation": bandwidth, "pacingStrategies": strategies}
    }))
}

/// A `preview` telephony block timing its agents out after `timeout` seconds.
fn preview(bandwidth: f64, timeout: i64) -> Value {
    telephony(json!({
        "preview": {
            "bandwidthAllocation": bandwidth,
            "timeoutConfig": {"durationInSeconds": timeout}
        }
    }))
}

/// One row of the refusal table: what an assertion calls it, the config the request
/// carries, and the path through the config the refusal must name — the leaf at fault,
/// so a row proves the refusal is about the constraint it is for and not about
/// something else the same body carries.
struct Refused {
    case: &'static str,
    config: Value,
    names: &'static str,
}

/// Every row a bad request: one leaf of the config outside the shape it targets, each
/// numeric range one step past each published bound, each list length one step past
/// each of its own, a value outside each enum catalog, a repeated element where the
/// list admits each once, a string outside its pattern and past its length, and a block
/// missing a `@required` member.
///
/// Sources: the constraints listed in this suite's module docs.
fn refused_configs() -> Vec<Refused> {
    let rows: Vec<(&'static str, Value, &'static str)> = vec![
        // `Capacity` 0.01..1 — a leaf of the channel block itself, beside the ones
        // behind its union, so the rule is the whole config and not one level of it.
        (
            "capacity one step below the 0.01 minimum",
            {
                let mut config = telephony(json!({"agentless": {}}));
                config["telephony"]["capacity"] = json!(0.009);
                config
            },
            "channelSubtypeConfig.telephony.capacity",
        ),
        (
            "capacity one step above the 1 maximum",
            {
                let mut config = telephony(json!({"agentless": {}}));
                config["telephony"]["capacity"] = json!(1.001);
                config
            },
            "channelSubtypeConfig.telephony.capacity",
        ),
        // `BandwidthAllocation` 0..2, behind the `outboundMode` union.
        (
            "a progressive bandwidthAllocation below the 0 minimum",
            telephony(json!({"progressive": {"bandwidthAllocation": -0.001}})),
            "outboundMode.progressive.bandwidthAllocation",
        ),
        (
            "a predictive bandwidthAllocation above the 2 maximum",
            telephony(json!({"predictive": {"bandwidthAllocation": 2.001}})),
            "outboundMode.predictive.bandwidthAllocation",
        ),
        (
            "a preview bandwidthAllocation above the 2 maximum",
            json!({
                "telephony": {
                    "outboundMode": {"preview": {
                        "bandwidthAllocation": 7.5,
                        "timeoutConfig": {"durationInSeconds": 30}
                    }},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            }),
            "outboundMode.preview.bandwidthAllocation",
        ),
        // `TimeoutDuration` 1..300, two levels behind the union.
        (
            "a preview timeout below the 1 second minimum",
            preview(1.0, 0),
            "outboundMode.preview.timeoutConfig.durationInSeconds",
        ),
        (
            "a preview timeout above the 300 second maximum",
            preview(1.0, 301),
            "outboundMode.preview.timeoutConfig.durationInSeconds",
        ),
        // `PacingStrategyList` length 1..1, one step past each of its bounds.
        (
            "no pacing strategy where the list holds one",
            predictive(1.0, json!([])),
            "outboundMode.predictive.pacingStrategies",
        ),
        (
            "two pacing strategies where the list holds one",
            predictive(
                1.0,
                json!([
                    abandonment_rate(0.05, "GREETING_START", 10, "PT4H"),
                    abandonment_rate(0.10, "GREETING_END", 20, "PT8H")
                ]),
            ),
            "outboundMode.predictive.pacingStrategies",
        ),
        // `TargetRate` 0..1, behind the `PacingStrategy` union.
        (
            "a targetRate below the 0 minimum",
            predictive(
                1.0,
                json!([abandonment_rate(-0.001, "GREETING_END", 10, "PT4H")]),
            ),
            "abandonmentRate.targetRate",
        ),
        (
            "a targetRate above the 1 maximum",
            predictive(
                1.0,
                json!([abandonment_rate(1.001, "GREETING_END", 10, "PT4H")]),
            ),
            "abandonmentRate.targetRate",
        ),
        // `connectionThresholdSeconds` min 1.
        (
            "a connectionThresholdSeconds below the 1 minimum",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_END", 0, "PT4H")]),
            ),
            "abandonmentRate.connectionThresholdSeconds",
        ),
        // The `ConnectionStartPoint` catalog.
        (
            "a connectionStartPoint the catalog does not hold",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_MIDDLE", 10, "PT4H")]),
            ),
            "abandonmentRate.connectionStartPoint",
        ),
        // `EvaluationWindow`'s pattern, one step past each end of the hour range it
        // admits, and a window past the `@length` maximum of 5 as well.
        (
            "an evaluationWindow of zero hours, below the pattern's range",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_END", 10, "PT0H")]),
            ),
            "abandonmentRate.evaluationWindow",
        ),
        (
            "an evaluationWindow of 25 hours, above the pattern's range",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_END", 10, "PT25H")]),
            ),
            "abandonmentRate.evaluationWindow",
        ),
        (
            "an evaluationWindow longer than the 5-character maximum",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_END", 10, "PT120H")]),
            ),
            "abandonmentRate.evaluationWindow",
        ),
        (
            "an evaluationWindow in minutes, which the pattern does not admit",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_END", 10, "PT4M")]),
            ),
            "abandonmentRate.evaluationWindow",
        ),
        // The `AgentAction` catalog, and the `@uniqueItems` of the list drawn from it.
        (
            "an agent action the catalog does not hold",
            json!({
                "telephony": {
                    "outboundMode": {"preview": {
                        "bandwidthAllocation": 1.0,
                        "timeoutConfig": {"durationInSeconds": 30},
                        "agentActions": ["ESCALATE"]
                    }},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            }),
            "outboundMode.preview.agentActions",
        ),
        (
            "the one agent action the catalog holds, repeated",
            json!({
                "telephony": {
                    "outboundMode": {"preview": {
                        "bandwidthAllocation": 1.0,
                        "timeoutConfig": {"durationInSeconds": 30},
                        "agentActions": ["DISCARD", "DISCARD"]
                    }},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            }),
            "outboundMode.preview.agentActions",
        ),
        // The `@required` members behind the union, at each of its three depths.
        (
            "a progressive config carrying no bandwidthAllocation",
            telephony(json!({"progressive": {}})),
            "bandwidthAllocation",
        ),
        (
            "a preview config carrying no timeoutConfig",
            telephony(json!({"preview": {"bandwidthAllocation": 1.0}})),
            "timeoutConfig",
        ),
        (
            "an abandonment rate carrying no evaluationWindow",
            predictive(
                1.0,
                json!([{"abandonmentRate": {
                    "targetRate": 0.5,
                    "connectionStartPoint": "GREETING_END",
                    "connectionThresholdSeconds": 10
                }}]),
            ),
            "evaluationWindow",
        ),
    ];
    rows.into_iter()
        .map(|(case, config, names)| Refused {
            case,
            config,
            names,
        })
        .collect()
}

/// Every row a config the model admits, each sitting exactly ON a published bound or
/// naming an entry of a catalog, so each constraint above is refused only outside
/// itself.
fn accepted_configs() -> Vec<(&'static str, Value)> {
    let mut rows = vec![
        ("capacity at the 0.01 minimum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["capacity"] = json!(0.01);
            config
        }),
        ("capacity at the 1 maximum", {
            let mut config = telephony(json!({"agentless": {}}));
            config["telephony"]["capacity"] = json!(1.0);
            config
        }),
        (
            "a progressive bandwidthAllocation at the 0 minimum",
            telephony(json!({"progressive": {"bandwidthAllocation": 0.0}})),
        ),
        (
            "a predictive bandwidthAllocation at the 2 maximum",
            telephony(json!({"predictive": {"bandwidthAllocation": 2.0}})),
        ),
        ("a preview timeout at the 1 second minimum", preview(2.0, 1)),
        (
            "a preview timeout at the 300 second maximum",
            preview(0.0, 300),
        ),
        (
            "a preview carrying the one agent action the catalog holds",
            json!({
                "telephony": {
                    "outboundMode": {"preview": {
                        "bandwidthAllocation": 1.0,
                        "timeoutConfig": {"durationInSeconds": 30},
                        "agentActions": ["DISCARD"]
                    }},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            }),
        ),
        (
            "the one pacing strategy the list holds, at the 0 targetRate and 1-hour window",
            predictive(
                1.0,
                json!([abandonment_rate(0.0, "CONNECTED_TO_SYSTEM", 1, "PT1H")]),
            ),
        ),
        (
            "a pacing strategy at the 1 targetRate and the 24-hour window",
            predictive(
                1.0,
                json!([abandonment_rate(1.0, "GREETING_END", 600, "PT24H")]),
            ),
        ),
        (
            "a two-digit evaluation window inside the pattern's range",
            predictive(
                1.0,
                json!([abandonment_rate(0.5, "GREETING_START", 30, "PT19H")]),
            ),
        ),
    ];
    // Every value of the `ConnectionStartPoint` catalog is a start point a pacing
    // strategy can name, so each is driven as its own row.
    for start_point in ["CONNECTED_TO_SYSTEM", "GREETING_START", "GREETING_END"] {
        rows.push((
            match start_point {
                "CONNECTED_TO_SYSTEM" => "a pacing strategy connected at CONNECTED_TO_SYSTEM",
                "GREETING_START" => "a pacing strategy connected at GREETING_START",
                _ => "a pacing strategy connected at GREETING_END",
            },
            predictive(
                0.5,
                json!([abandonment_rate(0.25, start_point, 5, "PT2H")]),
            ),
        ));
    }
    rows
}

/// `UpdateCampaignChannelSubtypeConfig` for `id`, carrying `config`.
async fn update_subtype_config(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/channel-subtype-config"),
        &json!({"channelSubtypeConfig": config}),
    )
    .await
}

/// `CreateCampaign` on `instance` with `config` as the campaign's
/// `channelSubtypeConfig`, whatever it answers.
async fn create_with_config(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    config: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            "channelSubtypeConfig": config
        }),
    )
    .await
}

/// The names of every campaign `ListCampaigns` reports in `region` — how a case asks
/// whether a refused create minted anything.
async fn campaign_names(router: &MultiServiceRouter, region: &str, case: &str) -> Vec<String> {
    let (status, listed) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {listed}"
    );
    listed["campaignSummaryList"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary["name"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the refusal in `body` names `path` — the member path of the leaf at fault,
/// which a `ValidationException` reports the failing value at.
fn refusal_names(body: &Value, path: &str) -> bool {
    body["message"]
        .as_str()
        .is_some_and(|message| message.contains(path))
}

/// The campaign this suite's refusal cases hold against: one carrying a config already,
/// so a write that slipped past a refusal would show in `DescribeCampaign`.
async fn kept_campaign(router: &Harness, region: &str, instance: &str, kept: &Value) -> String {
    create_campaign_with(
        router,
        region,
        instance,
        "channel-subtype-one-rule-kept",
        json!({"channelSubtypeConfig": kept}),
    )
    .await
}

/// Both operations refuse a config carrying a leaf its shape forbids, with the same
/// status, the same modelled error and a message naming that leaf, and neither writes
/// anything: the create mints no campaign and the update leaves the stored config
/// alone.
#[tokio::test]
async fn both_operations_refuse_a_config_the_model_forbids() {
    let region = "us-east-2";
    let instance = "channel-subtype-one-rule";
    let router = router();
    onboard_instance(&router, region, instance).await;

    let kept = telephony(json!({"agentless": {}}));
    let id = kept_campaign(&router, region, instance, &kept).await;

    for (index, row) in refused_configs().iter().enumerate() {
        let case = row.case;
        let name = format!("channel-subtype-one-rule-refused-{index}");

        let (status, body) =
            create_with_config(&router, region, instance, &name, &row.config).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateCampaign expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: CreateCampaign expected ValidationException, got {}",
            error_code(&body)
        );
        assert!(
            refusal_names(&body, row.names),
            "case {case:?}: CreateCampaign expected the refusal to name {:?}, got {:?}",
            row.names,
            body["message"]
        );
        let names = campaign_names(&router, region, case).await;
        assert!(
            !names.contains(&name),
            "case {case:?}: expected the refused create to mint nothing, got {names:?}"
        );

        let (status, body) = update_subtype_config(&router, region, &id, &row.config).await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateCampaignChannelSubtypeConfig expected HTTP 400, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: UpdateCampaignChannelSubtypeConfig expected ValidationException, \
             got {}",
            error_code(&body)
        );
        assert!(
            refusal_names(&body, row.names),
            "case {case:?}: UpdateCampaignChannelSubtypeConfig expected the refusal to name \
             {:?}, got {:?}",
            row.names,
            body["message"]
        );
        let campaign = describe_campaign(&router, region, &id, case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], kept,
            "case {case:?}: expected the refusal to leave the stored config alone, got {:?}",
            campaign["channelSubtypeConfig"]
        );
    }
}

/// Both operations take a config sitting on a published bound, and what each stores is
/// read back whole through `DescribeCampaign` — so the refusals above are the
/// constraints themselves and not either operation refusing configs at large.
#[tokio::test]
async fn both_operations_take_a_config_on_its_bounds() {
    let region = "eu-central-1";
    let instance = "channel-subtype-one-rule-bounds";
    let router = router();
    onboard_instance(&router, region, instance).await;

    let kept = telephony(json!({"agentless": {}}));
    let updated = kept_campaign(&router, region, instance, &kept).await;

    for (index, (case, config)) in accepted_configs().iter().enumerate() {
        let name = format!("channel-subtype-one-rule-accepted-{index}");

        let (status, body) = create_with_config(&router, region, instance, &name, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateCampaign expected HTTP 200, got {status} {body}"
        );
        let created = body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateCampaign returned no id: {body}"))
            .to_string();
        let campaign = describe_campaign(&router, region, &created, case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], *config,
            "case {case:?}: expected the created campaign to hold the config it was given, \
             got {:?}",
            campaign["channelSubtypeConfig"]
        );

        let (status, body) = update_subtype_config(&router, region, &updated, config).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignChannelSubtypeConfig expected HTTP 200, \
             got {status} {body}"
        );
        let campaign = describe_campaign(&router, region, &updated, case).await;
        assert_eq!(
            campaign["channelSubtypeConfig"], *config,
            "case {case:?}: expected the update to store the config it was given, got {:?}",
            campaign["channelSubtypeConfig"]
        );
    }
}
