// SPDX-License-Identifier: CC0-1.0

//! A campaign name identifies a campaign within its Connect instance, end to end
//! over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * a `CreateCampaign` naming a `(connectInstanceId, name)` pair the store already
//!   holds mints NOTHING and is refused with the modelled `ConflictException`
//!   (HTTP 409) — afterwards that instance holds exactly one campaign of that name,
//!   and it is the record the first create wrote;
//! * the name is scoped to the instance: the same name under a different Connect
//!   instance is a different campaign and is created;
//! * the comparison is exact — a name differing in case, in whitespace or in any
//!   character is a different name, since the model puts no normalization on
//!   `CampaignName`;
//! * the refusal is about the LIVE campaign, not a ledger of used names: deleting a
//!   campaign frees its name for a new one;
//! * `CampaignName`'s modelled `@length` 1..127 still decides what a name may be at
//!   all, and a value outside it is HTTP 400 `ValidationException` before any
//!   uniqueness question.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ONBOARDING_JOB_DURATION;
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

/// A `CreateCampaign` body for `instance` under `name`.
fn create_body(instance: &str, name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_config(),
    })
}

/// `CreateCampaign`, returning its status and body.
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
        &create_body(instance, name),
    )
    .await
}

/// The campaign summaries `instance` holds, in creation order.
async fn summaries(router: &MultiServiceRouter, region: &str, instance: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        region,
        "POST",
        "/v2/campaigns-summary",
        &json!({"filters": {"instanceIdFilter": {"value": instance, "operator": "Eq"}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance:?}: ListCampaigns expected 200, got {status} {body}"
    );
    body["campaignSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A repeat create of a name an instance already holds mints nothing and is refused
/// with the modelled `ConflictException` (HTTP 409), over a table of names spanning
/// what `CampaignName` admits.
///
/// Name catalog source: `CreateCampaign$name` targets `CampaignName`, `@length`
/// 1..127 with no pattern and no modelled normalization — see
/// `smithy_query(query="operation",operation="CreateCampaign")`. The rows differ
/// along length (both published bounds), character class and whitespace; none of
/// them is the name the issue cited.
#[tokio::test]
async fn a_repeated_name_under_one_instance_is_refused_as_a_conflict() {
    let region = "us-east-1";
    let router = router();
    let instance = "uniqueness-1";
    onboard(&router, region, instance).await;

    // (case, name)
    let names: &[(&str, String)] = &[
        ("shortest admissible name", "a".to_string()),
        ("longest admissible name", "n".repeat(127)),
        ("spaces and punctuation", "Spring Sale (2026) — wave #1".to_string()),
        ("non-ascii name", "campagne-été".to_string()),
        ("digits only", "4242".to_string()),
    ];

    for (case, name) in names {
        let (status, first) = create(&router, region, instance, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: first CreateCampaign expected 200, got {status} {first}"
        );
        let id = first["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateCampaign returned no id: {first}"))
            .to_string();
        let arn = first["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateCampaign returned no arn: {first}"))
            .to_string();

        // The repeat: same instance, same name.
        let (status, second) = create(&router, region, instance, name).await;
        assert_eq!(
            status, 409,
            "case {case:?}: repeated CreateCampaign expected HTTP 409, got {status} {second}"
        );
        assert!(
            error_code(&second).ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {} ({second})",
            error_code(&second)
        );
        assert_ne!(
            second["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: a refusal carries no campaign id, got {second}"
        );
        // Nothing was minted: the name still resolves to the first campaign, with
        // the id and ARN that create returned.
        let held: Vec<Value> = summaries(&router, region, instance)
            .await
            .into_iter()
            .filter(|summary| summary["name"].as_str() == Some(name.as_str()))
            .collect();
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected exactly one campaign named {name:?}, got {held:?}"
        );
        assert_eq!(
            held[0]["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the first create's id {id:?}, got {:?}",
            held[0]["id"]
        );
        assert_eq!(
            held[0]["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the first create's arn {arn:?}, got {:?}",
            held[0]["arn"]
        );

        // And the first campaign is still readable under its own id, unchanged.
        let (status, described) =
            call(&router, region, "GET", &format!("/v2/campaigns/{id}"), &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeCampaign expected 200, got {status} {described}"
        );
        assert_eq!(
            described["campaign"]["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the stored name {name:?}, got {described}"
        );
    }
}

/// Two names that merely LOOK alike are two campaigns: the comparison is exact, so a
/// difference in case, in whitespace or in a single character makes a new campaign
/// rather than a conflict.
#[tokio::test]
async fn names_differing_in_any_character_are_distinct_campaigns() {
    let region = "eu-west-1";
    let router = router();
    let instance = "uniqueness-2";
    onboard(&router, region, instance).await;

    // (case, first name, the near-miss that must still be created)
    let pairs: &[(&str, &str, &str)] = &[
        ("case differs", "Autumn-Wave", "autumn-wave"),
        ("trailing space", "winter-wave", "winter-wave "),
        ("leading space", "spring-wave", " spring-wave"),
        ("inner whitespace", "summer wave", "summer  wave"),
        ("one character longer", "wave", "wave2"),
    ];

    let mut seen: Vec<String> = Vec::new();
    for (case, first_name, near_miss) in pairs {
        for name in [first_name, near_miss] {
            let (status, body) = create(&router, region, instance, name).await;
            assert_eq!(
                status, 200,
                "case {case:?} name {name:?}: expected HTTP 200, got {status} {body}"
            );
            let id = body["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no id in {body}"))
                .to_string();
            assert!(
                !seen.contains(&id),
                "case {case:?} name {name:?}: expected a fresh id, got the already-minted {id:?}"
            );
            seen.push(id);
        }
    }

    let held = summaries(&router, region, instance).await.len();
    assert_eq!(
        held,
        pairs.len() * 2,
        "case \"near-miss names\": expected {} campaigns, got {held}",
        pairs.len() * 2
    );
}

/// The name is scoped to its Connect instance: the same name under each of several
/// instances is several campaigns, and each instance holds exactly one of them.
#[tokio::test]
async fn one_name_is_held_once_per_instance() {
    let region = "ap-southeast-2";
    let router = router();
    let instances = ["shared-a", "shared.b", "shared_c"];
    let name = "quarterly-outreach";

    let mut ids: Vec<String> = Vec::new();
    for instance in instances {
        onboard(&router, region, instance).await;
        let (status, body) = create(&router, region, instance, name).await;
        assert_eq!(
            status, 200,
            "case {instance:?}: expected HTTP 200, got {status} {body}"
        );
        let id = body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {instance:?}: no id in {body}"))
            .to_string();
        assert!(
            !ids.contains(&id),
            "case {instance:?}: expected a campaign of its own, got the already-minted {id:?}"
        );
        ids.push(id);
    }

    for instance in instances {
        // The repeat is a conflict for THIS instance only.
        let (status, body) = create(&router, region, instance, name).await;
        assert_eq!(
            status, 409,
            "case {instance:?}: repeated create expected HTTP 409, got {status} {body}"
        );
        let held = summaries(&router, region, instance).await;
        assert_eq!(
            held.len(),
            1,
            "case {instance:?}: expected exactly one campaign, got {held:?}"
        );
        assert_eq!(
            held[0]["name"].as_str(),
            Some(name),
            "case {instance:?}: expected the campaign named {name:?}, got {:?}",
            held[0]["name"]
        );
    }
}

/// The conflict is about the campaign that EXISTS, not about a name having once been
/// used: deleting a campaign frees its name, and the create that follows mints a new
/// campaign with a new id.
#[tokio::test]
async fn deleting_a_campaign_frees_its_name() {
    let region = "us-west-2";
    let router = router();
    let instance = "uniqueness-3";
    onboard(&router, region, instance).await;

    let name = "recycled";
    let mut previous = String::new();
    for round in 0..3 {
        let case = format!("round {round}");
        let (status, body) = create(&router, region, instance, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let id = body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {body}"))
            .to_string();
        assert_ne!(
            id, previous,
            "case {case:?}: expected a freshly minted id, got the deleted campaign's {id:?}"
        );

        // While it is there the name is taken.
        let (status, refused) = create(&router, region, instance, name).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the held name to conflict, got {status} {refused}"
        );

        let (status, deleted) = call(
            &router,
            region,
            "DELETE",
            &format!("/v2/campaigns/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCampaign expected 200, got {status} {deleted}"
        );
        previous = id;
    }

    let held = summaries(&router, region, instance).await;
    assert!(
        held.is_empty(),
        "case \"after the last delete\": expected no campaigns, got {held:?}"
    );
}

/// `CampaignName`'s modelled `@length` 1..127 decides what a name may be at all, and
/// a value outside it is refused as a bad request before any uniqueness question —
/// both published bounds, and one past each.
///
/// Source: `CreateCampaign$name` targets `CampaignName`, `@length` min 1 max 127
/// (`smithy_query(query="operation",operation="CreateCampaign")`).
#[tokio::test]
async fn a_name_outside_the_modelled_length_is_a_bad_request() {
    let region = "us-east-1";
    let router = router();
    let instance = "uniqueness-4";
    onboard(&router, region, instance).await;

    // (case, name, expected status)
    let cases: &[(&str, String, u16)] = &[
        ("empty, one below the minimum", String::new(), 400),
        ("at the minimum", "m".to_string(), 200),
        ("at the maximum", "m".repeat(127), 200),
        ("one past the maximum", "m".repeat(128), 400),
    ];

    for (case, name, expected) in cases {
        let (status, body) = create(&router, region, instance, name).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 400 {
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} ({body})",
                error_code(&body)
            );
        }
    }

    // Only the two admissible names were stored.
    let held = summaries(&router, region, instance).await.len();
    assert_eq!(
        held, 2,
        "case \"after the length table\": expected 2 campaigns, got {held}"
    );
}
