// SPDX-License-Identifier: CC0-1.0

//! `UpdateCampaignName`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * the call RENAMES the campaign `{id}` names to the `name` it was given, and the
//!   stored name is what every later read of the campaign (`DescribeCampaign`,
//!   `ListCampaigns`) projects — nothing else about the campaign changes;
//! * the operation is idempotent (`@idempotent` in the model, "This API is
//!   idempotent" in its documentation): repeating it with the same name leaves the
//!   same name;
//! * a campaign name identifies a campaign within its Connect instance, so a rename
//!   onto a name another campaign of that instance already holds is HTTP 409
//!   `ConflictException` and renames nothing, while the same name under a different
//!   instance — or a name differing in any single character — is accepted;
//! * a rename frees the name the campaign carried before it;
//! * an id this region's store does not hold is HTTP 404 `ResourceNotFoundException`
//!   and renames nothing;
//! * a `name` outside `CampaignName`'s modelled `@length` 1..127 is HTTP 400
//!   `ValidationException` and leaves the stored name alone;
//! * the operation models no `InvalidCampaignStateException`, so a campaign takes a
//!   rename in every lifecycle state its lifecycle can reach.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, CAMPAIGN_NOT_FOUND_MESSAGE, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

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
/// instance is a campaign target by the time this returns: `CreateCampaign` mints a
/// campaign only for an instance whose job has SUCCEEDED.
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

/// `CreateCampaign` for `instance` under `name`, returning its status and body.
async fn create(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await
}

/// Create a campaign on an onboarded `instance` and hand back its id and ARN.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
) -> (String, String) {
    let (status, response) = create(router, region, instance, name).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCampaign expected 200, got {status} {response}"
    );
    let id = response["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCampaign returned no id: {response}"))
        .to_string();
    let arn = response["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCampaign returned no arn: {response}"))
        .to_string();
    (id, arn)
}

/// `UpdateCampaignName` for `id` with `body`.
async fn rename_with(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/name"),
        body,
    )
    .await
}

/// `UpdateCampaignName` for `id` with the name `name`.
async fn rename(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    name: &str,
) -> (u16, Value) {
    rename_with(router, region, id, &json!({"name": name})).await
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

/// The `name` the `ListCampaigns` summary of `id` carries, or `None` when the listing
/// holds no summary for it.
async fn summary_name(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    case: &str,
) -> Option<String> {
    let (status, body) = call(router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCampaigns expected 200, got {status} {body}"
    );
    body["campaignSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListCampaigns returned no list: {body}"))
        .iter()
        .find(|summary| summary["id"].as_str() == Some(id))
        .and_then(|summary| summary["name"].as_str())
        .map(str::to_string)
}

