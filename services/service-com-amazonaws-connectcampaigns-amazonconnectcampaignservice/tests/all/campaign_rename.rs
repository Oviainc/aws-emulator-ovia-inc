// SPDX-License-Identifier: CC0-1.0

//! Renaming a campaign, end to end over HTTP.
//!
//! One rule, asserted over a table whose rows differ along the NAME asked for rather
//! than along the one request that exposed the gap: `UpdateCampaignName` replaces the
//! stored name of the campaign the path addresses with the name the request carries and
//! writes nothing else, so the new name is what every later read of that campaign
//! reports while its id, ARN, Connect instance, configs, tags, run state and place in
//! the creation order stay as they were. Asking for the name a campaign already carries
//! is the no-op the operation's declared idempotence makes it, and an id naming no
//! stored campaign is refused in the words this service answers a campaign-resource
//! lookup that resolves to nothing with, storing nothing.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::campaign_not_found_message;

use super::common::{call, router, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance every campaign below is created for.
const INSTANCE: &str = "rename-instance";

/// The contact flow the fixture campaign dials through, so a case can check the
/// outbound-call config a rename must leave alone.
const CONTACT_FLOW: &str = "flow-rename-1";

/// `CampaignName`'s published length bounds (`@length(min 1, max 127)`, from
/// `smithy_query(query="shape", shape="CampaignName")`): a name outside them is refused
/// at the boundary, and a name at either bound is accepted.
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 127;

/// A `CreateCampaign` body for `INSTANCE`: every member the model marks required, plus
/// the tags and configs a rename must leave untouched.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": INSTANCE,
        "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.25}},
        "outboundCallConfig": {"connectContactFlowId": CONTACT_FLOW},
        "tags": {"owner": "rename-suite"},
    })
}

/// Register `INSTANCE` with the account and create one campaign named `name`, handing
/// back the minted campaign id.
async fn a_stored_campaign(router: &Harness, name: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case \"create the campaign to rename\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    created.body["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"create the campaign to rename\": no id in {}",
                created.body
            )
        })
        .to_string()
}

/// Ask for `name` on the campaign `id` names.
async fn rename(router: &Harness, id: &str, name: &str) -> super::common::Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/campaigns/{id}/name"),
        &json!({"name": name}),
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

/// The `name` member of a described campaign, or `"<absent>"`.
fn name_of(campaign: &Value) -> &str {
    campaign.get("name").and_then(Value::as_str).unwrap_or("<absent>")
}

/// Names a rename may ask for, spanning `CampaignName` (len 1..127): the shortest name
/// the minimum admits, an ordinary one, one carrying the spaces and punctuation the
/// unconstrained shape allows, a non-ASCII one, and the longest name the maximum admits.
fn accepted_names() -> Vec<(&'static str, String)> {
    vec![
        (
            "one character (the length minimum)",
            "x".repeat(NAME_MIN),
        ),
        ("an ordinary name", "Autumn outreach".to_string()),
        (
            "spaces and punctuation the shape allows",
            "Q4 / west — wave #2 (retry)".to_string(),
        ),
        ("a non-ASCII name", "campagne été".to_string()),
        (
            "127 characters (the length maximum)",
            "n".repeat(NAME_MAX),
        ),
    ]
}

