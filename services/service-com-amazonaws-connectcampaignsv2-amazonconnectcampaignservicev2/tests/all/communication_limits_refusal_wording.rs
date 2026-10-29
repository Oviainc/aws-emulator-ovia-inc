// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of the `communicationLimitsList` `@length`, end to
//! end over the wire.
//!
//! The rule under test: a refusal of a modelled constraint carries the framework's one
//! validation envelope — the `<n> validation error(s) detected: ` count prefix wrapping
//! one clause per violation, each clause opening with the value it refused, naming the
//! member path it arrived at and the requirement it failed — whichever layer caught the
//! violation. A bound a handler has to weigh itself (a `@length` sitting on a union
//! member, which the centralized constraint walk does not reach) is therefore worded
//! exactly like a bound the boundary catches, so a client comparing two refusals of the
//! same class gets two sentences of the same shape.
//!
//! The rows span what the rule generalises over: every operation that accepts a
//! `CommunicationLimits` (`CreateCampaign`, `UpdateCampaignCommunicationLimits` and
//! `PutInstanceCommunicationLimits` — two request members, so two member paths) crossed
//! with list lengths on both sides of the published maximum, each refused length a
//! DIFFERENT count so the length the clause reports comes from the request rather than
//! from fixed text. The last case puts a handler-caught refusal beside a
//! boundary-caught one and asserts both read the same way.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::COMMUNICATION_LIMIT_TIME_UNIT_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::MAX_COMMUNICATION_LIMITS;

use super::common::{call, describe_campaign, error_code, onboard_instance, router, Harness};

/// The region every case signs for: nothing in a communication-limits config is
/// region-dependent, so one region carries every row.
const REGION: &str = "us-east-1";

/// The member path a campaign's override arrives under, which is what a refusal clause
/// names: the request member, the union member in force, then the list member.
const CAMPAIGN_MEMBER: &str =
    "communicationLimitsOverride.allChannelSubtypes.communicationLimitsList";

/// The member path the instance-wide config arrives under, spelled the same way.
const INSTANCE_MEMBER: &str = "communicationLimitsConfig.allChannelSubtypes.communicationLimitsList";

/// The count prefix the framework opens a single-violation refusal with.
const ENVELOPE_PREFIX: &str = "1 validation error detected: ";

/// The text that joins a clause's member path to the requirement it failed.
const CLAUSE_MARKER: &str = " failed to satisfy constraint: ";

/// A `CommunicationLimitsConfig`/`InstanceCommunicationLimitsConfig` whose
/// `communicationLimitsList` holds `count` entries, each inside `CommunicationLimit`'s
/// own modelled bounds (`maxCountPerRecipient` at least 1, `frequency` in 1..30, `unit`
/// from `CommunicationLimitTimeUnit`) so the only thing any case turns on is the length.
fn config_of(count: usize) -> Value {
    let unit = COMMUNICATION_LIMIT_TIME_UNIT_VALUES[0];
    let entries: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "maxCountPerRecipient": index as i64 + 1,
                "frequency": (index as i64 % 30) + 1,
                "unit": unit,
            })
        })
        .collect();
    json!({"allChannelSubtypes": {"communicationLimitsList": entries}})
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

/// The whole message a list of `count` entries at `member` is refused with: the
/// framework's envelope around one clause that echoes the length it refused in the
/// `<n> items` form a list's own length is displayed with, names the member and states
/// the published maximum.
fn expected_message(member: &str, count: usize) -> String {
    format!(
        "{ENVELOPE_PREFIX}Value '{count} items' at '{member}'{CLAUSE_MARKER}Member must have \
         length less than or equal to {MAX_COMMUNICATION_LIMITS}"
    )
}

/// The message a response carries.
fn message_of(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Assert `response` is the bad request a list past the maximum is refused with, worded
/// as [`expected_message`].
fn assert_refused(case: &str, status: u16, response: &Value, member: &str, count: usize) {
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
        expected_message(member, count),
        "case {case:?}: the refusal was worded {:?}",
        message_of(response)
    );
}

/// Assert `message` has the shape the framework gives every constraint refusal: the
/// count prefix, then a clause that opens with the refused value, names a member and
/// ends in the requirement that member failed.
fn assert_envelope_shape(case: &str, message: &str) {
    assert!(
        message.starts_with(ENVELOPE_PREFIX),
        "case {case:?}: expected the validation envelope prefix, got {message:?}"
    );
    let clause = &message[ENVELOPE_PREFIX.len()..];
    assert!(
        clause.starts_with("Value '") || clause.starts_with("Value null at '"),
        "case {case:?}: expected the clause to open with the refused value, got {clause:?}"
    );
    let Some((path, requirement)) = clause.split_once(CLAUSE_MARKER) else {
        panic!("case {case:?}: expected a clause naming the constraint, got {clause:?}");
    };
    assert!(
        path.ends_with('\''),
        "case {case:?}: expected the clause to name a quoted member path, got {path:?}"
    );
    assert!(
        requirement.starts_with("Member must "),
        "case {case:?}: expected the clause to end in the requirement, got {requirement:?}"
    );
}

