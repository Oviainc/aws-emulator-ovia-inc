// SPDX-License-Identifier: CC0-1.0

//! THE RULE: a rescore execution plan belongs to the region the request that created it
//! was signed for, and is reachable only through that region — it is listed there, its
//! id and its ARN resolve there, and in every other region it is absent from a listing
//! and refused as a resource that does not exist.
//!
//! The rule follows from the resource being regional: this service has a per-region
//! endpoint (`aws.api#service` gives it the endpoint prefix `kendra-ranking`) and mints
//! an ARN that names the region of the creating request's credential scope, so a plan's
//! identity is scoped to one region. Each suite asserts the rule over a TABLE of region
//! pairs rather than one recorded call, and over EVERY operation of this service that
//! addresses a plan — by `Id` (Describe/Update/Delete/Rescore) or by `ResourceARN`
//! (the tagging operations) — plus the `ClientToken` idempotency that only dedupes
//! within one region.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::scaffolded;

use crate::harness::{call_in, emulator, error_code, sleep_past_the_transition};

/// One row of the region table: a plan's HOME region (the one it is created in) and an
/// AWAY region (any other region a caller can sign for).
struct RegionPair {
    case: &'static str,
    home: &'static str,
    away: &'static str,
}

/// The region pairs every suite here runs. The rows differ along which side of the pair
/// is the emulator's default region (`us-east-1`, the region a request carrying no
/// credential scope is served in), pairs where neither side is the default, and a
/// region name from another AWS partition — so the rule is asserted well beyond the one
/// pair an issue can cite.
fn region_pairs() -> Vec<RegionPair> {
    vec![
        RegionPair {
            case: "the default region holds the plan",
            home: "us-east-1",
            away: "eu-west-1",
        },
        RegionPair {
            case: "the default region is the one that must not see it",
            home: "eu-west-1",
            away: "us-east-1",
        },
        RegionPair {
            case: "neither region is the default",
            home: "ap-southeast-2",
            away: "eu-central-1",
        },
        RegionPair {
            case: "a region of another partition",
            home: "us-gov-west-1",
            away: "us-west-2",
        },
    ]
}

/// Create a plan named `name` in `region`, optionally under `client_token`, and answer
/// its `(Id, Arn)`.
async fn create_plan_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    client_token: Option<&str>,
) -> (String, String) {
    let mut request = json!({ "Name": name });
    if let Some(token) = client_token {
        request["ClientToken"] = json!(token);
    }
    let (status, body) = call_in(router, region, "CreateRescoreExecutionPlan", request).await;
    assert_eq!(
        status, 200,
        "create {name} in {region}: unexpected answer {body}"
    );
    (
        body["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("create {name} in {region}: no Id in {body}"))
            .to_string(),
        body["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("create {name} in {region}: no Arn in {body}"))
            .to_string(),
    )
}

/// The plan ids `ListRescoreExecutionPlans` reports in `region`.
async fn listed_ids(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, listed) = call_in(router, region, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list in {region}: unexpected answer {listed}");
    listed["SummaryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every operation that addresses a plan by its `Id`, with the request body naming
/// `plan`. All of them answer the same not-found error for a plan of another region.
fn id_addressed_calls(plan: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "DescribeRescoreExecutionPlan",
            json!({ "Id": plan }),
        ),
        (
            "UpdateRescoreExecutionPlan",
            json!({ "Id": plan, "Name": "renamed-from-away" }),
        ),
        (
            "DeleteRescoreExecutionPlan",
            json!({ "Id": plan }),
        ),
        (
            "Rescore",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "anything",
                "Documents": [{ "Id": "d1", "OriginalScore": 0.5, "Body": "a note" }],
            }),
        ),
    ]
}

/// Every operation that addresses a plan by its `ResourceARN`, with the request body
/// naming `arn`. All of them answer the same unavailable-resource error for a plan of
/// another region.
fn arn_addressed_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "from", "Value": "away" }] }),
        ),
        (
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": ["team"] }),
        ),
        ("ListTagsForResource", json!({ "ResourceARN": arn })),
    ]
}