#[tokio::test]
async fn a_rename_replaces_the_stored_name_and_leaves_the_campaign_otherwise_alone() {
    let router = router();
    let id = a_stored_campaign(&router, "original").await;
    let before = described(&router, &id).await;

    // The campaign is put into a run state first, so a rename is shown to leave the run
    // state the lifecycle operations own exactly where it was.
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
        "case \"start the campaign before renaming it\": expected HTTP 200, got {} {}",
        started.status, started.body
    );

    for (case, name) in accepted_names() {
        let renamed = rename(&router, &id, &name).await;
        assert_eq!(
            renamed.status, 200,
            "case {case:?}: expected HTTP 200 from the rename, got {} {}",
            renamed.status, renamed.body
        );

        let after = described(&router, &id).await;
        assert_eq!(
            name_of(&after),
            name,
            "case {case:?}: expected the campaign to read back under the name asked for, \
             got {after}"
        );

        // Every other member is the one the create stored, so only the name moved.
        for member in [
            "id",
            "arn",
            "connectInstanceId",
            "dialerConfig",
            "outboundCallConfig",
            "tags",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: expected {member} to survive the rename unchanged, got {after}"
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
            "case {case:?}: expected the run state to survive the rename, got {} {}",
            state.status,
            state.body
        );

        // And the list plane reports the new name for the same campaign, so no reader is
        // left with the old one.
        let listed = call(
            &router,
            REGION,
            "POST",
            "/campaigns-summary",
            &json!({"maxResults": 50}),
        )
        .await;
        let summaries = listed.body["campaignSummaryList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let summary = summaries
            .iter()
            .find(|summary| summary.get("id").and_then(Value::as_str) == Some(id.as_str()));
        assert_eq!(
            summary.map(name_of),
            Some(name.as_str()),
            "case {case:?}: expected the summary of that campaign to carry the new name, \
             got {}",
            listed.body
        );
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: a rename mints and drops no campaign, got {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn asking_for_the_name_a_campaign_already_carries_succeeds_and_changes_nothing() {
    let router = router();
    let id = a_stored_campaign(&router, "steady").await;

    // Rows differ along how a repeat is reached: straight after the create, and twice
    // more after a rename that did move the name.
    let rows: Vec<(&str, String)> = vec![
        ("the name the create stored", "steady".to_string()),
        ("a name a rename moved it to", "moved".to_string()),
        (
            "the longest name, repeated",
            "r".repeat(NAME_MAX),
        ),
    ];

    for (case, name) in rows {
        for attempt in 1..=3 {
            let renamed = rename(&router, &id, &name).await;
            assert_eq!(
                renamed.status, 200,
                "case {case:?} attempt {attempt}: expected HTTP 200, got {} {}",
                renamed.status, renamed.body
            );
            let after = described(&router, &id).await;
            assert_eq!(
                name_of(&after),
                name,
                "case {case:?} attempt {attempt}: expected the one stored name to be the \
                 one asked for, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn renaming_an_id_that_names_no_campaign_is_refused_and_stores_nothing() {
    let router = router();
    let stored = a_stored_campaign(&router, "untouched").await;

    // Rows span `CampaignId` (len 0..256, pattern `^[\S]*$`, from
    // `smithy_query(query="shape", shape="CampaignId")`): the uuid form, the shortest id
    // a path label can carry, punctuation the pattern admits, the longest id the bound
    // admits, the 32-hex form this emulator mints, and an id a delete has since dropped.
    let mut rows: Vec<(&str, String)> = vec![
        (
            "the uuid form",
            "2f9c7b41-6d05-4e38-9a21-0b7c5d4e3f20".to_string(),
        ),
        ("one character", "q".to_string()),
        ("punctuation the pattern admits", "a:b-D9.e_f".to_string()),
        ("256 characters (the length maximum)", "k".repeat(256)),
        (
            "the minted id form",
            "fedcba9876543210fedcba9876543210".to_string(),
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
        "case \"delete a campaign to rename afterwards\": expected HTTP 200, got {} {}",
        deleted.status, deleted.body
    );
    rows.push(("an id a delete has dropped", dropped));

    for (case, id) in rows {
        let refused = rename(&router, &id, "a new name").await;
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

    // None of those refusals renamed the campaign that does exist.
    let after = described(&router, &stored).await;
    assert_eq!(
        name_of(&after),
        "untouched",
        "case \"the stored campaign after every refusal\": expected its name to be \
         untouched, got {after}"
    );
}

#[tokio::test]
async fn a_name_outside_its_published_length_bounds_is_refused_and_the_stored_name_stays() {
    let router = router();
    let id = a_stored_campaign(&router, "keeper").await;

    // One row per way a name can miss the published bounds, plus the absent member.
    let rows: Vec<(&str, Value)> = vec![
        ("a name below the length minimum", json!({"name": ""})),
        (
            "a name one past the length maximum",
            json!({"name": "y".repeat(NAME_MAX + 1)}),
        ),
        (
            "a name far past the length maximum",
            json!({"name": "y".repeat(NAME_MAX * 4)}),
        ),
        ("no name member at all", json!({})),
    ];

    for (case, body) in rows {
        let refused = call(
            &router,
            REGION,
            "POST",
            &format!("/campaigns/{id}/name"),
            &body,
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for a name outside its published bounds, \
             got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_code
        );
        let after = described(&router, &id).await;
        assert_eq!(
            name_of(&after),
            "keeper",
            "case {case:?}: a refused rename writes nothing, got {after}"
        );
    }
}