/// `CreateCampaign` with `config` as the campaign's override.
async fn create_with(
    router: &Harness,
    instance: &str,
    name: &str,
    config: &Value,
) -> (u16, Value) {
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
async fn put_instance_limits(
    router: &Harness,
    instance: &str,
    config: &Value,
) -> (u16, Value) {
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

/// The list lengths every operation below is driven with: each published side of
/// `communicationLimitsList`'s `@length` (whose maximum is 2, per
/// `smithy_query(query="shape", shape="CommunicationLimits")`) — the empty list, one
/// entry, the maximum itself — and three different lengths past it, so the count the
/// refusal reports is read off the request and not fixed text.
fn lengths() -> [usize; 6] {
    [0, 1, MAX_COMMUNICATION_LIMITS, MAX_COMMUNICATION_LIMITS + 1, 4, 9]
}

#[tokio::test]
async fn an_over_long_list_is_refused_in_the_validation_envelope_at_create() {
    let router = router();
    let instance = "limits-wording-create";
    onboard_instance(&router, REGION, instance).await;

    for count in lengths() {
        let case = format!("CreateCampaign with {count} entries");
        let config = config_of(count);
        let (status, body) = create_with(&router, instance, &case, &config).await;

        if count <= MAX_COMMUNICATION_LIMITS {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            // The accepted config is what the campaign holds: the length check refuses
            // only what the model's maximum leaves out.
            let id = body["id"].as_str().unwrap_or_default().to_string();
            let campaign = describe_campaign(&router, REGION, &id, &case).await;
            assert_eq!(
                campaign["communicationLimitsOverride"], config,
                "case {case:?}: expected the campaign to hold {config}, got {}",
                campaign["communicationLimitsOverride"]
            );
        } else {
            assert_refused(&case, status, &body, CAMPAIGN_MEMBER, count);
        }
    }
}

#[tokio::test]
async fn an_over_long_list_is_refused_in_the_validation_envelope_at_the_campaign_update() {
    let router = router();
    let instance = "limits-wording-update";
    onboard_instance(&router, REGION, instance).await;
    let (status, created) = create_with(
        &router,
        instance,
        "limits wording update",
        &config_of(1),
    )
    .await;
    assert_eq!(status, 200, "the fixture campaign: got {status} {created}");
    let id = created["id"]
        .as_str()
        .expect("a minted campaign id")
        .to_string();

    // What the campaign holds, grown by each accepted row: every refused row is
    // asserted to leave exactly this.
    let mut stored = config_of(1);

    for count in lengths() {
        let case = format!("UpdateCampaignCommunicationLimits with {count} entries");
        let config = config_of(count);
        let (status, body) = update_campaign_limits(&router, &id, &config).await;

        if count <= MAX_COMMUNICATION_LIMITS {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            stored = config.clone();
        } else {
            assert_refused(&case, status, &body, CAMPAIGN_MEMBER, count);
        }

        let campaign = describe_campaign(&router, REGION, &id, &case).await;
        assert_eq!(
            campaign["communicationLimitsOverride"], stored,
            "case {case:?}: expected the campaign to hold {stored}, got {}",
            campaign["communicationLimitsOverride"]
        );
    }
}

#[tokio::test]
async fn an_over_long_list_is_refused_in_the_validation_envelope_at_the_instance_put() {
    let router = router();
    let instance = "limits-wording-instance-put";
    onboard_instance(&router, REGION, instance).await;

    // The instance holds no config until a put writes one, so the first rows assert
    // what a refusal leaves behind: nothing.
    let mut stored = Value::Null;

    for count in lengths() {
        let case = format!("PutInstanceCommunicationLimits with {count} entries");
        let config = config_of(count);
        let (status, body) = put_instance_limits(&router, instance, &config).await;

        if count <= MAX_COMMUNICATION_LIMITS {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            stored = config.clone();
        } else {
            // The SAME requirement, reported at this operation's own member path.
            assert_refused(&case, status, &body, INSTANCE_MEMBER, count);
        }

        let reported = instance_limits(&router, instance, &case).await;
        assert_eq!(
            reported, stored,
            "case {case:?}: expected the instance to hold {stored}, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_handler_caught_bound_and_a_boundary_caught_one_read_the_same() {
    let router = router();
    let instance = "limits-wording-comparison";
    onboard_instance(&router, REGION, instance).await;
    let (status, created) = create_with(
        &router,
        instance,
        "limits wording comparison",
        &config_of(1),
    )
    .await;
    assert_eq!(status, 200, "the fixture campaign: got {status} {created}");
    let id = created["id"]
        .as_str()
        .expect("a minted campaign id")
        .to_string();

    // One refusal per layer that can raise one, all for the same request family: the
    // `@length` on a union member, which only the handler can weigh; a tag key, which
    // the handler weighs because a map's key is never a leaf of the walk; and a member
    // the model makes required, which the boundary catches. The rule says all three are
    // one sentence shape.
    let (_, handler_length) =
        update_campaign_limits(&router, &id, &config_of(MAX_COMMUNICATION_LIMITS + 1)).await;
    let (_, handler_instance_length) = put_instance_limits(
        &router,
        instance,
        &config_of(MAX_COMMUNICATION_LIMITS + 3),
    )
    .await;
    let (_, handler_tag_key) = call(
        &router,
        REGION,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": "limits wording reserved tag",
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
            // A key carrying the reserved prefix `TagKey`'s `@pattern` excludes.
            "tags": {"aws:reserved": "v"},
        }),
    )
    .await;
    let (_, boundary_required) = call(
        &router,
        REGION,
        "POST",
        &format!("/v2/campaigns/{id}/communication-limits"),
        &json!({}),
    )
    .await;

    for (case, response) in [
        ("the campaign list length, caught in the handler", &handler_length),
        (
            "the instance list length, caught in the handler",
            &handler_instance_length,
        ),
        ("a tag key, caught in the handler", &handler_tag_key),
        (
            "a required member, caught at the boundary",
            &boundary_required,
        ),
    ] {
        assert_envelope_shape(case, message_of(response));
    }
}