/// The error message a failure response carries.
fn error_message(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A rename writes the store, every reader projects the new name, and nothing else
/// about the campaign moves — over a table of names spanning what `CampaignName`
/// admits. Repeating a rename with the same name is the same write.
///
/// Name catalog source: `UpdateCampaignName$name` targets `CampaignName`, `@length`
/// 1..127 with no pattern and no modelled normalization — see
/// `smithy_query(query="operation",operation="UpdateCampaignName")`. The rows differ
/// along length (both published bounds), character class and whitespace.
#[tokio::test]
async fn a_rename_is_what_every_reader_projects() {
    let region = "us-east-1";
    let router = router();
    let instance = "rename-1";
    onboard(&router, region, instance).await;
    let (id, arn) = create_campaign(&router, region, instance, "original").await;

    // (case, new name)
    let names: &[(&str, String)] = &[
        ("shortest admissible name", "a".to_string()),
        ("longest admissible name", "n".repeat(127)),
        (
            "spaces and punctuation",
            "Spring Sale (2026) — wave #1".to_string(),
        ),
        ("non-ascii name", "campagne-été".to_string()),
        ("digits only", "4242".to_string()),
    ];

    for (case, name) in names {
        let (status, body) = rename(&router, region, &id, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateCampaignName expected 200, got {status} {body}"
        );

        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected DescribeCampaign to report {name:?}, got {:?}",
            campaign["name"]
        );
        // Only the name moved: the identity and the configuration the create wrote
        // are still the campaign's own.
        assert_eq!(
            campaign["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the id {id:?} to be unchanged, got {:?}",
            campaign["id"]
        );
        assert_eq!(
            campaign["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the arn {arn:?} to be unchanged, got {:?}",
            campaign["arn"]
        );
        assert_eq!(
            campaign["connectInstanceId"].as_str(),
            Some(instance),
            "case {case:?}: expected the instance {instance:?} to be unchanged, got {:?}",
            campaign["connectInstanceId"]
        );
        assert_eq!(
            campaign["channelSubtypeConfig"], telephony_config(),
            "case {case:?}: expected the channel subtype config to be unchanged, got {:?}",
            campaign["channelSubtypeConfig"]
        );

        let listed = summary_name(&router, region, &id, case).await;
        assert_eq!(
            listed.as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected ListCampaigns to report {name:?}, got {listed:?}"
        );

        // Idempotent: the same rename again is the same write.
        let (status, body) = rename(&router, region, &id, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeated rename expected 200, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the repeated rename to leave {name:?}, got {:?}",
            campaign["name"]
        );
    }
}

/// A rename onto a name another campaign of the same Connect instance already holds
/// is refused as a conflict and renames nothing — while a name differing in any
/// single character is a different name and is accepted.
#[tokio::test]
async fn renaming_onto_a_held_name_is_a_conflict() {
    let region = "eu-west-1";
    let router = router();
    let instance = "rename-2";
    onboard(&router, region, instance).await;

    // (case, the name the other campaign holds, the near-miss that must be accepted)
    let cases: &[(&str, &str, &str)] = &[
        ("case differs", "Autumn-Wave", "autumn-wave"),
        ("trailing space", "winter-wave", "winter-wave "),
        ("leading space", "spring-wave", " spring-wave"),
        ("inner whitespace", "summer wave", "summer  wave"),
        ("one character longer", "wave", "wave2"),
    ];

    for (index, (case, held, near_miss)) in cases.iter().enumerate() {
        let (_, _) = create_campaign(&router, region, instance, held).await;
        let mine = format!("mine-{index}");
        let (id, _) = create_campaign(&router, region, instance, &mine).await;

        let (status, body) = rename(&router, region, &id, held).await;
        assert_eq!(
            status, 409,
            "case {case:?}: renaming onto the held {held:?} expected HTTP 409, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {} ({body})",
            error_code(&body)
        );
        // The wording itself is a requirement, so it is pinned literally here
        // rather than read back from the helper the handler composes it with: the
        // refusal names the name it collides on and the instance it collides in.
        assert_eq!(
            error_message(&body),
            format!("A campaign named {held} already exists in Connect instance {instance}"),
            "case {case:?}: expected the conflict to name {held:?} and {instance:?}, got {:?}",
            error_message(&body)
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(mine.as_str()),
            "case {case:?}: expected the refusal to leave {mine:?} stored, got {:?}",
            campaign["name"]
        );

        // The near miss is a different name, so it is taken.
        let (status, body) = rename(&router, region, &id, near_miss).await;
        assert_eq!(
            status, 200,
            "case {case:?}: renaming to {near_miss:?} expected HTTP 200, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(*near_miss),
            "case {case:?}: expected {near_miss:?} stored, got {:?}",
            campaign["name"]
        );

        // A campaign is never in conflict with itself: renaming it to the name it
        // already carries is accepted.
        let (status, body) = rename(&router, region, &id, near_miss).await;
        assert_eq!(
            status, 200,
            "case {case:?}: renaming to its own name expected HTTP 200, got {status} {body}"
        );
    }
}

/// The name is scoped to its Connect instance: the same name under each of several
/// instances is several campaigns, so a rename onto a name only ANOTHER instance
/// holds is accepted.
#[tokio::test]
async fn one_name_is_renameable_once_per_instance() {
    let region = "ap-southeast-2";
    let router = router();
    let instances = ["scoped-a", "scoped.b", "scoped_c"];
    let target = "quarterly-outreach";

    let mut ids: Vec<String> = Vec::new();
    for (index, instance) in instances.iter().enumerate() {
        onboard(&router, region, instance).await;
        let (id, _) = create_campaign(&router, region, instance, &format!("draft-{index}")).await;
        ids.push(id);
    }

    for (instance, id) in instances.iter().zip(&ids) {
        let (status, body) = rename(&router, region, id, target).await;
        assert_eq!(
            status, 200,
            "case {instance:?}: expected the rename to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, id, instance).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(target),
            "case {instance:?}: expected {target:?} stored, got {:?}",
            campaign["name"]
        );
    }
}

/// A rename frees the name the campaign carried: the create that follows takes it and
/// mints a campaign of its own.
#[tokio::test]
async fn renaming_frees_the_previous_name() {
    let region = "us-west-2";
    let router = router();
    let instance = "rename-3";
    onboard(&router, region, instance).await;

    let held = "recycled";
    let (id, _) = create_campaign(&router, region, instance, held).await;
    for round in 0..3 {
        let case = format!("round {round}");
        // While the campaign carries the name, a create under it conflicts.
        let (status, refused) = create(&router, region, instance, held).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the held name to conflict, got {status} {refused}"
        );

        let renamed = format!("renamed-{round}");
        let (status, body) = rename(&router, region, &id, &renamed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rename to be accepted, got {status} {body}"
        );

        // The freed name is now available to a new campaign.
        let (fresh_id, _) = create_campaign(&router, region, instance, held).await;
        assert_ne!(
            fresh_id, id,
            "case {case:?}: expected a freshly minted campaign, got the renamed {fresh_id:?}"
        );

        // Put the names back the way the next round expects: the new campaign goes,
        // and the renamed one takes the freed name again.
        let (status, deleted) = call(
            &router,
            region,
            "DELETE",
            &format!("/v2/campaigns/{fresh_id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCampaign expected 200, got {status} {deleted}"
        );
        let (status, body) = rename(&router, region, &id, held).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the freed name to be retaken, got {status} {body}"
        );
    }
}

/// An id this region's store does not hold names no campaign, so the rename is
/// refused as not found and renames nothing — including an id another region holds a
/// campaign under, which is a different resource.
#[tokio::test]
async fn an_unheld_campaign_id_is_not_found() {
    let region = "us-east-1";
    let other_region = "eu-central-1";
    let router = router();
    let instance = "rename-4";
    onboard(&router, region, instance).await;
    let (id, _) = create_campaign(&router, region, instance, "held").await;

    // (case, id) — each within `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its
    // 1..256 length bound, so the refusal is the handler's and not the constraint's.
    let unheld: &[(&str, String)] = &[
        ("a never-minted id", "000000009999".to_string()),
        ("the shortest admissible id", "x".to_string()),
        ("the longest admissible id", "z".repeat(256)),
        ("a colon-separated id", "tenant:absent:0001".to_string()),
    ];

    for (case, absent) in unheld {
        let (status, body) = rename(&router, region, absent, "whatever").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {} ({body})",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            CAMPAIGN_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the shared not-found wording, got {:?}",
            error_message(&body)
        );
    }

    // The campaign of another region is not this endpoint's to rename.
    let (status, body) = rename(&router, other_region, &id, "renamed-elsewhere").await;
    assert_eq!(
        status, 404,
        "case \"another region's campaign\": expected HTTP 404, got {status} {body}"
    );
    let campaign = describe(&router, region, &id, "another region's campaign").await;
    assert_eq!(
        campaign["name"].as_str(),
        Some("held"),
        "case \"another region's campaign\": expected \"held\" stored, got {:?}",
        campaign["name"]
    );
}

