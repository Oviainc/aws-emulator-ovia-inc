// SPDX-License-Identifier: CC0-1.0

//! The `@range` bounds of a `CommunicationLimit` sitting BEHIND the
//! `CommunicationLimits` union, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test, over every operation that accepts a
//! `CommunicationLimits` — `CreateCampaign`, `UpdateCampaignCommunicationLimits` and
//! `PutInstanceCommunicationLimits`:
//!
//! * every numeric member of a `CommunicationLimit` is held to the `@range` its model
//!   declares — `maxCountPerRecipient` at least 1, `frequency` 1..30 — so a value
//!   outside those bounds is HTTP 400 `ValidationException` whose message is the
//!   framework's validation envelope naming the offending LIST ELEMENT and the
//!   requirement it failed, and the call stores nothing;
//! * every value inside them is accepted, stored and read back by the operation that
//!   reports the config (`DescribeCampaign`, `GetInstanceCommunicationLimits`),
//!   including at each published bound and well past the member that has no maximum.
//!
//! The rows differ along what the rule generalises over: which member is out of range,
//! which side of its bounds the value falls, how far past it, which list POSITION
//! carries it, and how many members of one element are out of range at once. The bounds
//! asserted here are read back out of `smithy-model/` by
//! [`the_communication_limit_rules_this_suite_asserts_are_the_models`], so no row can be
//! asserting a bound the model no longer declares.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::COMMUNICATION_LIMIT_TIME_UNIT_VALUES;

use super::common::{call, describe_campaign, error_code, onboard_instance, router, Harness};

/// The region every case signs for: nothing in a communication-limits config is
/// region-dependent, so one region carries every row.
const REGION: &str = "us-east-1";

/// `CommunicationLimit$maxCountPerRecipient`'s published `@range`, from
/// `smithy_query(query="shape", shape="CommunicationLimit")`: a minimum of 1 and no
/// maximum at all. Pinned to `smithy-model/` by
/// [`the_communication_limit_rules_this_suite_asserts_are_the_models`].
const MAX_COUNT_MIN: i64 = 1;

/// `CommunicationLimit$frequency`'s published `@range`, from the same query: 1..30.
const FREQUENCY_MIN: i64 = 1;
const FREQUENCY_MAX: i64 = 30;

/// The member path a campaign's override arrives under, which is what a refusal clause
/// names: the request member, the union member in force, then the list member.
const CAMPAIGN_MEMBER: &str =
    "communicationLimitsOverride.allChannelSubtypes.communicationLimitsList";

/// The member path the instance-wide config arrives under, spelled the same way.
const INSTANCE_MEMBER: &str =
    "communicationLimitsConfig.allChannelSubtypes.communicationLimitsList";

/// The one `CommunicationLimitTimeUnit` the model's catalog declares — every row below
/// turns on a numeric member, so the unit is held constant and always a catalog value.
fn unit() -> &'static str {
    COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0]
}

/// One `CommunicationLimit` entry on the wire.
fn limit(max_count_per_recipient: i64, frequency: i64) -> Value {
    json!({
        "maxCountPerRecipient": max_count_per_recipient,
        "frequency": frequency,
        "unit": unit(),
    })
}

/// A `CommunicationLimitsConfig`/`InstanceCommunicationLimitsConfig` whose
/// `communicationLimitsList` holds `entries`.
fn config_of(entries: Vec<Value>) -> Value {
    json!({"allChannelSubtypes": {"communicationLimitsList": entries}})
}