#[tokio::test]
async fn a_plan_is_listed_only_in_the_region_it_was_created_in() {
    for pair in region_pairs() {
        let router = emulator();
        let case = pair.case;

        // Before anything is created, every region is empty — including the plan's
        // home, so the emptiness asserted below is not a property of one region.
        for region in [pair.home, pair.away] {
            let ids = listed_ids(&router, region).await;
            assert!(
                ids.is_empty(),
                "case {case:?}: a region with no plan must list none, {region} listed {ids:?}"
            );
        }

        let (home_id, _) = create_plan_in(&router, pair.home, "home-plan", None).await;
        let away_ids = listed_ids(&router, pair.away).await;
        assert!(
            away_ids.is_empty(),
            "case {case:?}: a plan of {} must not be listed in {}, got {away_ids:?}",
            pair.home,
            pair.away
        );
        let home_ids = listed_ids(&router, pair.home).await;
        assert_eq!(
            home_ids,
            vec![home_id.clone()],
            "case {case:?}: {} must list exactly its own plan, got {home_ids:?}",
            pair.home
        );

        // With a plan in BOTH regions, each listing reports its own and only its own —
        // so the away listing above was empty for want of a plan there, not because
        // nothing was ever created.
        let (away_id, _) = create_plan_in(&router, pair.away, "away-plan", None).await;
        let home_ids = listed_ids(&router, pair.home).await;
        assert_eq!(
            home_ids,
            vec![home_id.clone()],
            "case {case:?}: {} must keep listing only its own plan, got {home_ids:?}",
            pair.home
        );
        let away_ids = listed_ids(&router, pair.away).await;
        assert_eq!(
            away_ids,
            vec![away_id.clone()],
            "case {case:?}: {} must list only its own plan, got {away_ids:?}",
            pair.away
        );
        assert_ne!(
            home_id, away_id,
            "case {case:?}: two plans of two regions must not share an id"
        );
    }
}