/// `CampaignName`'s modelled `@length` 1..127 decides what a name may be at all, and
/// a value outside it — or a request carrying no name — is refused as a bad request,
/// leaving the stored name alone.
///
/// Source: `UpdateCampaignName$name` targets `CampaignName`, `@length` min 1 max 127,
/// and is `@required` (`smithy_query(query="operation",operation="UpdateCampaignName")`).
#[tokio::test]
async fn a_name_outside_the_modelled_length_is_a_bad_request() {
    let region = "ca-central-1";
    let router = router();
    let instance = "rename-5";
    onboard(&router, region, instance).await;
    let (id, _) = create_campaign(&router, region, instance, "before").await;

    // (case, body, expected status, the name stored afterwards)
    let cases: &[(&str, Value, u16, &str)] = &[
        ("empty, one below the minimum", json!({"name": ""}), 400, "before"),
        ("at the minimum", json!({"name": "m"}), 200, "m"),
        ("at the maximum", json!({"name": "m".repeat(127)}), 200, "<127>"),
        ("one past the maximum", json!({"name": "m".repeat(128)}), 400, "<127>"),
        ("no name at all", json!({}), 400, "<127>"),
    ];

    for (case, body, expected, stored) in cases {
        let (status, response) = rename_with(&router, region, &id, body).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if *expected == 400 {
            assert!(
                error_code(&response).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} ({response})",
                error_code(&response)
            );
        }
        let expected_name = if *stored == "<127>" {
            "m".repeat(127)
        } else {
            (*stored).to_string()
        };
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(expected_name.as_str()),
            "case {case:?}: expected {expected_name:?} stored, got {:?}",
            campaign["name"]
        );
    }
}

/// A campaign takes a rename in every lifecycle state its lifecycle can reach: this
/// operation models no `InvalidCampaignStateException`, so the state a campaign is at
/// does not decide whether it can be renamed.
#[tokio::test]
async fn every_reachable_state_takes_a_rename() {
    let region = "ap-south-1";
    let router = router();
    let instance = "rename-6";
    onboard(&router, region, instance).await;
    let (id, _) = create_campaign(&router, region, instance, "stateful").await;

    // Each lifecycle action the campaign state machine offers, with the state it
    // leaves the campaign in.
    let walk: &[(&str, CampaignState)] = &[
        ("", CampaignState::Initialized),
        ("start", CampaignState::Running),
        ("pause", CampaignState::Paused),
        ("stop", CampaignState::Stopped),
    ];

    for (action, state) in walk {
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

        let name = format!("named-in-{}", state.as_wire());
        let (status, body) = rename(&router, region, &id, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rename to be accepted, got {status} {body}"
        );
        let campaign = describe(&router, region, &id, &case).await;
        assert_eq!(
            campaign["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected {name:?} stored, got {:?}",
            campaign["name"]
        );
    }
}