/// The requirement clause a refusal states for a value below a `@range` minimum.
fn min_value(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement clause a refusal states for a value past a `@range` maximum.
fn max_value(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// One clause of a refusal: the value that was refused, the path (relative to the
/// operation's own request member) it arrived at, and the requirement it failed.
struct Clause {
    value: i64,
    path: String,
    requirement: String,
}

impl Clause {
    /// The refused `value` at list position `position` (1-based, as the envelope spells
    /// a list element), under `member` of the element.
    fn new(position: usize, member: &str, value: i64, requirement: String) -> Self {
        Self {
            value,
            path: format!("{position}.member.{member}"),
            requirement,
        }
    }

    /// The clause as the envelope words it, under the request `member` the operation
    /// takes the config at.
    fn message(&self, member: &str) -> String {
        format!(
            "Value '{}' at '{member}.{}' failed to satisfy constraint: {}",
            self.value, self.path, self.requirement
        )
    }
}

/// One row of the table: the case name, the list the request carries, and the clauses a
/// refusal must state — empty when the model admits the whole list.
struct Row {
    case: &'static str,
    entries: Vec<Value>,
    refusal: Vec<Clause>,
}

/// The whole message a refused row must come back with, under the request `member`: the
/// framework's envelope — the count prefix wrapping one clause per violation, joined
/// with `; ` — around this row's clauses.
fn expected_message(row: &Row, member: &str) -> String {
    let clauses: Vec<String> = row
        .refusal
        .iter()
        .map(|clause| clause.message(member))
        .collect();
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The message a response carries.
fn message_of(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The table every operation below is driven with.
///
/// The accepted rows sit at each published bound (`maxCountPerRecipient`'s minimum,
/// both of `frequency`'s) and well past the bound `maxCountPerRecipient` does NOT have;
/// the refused rows fall one step outside each bound, far outside it, at the second list
/// position rather than the first, and — the last row — outside two bounds at once, so
/// the envelope's count comes from the request rather than fixed text.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every member at its published minimum",
            entries: vec![limit(MAX_COUNT_MIN, FREQUENCY_MIN)],
            refusal: Vec::new(),
        },
        Row {
            case: "frequency at its published maximum",
            entries: vec![limit(MAX_COUNT_MIN, FREQUENCY_MAX)],
            refusal: Vec::new(),
        },
        Row {
            case: "a maxCountPerRecipient far past frequency's maximum, which its own \
                   @range does not bound",
            entries: vec![limit(1_000_000, FREQUENCY_MAX - 1)],
            refusal: Vec::new(),
        },
        Row {
            case: "two entries, both inside every bound",
            entries: vec![
                limit(MAX_COUNT_MIN, FREQUENCY_MIN),
                limit(7, FREQUENCY_MAX),
            ],
            refusal: Vec::new(),
        },
        Row {
            case: "maxCountPerRecipient one below its minimum",
            entries: vec![limit(MAX_COUNT_MIN - 1, FREQUENCY_MIN)],
            refusal: vec![Clause::new(
                1,
                "maxCountPerRecipient",
                MAX_COUNT_MIN - 1,
                min_value(MAX_COUNT_MIN),
            )],
        },
        Row {
            case: "a negative maxCountPerRecipient",
            entries: vec![limit(-42, FREQUENCY_MIN)],
            refusal: vec![Clause::new(
                1,
                "maxCountPerRecipient",
                -42,
                min_value(MAX_COUNT_MIN),
            )],
        },
        Row {
            case: "frequency one below its minimum",
            entries: vec![limit(MAX_COUNT_MIN, FREQUENCY_MIN - 1)],
            refusal: vec![Clause::new(
                1,
                "frequency",
                FREQUENCY_MIN - 1,
                min_value(FREQUENCY_MIN),
            )],
        },
        Row {
            case: "frequency one past its maximum",
            entries: vec![limit(MAX_COUNT_MIN, FREQUENCY_MAX + 1)],
            refusal: vec![Clause::new(
                1,
                "frequency",
                FREQUENCY_MAX + 1,
                max_value(FREQUENCY_MAX),
            )],
        },
        Row {
            case: "frequency far past its maximum",
            entries: vec![limit(3, 500)],
            refusal: vec![Clause::new(1, "frequency", 500, max_value(FREQUENCY_MAX))],
        },
        Row {
            case: "the SECOND entry out of range, the first inside every bound",
            entries: vec![limit(3, 2), limit(3, FREQUENCY_MAX + 7)],
            refusal: vec![Clause::new(
                2,
                "frequency",
                FREQUENCY_MAX + 7,
                max_value(FREQUENCY_MAX),
            )],
        },
        Row {
            case: "one entry outside both of its members' bounds",
            entries: vec![limit(0, 500)],
            refusal: vec![
                Clause::new(1, "maxCountPerRecipient", 0, min_value(MAX_COUNT_MIN)),
                Clause::new(1, "frequency", 500, max_value(FREQUENCY_MAX)),
            ],
        },
    ]
}

/// Assert `response` is the bad request this row must draw, worded as
/// [`expected_message`].
fn assert_refused(row: &Row, member: &str, status: u16, response: &Value) {
    let case = row.case;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {response}"
    );
    assert!(
        error_code(response).ends_with("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?}",
        error_code(response)
    );
    assert_eq!(
        message_of(response),
        expected_message(row, member),
        "case {case:?}: the refusal was worded {:?}",
        message_of(response)
    );
}

/// The telephony channel config every campaign in this suite is created with — both
/// members the model makes required.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
        }
    })
}