#[tokio::test]
async fn an_id_addressed_call_in_another_region_is_refused_and_changes_nothing() {
    for pair in region_pairs() {
        let router = emulator();
        let case = pair.case;
        let (home_id, _) = create_plan_in(&router, pair.home, "home-plan", None).await;
        // Past the transient CREATING span, so a refusal below cannot be a conflict
        // about the plan's status rather than the region scoping under test.
        sleep_past_the_transition();

        for (operation, body) in id_addressed_calls(&home_id) {
            let (status, answer) = call_in(&router, pair.away, operation, body).await;
            assert_eq!(
                status,
                scaffolded::CLIENT_ERROR_STATUS,
                "case {case:?}: {operation} in {} must be refused, got {status} {answer}",
                pair.away
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: {operation} in {} must report the plan as not found, got {answer}",
                pair.away
            );
        }

        // The refused calls changed nothing: the plan is still in its home region, with
        // the name it was created under, and is still listed there.
        let (status, described) = call_in(
            &router,
            pair.home,
            "DescribeRescoreExecutionPlan",
            json!({ "Id": home_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the plan must still be readable in {}, got {status} {described}",
            pair.home
        );
        assert_eq!(
            described["Name"],
            json!("home-plan"),
            "case {case:?}: a refused cross-region update must not rename the plan, got {described}"
        );
        let home_ids = listed_ids(&router, pair.home).await;
        assert_eq!(
            home_ids,
            vec![home_id.clone()],
            "case {case:?}: a refused cross-region delete must not drop the plan, got {home_ids:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_addressed_call_in_another_region_is_refused_and_changes_nothing() {
    for pair in region_pairs() {
        let router = emulator();
        let case = pair.case;
        let (_, home_arn) = create_plan_in(&router, pair.home, "home-plan", None).await;

        // One tag to start from, so a refused untag below has something it could have
        // removed and a refused tag something it could have added to.
        let (status, answer) = call_in(
            &router,
            pair.home,
            "TagResource",
            json!({ "ResourceARN": home_arn, "Tags": [{ "Key": "team", "Value": "search" }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: tagging in the plan's own region: got {status} {answer}"
        );

        for (operation, body) in arn_addressed_calls(&home_arn) {
            let (status, answer) = call_in(&router, pair.away, operation, body).await;
            assert_eq!(
                status,
                scaffolded::CLIENT_ERROR_STATUS,
                "case {case:?}: {operation} in {} must be refused, got {status} {answer}",
                pair.away
            );
            assert_eq!(
                error_code(&answer),
                "ResourceUnavailableException",
                "case {case:?}: {operation} in {} must report the ARN as unavailable, got {answer}",
                pair.away
            );
        }

        // The refused calls wrote nothing: the plan still carries exactly the tag its
        // own region gave it.
        let (status, listed) = call_in(
            &router,
            pair.home,
            "ListTagsForResource",
            json!({ "ResourceARN": home_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading tags in the plan's own region: got {status} {listed}"
        );
        assert_eq!(
            listed["Tags"],
            json!([{ "Key": "team", "Value": "search" }]),
            "case {case:?}: a refused cross-region tagging call must not touch the tags, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_client_token_dedupes_only_within_the_region_it_was_used_in() {
    // "Multiple calls to the CreateRescoreExecutionPlanRequest API with the same client
    // token will create only one rescore execution plan" — one plan in THAT region, so
    // the same token in another region mints that region's own plan.
    for pair in region_pairs() {
        let router = emulator();
        let case = pair.case;
        let token = "shared-token";

        let (home_id, home_arn) =
            create_plan_in(&router, pair.home, "home-plan", Some(token)).await;
        let (repeat_id, repeat_arn) =
            create_plan_in(&router, pair.home, "home-plan", Some(token)).await;
        assert_eq!(
            (repeat_id.as_str(), repeat_arn.as_str()),
            (home_id.as_str(), home_arn.as_str()),
            "case {case:?}: a repeated token in one region must answer with that region's plan"
        );

        let (away_id, away_arn) =
            create_plan_in(&router, pair.away, "away-plan", Some(token)).await;
        assert_ne!(
            away_id, home_id,
            "case {case:?}: the same token in {} must mint its own plan",
            pair.away
        );
        assert_eq!(
            away_arn,
            format!(
                "arn:aws:kendra-ranking:{}:{}:rescore-execution-plan/{away_id}",
                pair.away,
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case:?}: the second plan's ARN must name the region that minted it"
        );

        // Two plans now exist, one per region, each visible only in its own.
        let home_ids = listed_ids(&router, pair.home).await;
        assert_eq!(
            home_ids,
            vec![home_id.clone()],
            "case {case:?}: {} must hold exactly its own plan, got {home_ids:?}",
            pair.home
        );
        let away_ids = listed_ids(&router, pair.away).await;
        assert_eq!(
            away_ids,
            vec![away_id.clone()],
            "case {case:?}: {} must hold exactly its own plan, got {away_ids:?}",
            pair.away
        );
    }
}

#[tokio::test]
async fn a_deleted_plan_frees_only_its_own_region() {
    for pair in region_pairs() {
        let router = emulator();
        let case = pair.case;
        let (home_id, _) = create_plan_in(&router, pair.home, "home-plan", None).await;
        let (away_id, _) = create_plan_in(&router, pair.away, "away-plan", None).await;
        // A plan can only be deleted once the clock has made it ACTIVE.
        sleep_past_the_transition();

        let (status, answer) = call_in(
            &router,
            pair.home,
            "DeleteRescoreExecutionPlan",
            json!({ "Id": home_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting in the plan's own region: got {status} {answer}"
        );

        let home_ids = listed_ids(&router, pair.home).await;
        assert!(
            home_ids.is_empty(),
            "case {case:?}: the deleted plan's region must now list none, got {home_ids:?}"
        );
        let away_ids = listed_ids(&router, pair.away).await;
        assert_eq!(
            away_ids,
            vec![away_id.clone()],
            "case {case:?}: a delete in {} must leave {} untouched, got {away_ids:?}",
            pair.home,
            pair.away
        );
        let (status, described) = call_in(
            &router,
            pair.away,
            "DescribeRescoreExecutionPlan",
            json!({ "Id": away_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the other region's plan must still be readable, got {status} {described}"
        );
    }
}
