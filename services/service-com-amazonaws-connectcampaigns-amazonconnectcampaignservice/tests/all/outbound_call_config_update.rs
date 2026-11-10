// SPDX-License-Identifier: CC0-1.0

//! Amending the outbound-call config of a campaign, end to end over HTTP.
//!
//! One rule, asserted over a table whose rows differ along WHICH members an amendment
//! carries and what they ask for, rather than along the single request that exposed the
//! gap: `UpdateCampaignOutboundCallConfig` replaces, in the outbound-call config of the
//! campaign the path addresses, exactly the members the request carries, leaves every
//! member it omits — the stored queue id among them, since the request models no member
//! for it — at the value it already had, and touches nothing else about the campaign. So
//! each later read of that campaign reports the config it was created with as amended by
//! the requests that have reached it. An amendment asking only for values the campaign
//! already carries is the no-op the operation's declared idempotence makes it, and an id
//! naming no stored campaign is refused in the words this service answers a
//! campaign-resource lookup that resolves to nothing with, storing nothing.

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::campaign_not_found_message;

use super::common::{call, router, Answer, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance every campaign below is created for.
const INSTANCE: &str = "outbound-config-instance";

/// `ContactFlowId`'s published length bound (`@length(max 500)`, from
/// `smithy_query(query="shape", shape="ContactFlowId")`).
const CONTACT_FLOW_MAX: usize = 500;

/// `SourcePhoneNumber`'s published length bound (`@length(max 100)`, from
/// `smithy_query(query="shape", shape="SourcePhoneNumber")`).
const SOURCE_PHONE_MAX: usize = 100;

/// The outbound-call config every campaign below is created with: every member the
/// `OutboundCallConfig` shape carries, so a case can see which of them an amendment moves
/// and which it leaves.
fn created_config() -> Value {
    json!({
        "connectContactFlowId": "flow-created",
        "connectSourcePhoneNumber": "+15550100",
        "connectQueueId": "queue-created",
        "answerMachineDetectionConfig": {
            "enableAnswerMachineDetection": true,
            "awaitAnswerMachinePrompt": false,
        },
    })
}

/// A `CreateCampaign` body for `INSTANCE`, carrying the config above plus the members an
/// update must leave untouched.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": INSTANCE,
        "dialerConfig": {"predictiveDialerConfig": {"bandwidthAllocation": 0.75}},
        "outboundCallConfig": created_config(),
        "tags": {"owner": "outbound-config-suite"},
    })
}

/// Register `INSTANCE` with the account and create one campaign, handing back the minted
/// campaign id.
async fn a_stored_campaign(router: &Harness, name: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case \"create the campaign to amend\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    created.body["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"create the campaign to amend\": no id in {}",
                created.body
            )
        })
        .to_string()
}

/// Ask for `amendment` on the outbound-call config of the campaign `id` names.
async fn amend(router: &Harness, id: &str, amendment: &Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/campaigns/{id}/outbound-call-config"),
        amendment,
    )
    .await
}

/// The campaign `id` names, as `DescribeCampaign` reports it.
async fn described(router: &Harness, id: &str) -> Value {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!("/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"read the campaign back\": expected HTTP 200, got {} {}",
        answer.status, answer.body
    );
    answer.body["campaign"].clone()
}

/// `value` with every null member of every object in it dropped, so a member reported as
/// absent and one reported as null compare equal: which of the two a response carries is
/// the serializer's concern, not this rule's.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(elements) => Value::Array(elements.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// The config `expected` describes once `amendment` has been applied to it under the
/// rule: a member the amendment carries replaces the stored one, and a member it omits
/// leaves the stored one where it is.
fn patched(expected: &Value, amendment: &Value) -> Value {
    let mut patched = expected.clone();
    // Exactly the members `UpdateCampaignOutboundCallConfigRequest` models beside the
    // campaign id (from `smithy_query(query="operation",
    // operation="UpdateCampaignOutboundCallConfig")`).
    for member in [
        "connectContactFlowId",
        "connectSourcePhoneNumber",
        "answerMachineDetectionConfig",
    ] {
        if let Some(asked) = amendment.get(member) {
            patched[member] = asked.clone();
        }
    }
    patched
}