/// `CreateCampaign` with `config` as the campaign's override.
async fn create_with(router: &Harness, instance: &str, name: &str, config: &Value) -> (u16, Value) {
    call(
        router,
        REGION,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
            "communicationLimitsOverride": config,
        }),
    )
    .await
}

/// `UpdateCampaignCommunicationLimits` for `id` with `config` as the override.
async fn update_campaign_limits(router: &Harness, id: &str, config: &Value) -> (u16, Value) {
    call(
        router,
        REGION,
        "POST",
        &format!("/v2/campaigns/{id}/communication-limits"),
        &json!({"communicationLimitsOverride": config}),
    )
    .await
}

/// `PutInstanceCommunicationLimits` for `instance` with `config`.
async fn put_instance_limits(router: &Harness, instance: &str, config: &Value) -> (u16, Value) {
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &json!({"communicationLimitsConfig": config}),
    )
    .await
}

/// The config `GetInstanceCommunicationLimits` reports for `instance`.
async fn instance_limits(router: &Harness, instance: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/communication-limits"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetInstanceCommunicationLimits expected HTTP 200, got {status} {body}"
    );
    body.get("communicationLimitsConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// How many campaigns this region holds, as `ListCampaigns` reports them — how a case
/// asserts a refused create minted nothing.
async fn campaign_count(router: &Harness, case: &str) -> usize {
    let (status, body) = call(router, REGION, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected HTTP 200, got {status} {body}"
    );
    body.get("campaignSummaryList")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// The bounds this suite asserts are the model's.
///
/// Every bound is enforced off `CommunicationLimit`'s own generated schema rather than
/// restated in the crate, so this pins the suite's rows — a model refresh that moved a
/// bound fails here instead of leaving the rows asserting a stale one.
#[test]
fn the_communication_limit_rules_this_suite_asserts_are_the_models() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json"
    );
    let text = std::fs::read_to_string(path).expect("the service's Smithy model is readable");
    let model: Value = serde_json::from_str(&text).expect("the service's Smithy model is JSON");
    let members =
        &model["shapes"]["com.amazonaws.connectcampaignsv2#CommunicationLimit"]["members"];

    let max_count = &members["maxCountPerRecipient"]["traits"]["smithy.api#range"];
    assert_eq!(
        max_count["min"],
        json!(MAX_COUNT_MIN),
        "case \"maxCountPerRecipient @range min\": this suite's floor must be the modelled \
         one, got {max_count}"
    );
    assert!(
        max_count.get("max").is_none(),
        "case \"maxCountPerRecipient @range max\": the rows assume the member has no \
         modelled ceiling, got {max_count}"
    );

    let frequency = &members["frequency"]["traits"]["smithy.api#range"];
    assert_eq!(
        frequency["min"],
        json!(FREQUENCY_MIN),
        "case \"frequency @range min\": this suite's floor must be the modelled one, got \
         {frequency}"
    );
    assert_eq!(
        frequency["max"],
        json!(FREQUENCY_MAX),
        "case \"frequency @range max\": this suite's ceiling must be the modelled one, got \
         {frequency}"
    );
}