/// Amendments one campaign is taken through in order, spanning which members an update
/// carries and what each may ask for.
fn amendment_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "every member the request models, at once",
            json!({
                "connectContactFlowId": "flow-all-three",
                "connectSourcePhoneNumber": "+15550111",
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": false,
                },
            }),
        ),
        (
            "the contact flow id alone",
            json!({"connectContactFlowId": "flow-only"}),
        ),
        (
            "the source phone number alone, in E.164 form",
            json!({"connectSourcePhoneNumber": "+442071838750"}),
        ),
        (
            "the detection config alone, with both of its members",
            json!({
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": true,
                    "awaitAnswerMachinePrompt": true,
                }
            }),
        ),
        (
            "the detection config alone, with only its required member",
            json!({
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": false,
                }
            }),
        ),
        (
            "no optional member at all",
            json!({}),
        ),
        (
            "a contact flow id at its 500-character length maximum",
            json!({"connectContactFlowId": "f".repeat(CONTACT_FLOW_MAX)}),
        ),
        (
            "a source phone number at its 100-character length maximum",
            json!({"connectSourcePhoneNumber": "9".repeat(SOURCE_PHONE_MAX)}),
        ),
        (
            "a contact flow id of one character",
            json!({"connectContactFlowId": "f"}),
        ),
        (
            "the contact flow id and the detection config together",
            json!({
                "connectContactFlowId": "flow-pair",
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": true,
                    "awaitAnswerMachinePrompt": false,
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_leaves_every_other_value_alone() {
    let router = router();
    let id = a_stored_campaign(&router, "amendable").await;
    let before = described(&router, &id).await;

    // The campaign is put into a run state first, so an amendment is shown to leave the
    // run state the lifecycle operations own exactly where it was.
    let started = call(
        &router,
        REGION,
        "POST",
        &format!("/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"start the campaign before amending it\": expected HTTP 200, got {} {}",
        started.status, started.body
    );

    let mut expected = created_config();
    for (case, amendment) in amendment_rows() {
        let amended = amend(&router, &id, &amendment).await;
        assert_eq!(
            amended.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} {}",
            amended.status, amended.body
        );

        expected = patched(&expected, &amendment);
        let after = described(&router, &id).await;
        assert_eq!(
            without_nulls(&after["outboundCallConfig"]),
            without_nulls(&expected),
            "case {case:?}: expected the stored config to carry the amended members and \
             keep the rest, got {after}"
        );
        // The queue id has no member in this request, so no amendment can move it.
        assert_eq!(
            after.pointer("/outboundCallConfig/connectQueueId"),
            before.pointer("/outboundCallConfig/connectQueueId"),
            "case {case:?}: expected the stored queue id to survive the update, got {after}"
        );

        // Every other member of the campaign is the one the create stored.
        for member in ["id", "arn", "name", "connectInstanceId", "dialerConfig", "tags"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: expected {member} to survive the update unchanged, got {after}"
            );
        }

        let state = call(
            &router,
            REGION,
            "GET",
            &format!("/campaigns/{id}/state"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (
                state.status,
                state
                    .body
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
            ),
            (200, "Running"),
            "case {case:?}: expected the run state to survive the update, got {} {}",
            state.status,
            state.body
        );
    }
}

#[tokio::test]
async fn asking_for_values_a_campaign_already_carries_succeeds_and_changes_nothing() {
    let router = router();
    let id = a_stored_campaign(&router, "steady-config").await;

    // Rows differ along how a repeat is reached: the values the create stored, values an
    // amendment moved the config to, and an amendment that names no member at all.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the values the create stored",
            json!({
                "connectContactFlowId": "flow-created",
                "connectSourcePhoneNumber": "+15550100",
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": true,
                    "awaitAnswerMachinePrompt": false,
                },
            }),
        ),
        (
            "values an amendment moved the config to",
            json!({
                "connectContactFlowId": "flow-repeated",
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": false,
                },
            }),
        ),
        ("no member at all", json!({})),
    ];

    let mut expected = created_config();
    for (case, amendment) in rows {
        expected = patched(&expected, &amendment);
        for attempt in 1..=3 {
            let amended = amend(&router, &id, &amendment).await;
            assert_eq!(
                amended.status, 200,
                "case {case:?} attempt {attempt}: expected HTTP 200, got {} {}",
                amended.status, amended.body
            );
            let after = described(&router, &id).await;
            assert_eq!(
                without_nulls(&after["outboundCallConfig"]),
                without_nulls(&expected),
                "case {case:?} attempt {attempt}: expected one stored config, the one \
                 asked for, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn updating_an_id_that_names_no_campaign_is_refused_and_stores_nothing() {
    let router = router();
    let stored = a_stored_campaign(&router, "untouched-config").await;

    // Rows span `CampaignId` (len 0..256, pattern `^[\S]*$`, from
    // `smithy_query(query="shape", shape="CampaignId")`): the uuid form, the shortest id a
    // path label can carry, punctuation the pattern admits, the longest id the bound
    // admits, the 32-hex form this emulator mints, and an id a delete has since dropped.
    let mut rows: Vec<(&str, String)> = vec![
        (
            "the uuid form",
            "7b1d9f44-1c20-4a6e-8b55-9f0e2d3c4a51".to_string(),
        ),
        ("one character", "q".to_string()),
        ("punctuation the pattern admits", "a:b-D9.e_f".to_string()),
        ("256 characters (the length maximum)", "k".repeat(256)),
        (
            "the minted id form",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
    ];

    let dropped = a_stored_campaign(&router, "to-be-deleted").await;
    let deleted = call(
        &router,
        REGION,
        "DELETE",
        &format!("/campaigns/{dropped}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete a campaign to amend afterwards\": expected HTTP 200, got {} {}",
        deleted.status, deleted.body
    );
    rows.push(("an id a delete has dropped", dropped));

    for (case, id) in rows {
        let refused = amend(
            &router,
            &id,
            &json!({"connectContactFlowId": "flow-nowhere"}),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for an id no campaign carries, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            campaign_not_found_message(&id),
            "case {case:?}: expected this service's campaign-resource not-found wording \
             around the id the request carried, got {}",
            refused.body
        );
    }

    // None of those refusals amended the campaign that does exist.
    let after = described(&router, &stored).await;
    assert_eq!(
        without_nulls(&after["outboundCallConfig"]),
        without_nulls(&created_config()),
        "case \"the stored campaign after every refusal\": expected its config to be \
         untouched, got {after}"
    );
}

#[tokio::test]
async fn a_value_outside_its_published_length_bounds_is_refused_and_the_stored_config_stays() {
    let router = router();
    let id = a_stored_campaign(&router, "keeper-config").await;

    // One row per way an amendment can miss the published bounds of the members it
    // carries, each one past the bound and one far past it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a contact flow id one past its length maximum",
            json!({"connectContactFlowId": "f".repeat(CONTACT_FLOW_MAX + 1)}),
        ),
        (
            "a contact flow id far past its length maximum",
            json!({"connectContactFlowId": "f".repeat(CONTACT_FLOW_MAX * 4)}),
        ),
        (
            "a source phone number one past its length maximum",
            json!({"connectSourcePhoneNumber": "9".repeat(SOURCE_PHONE_MAX + 1)}),
        ),
        (
            "a source phone number far past its length maximum",
            json!({"connectSourcePhoneNumber": "9".repeat(SOURCE_PHONE_MAX * 4)}),
        ),
        (
            "a detection config without its required member",
            json!({"answerMachineDetectionConfig": {"awaitAnswerMachinePrompt": true}}),
        ),
    ];

    for (case, amendment) in rows {
        let refused = amend(&router, &id, &amendment).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a value outside the published \
             constraints, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code
        );
        let after = described(&router, &id).await;
        assert_eq!(
            without_nulls(&after["outboundCallConfig"]),
            without_nulls(&created_config()),
            "case {case:?}: a refused update writes nothing, got {after}"
        );
    }
}