/// `CreateCampaign` holds the override it is given to `CommunicationLimit`'s `@range`,
/// and a campaign is minted only for a config inside it.
#[tokio::test]
async fn create_campaign_holds_a_communication_limit_to_its_range() {
    let router = router();
    let instance = "limit-range-create";
    onboard_instance(&router, REGION, instance).await;

    let mut minted = 0usize;
    for row in rows() {
        let config = config_of(row.entries.clone());
        let (status, body) = create_with(&router, instance, row.case, &config).await;

        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200, got {status} {body}",
                row.case
            );
            minted += 1;
            // The accepted config is what the campaign holds, verbatim.
            let id = body["id"].as_str().unwrap_or_default().to_string();
            let campaign = describe_campaign(&router, REGION, &id, row.case).await;
            assert_eq!(
                campaign["communicationLimitsOverride"], config,
                "case {:?}: expected the campaign to hold {config}, got {}",
                row.case, campaign["communicationLimitsOverride"]
            );
        } else {
            assert_refused(&row, CAMPAIGN_MEMBER, status, &body);
        }

        let held = campaign_count(&router, row.case).await;
        assert_eq!(
            held, minted,
            "case {:?}: expected {minted} campaigns to exist, got {held}",
            row.case
        );
    }
}

/// `UpdateCampaignCommunicationLimits` holds the override it is given to the same
/// `@range`, and a refused update leaves the campaign's stored override alone.
#[tokio::test]
async fn updating_a_campaigns_limits_holds_them_to_their_range() {
    let router = router();
    let instance = "limit-range-update";
    onboard_instance(&router, REGION, instance).await;
    let (status, created) = create_with(
        &router,
        instance,
        "limit range update",
        &config_of(vec![limit(3, 3)]),
    )
    .await;
    assert_eq!(status, 200, "the fixture campaign: got {status} {created}");
    let id = created["id"]
        .as_str()
        .expect("a minted campaign id")
        .to_string();

    // What the campaign holds, replaced by each accepted row: every refused row is
    // asserted to leave exactly this.
    let mut stored = config_of(vec![limit(3, 3)]);

    for row in rows() {
        let config = config_of(row.entries.clone());
        let (status, body) = update_campaign_limits(&router, &id, &config).await;

        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200, got {status} {body}",
                row.case
            );
            stored = config.clone();
        } else {
            assert_refused(&row, CAMPAIGN_MEMBER, status, &body);
        }

        let campaign = describe_campaign(&router, REGION, &id, row.case).await;
        assert_eq!(
            campaign["communicationLimitsOverride"], stored,
            "case {:?}: expected the campaign to hold {stored}, got {}",
            row.case, campaign["communicationLimitsOverride"]
        );
    }
}

/// `PutInstanceCommunicationLimits` holds the instance-wide config to the same
/// `@range`, reports the same requirement at its OWN member path, and a refused put
/// leaves the instance's stored config alone.
#[tokio::test]
async fn putting_instance_limits_holds_them_to_their_range() {
    let router = router();
    let instance = "limit-range-instance";
    onboard_instance(&router, REGION, instance).await;

    // The instance holds no config until a put writes one, so the first rows assert
    // what a refusal leaves behind: nothing.
    let mut stored = Value::Null;

    for row in rows() {
        let config = config_of(row.entries.clone());
        let (status, body) = put_instance_limits(&router, instance, &config).await;

        if row.refusal.is_empty() {
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200, got {status} {body}",
                row.case
            );
            stored = config.clone();
        } else {
            // The SAME requirement, reported at this operation's own member path.
            assert_refused(&row, INSTANCE_MEMBER, status, &body);
        }

        let reported = instance_limits(&router, instance, row.case).await;
        assert_eq!(
            reported, stored,
            "case {:?}: expected the instance to hold {stored}, got {reported}",
            row.case
        );
    }
}
