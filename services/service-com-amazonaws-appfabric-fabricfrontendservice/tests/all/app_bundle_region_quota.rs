// SPDX-License-Identifier: CC0-1.0

//! The app bundle's per-Region quota, driven over the wire (restJson1) through
//! the same router the emulator boots.
//!
//! RULE: an app bundle belongs to the Region its create call was signed for, and
//! an account holds at most
//! `state::MAX_APP_BUNDLES_PER_REGION` of them in a Region, so a `CreateAppBundle`
//! for a Region that already holds that many is refused with the modelled
//! `ServiceQuotaExceededException` (HTTP 402) — leaving the stored bundle
//! untouched — and succeeds again once `DeleteAppBundle` has freed the Region.
//! An idempotent retry of the create that minted the bundle is a REPLAY, not a
//! create, so the quota never refuses it.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `fixture_region`) is
//! the one `app_bundle_ingestion_lifecycle` already builds; only the quota tables
//! are new here.

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::MAX_APP_BUNDLES_PER_REGION;

use crate::app_bundle_ingestion_lifecycle::{
    arn_of, encode, fixture_region, uuid_of, Answer, Emulator,
};

/// The account segment of every ARN a generated emulator emits.
const ACCOUNT: &str = "000000000000";

/// The bodies a create call may carry, as table rows: the rule holds whatever
/// the optional members are, so no row may change the outcome. The members are
/// `CreateAppBundleRequest`'s (`clientToken`, `customerManagedKeyIdentifier`,
/// `tags`).
fn create_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("no optional members", json!({})),
        (
            "a customer-managed key by bare id",
            json!({"customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
        ),
        (
            "a customer-managed key by ARN",
            json!({"customerManagedKeyIdentifier":
                format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/66666666-7777-4888-8999-aaaaaaaaaaaa")}),
        ),
        (
            "two tags",
            json!({"tags": [{"key": "env", "value": "prod"}, {"key": "team", "value": "data"}]}),
        ),
        ("an empty tag list", json!({"tags": []})),
        (
            "a client token",
            json!({"clientToken": "aaaaaaaa-0000-4000-8000-000000000000"}),
        ),
    ]
}

/// `GET /appbundles/{appBundleIdentifier}`.
async fn get_bundle(emulator: &Emulator, region: &str, identifier: &str) -> Answer {
    emulator
        .call_in_region(
            region,
            "GET",
            &format!("/appbundles/{}", encode(identifier)),
            None,
        )
        .await
}

/// `DELETE /appbundles/{appBundleIdentifier}`.
async fn delete_bundle(emulator: &Emulator, region: &str, identifier: &str) -> Answer {
    emulator
        .call_in_region(
            region,
            "DELETE",
            &format!("/appbundles/{}", encode(identifier)),
            None,
        )
        .await
}

/// Every stored bundle's ARN, as `ListAppBundles` reports it.
async fn listed_arns(emulator: &Emulator, region: &str, case: &str) -> Vec<String> {
    let answer = emulator
        .call_in_region(region, "GET", "/appbundles?maxResults=100", None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListAppBundles, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appBundleSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no appBundleSummaryList in {}", answer.body))
        .iter()
        .map(|summary| {
            summary
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a summary without an arn in {}", answer.body))
                .to_string()
        })
        .collect()
}

/// Assert an answer is the modelled quota refusal: HTTP 402, the modelled error
/// code, and the members the shape carries filled in from the request's own
/// service and resource kind. The message itself is not asserted — AWS documents
/// no sentence for it.
fn assert_quota_refusal(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 402,
        "case {case:?}: expected HTTP 402, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {}",
        answer.body
    );
    for (member, expected) in [("resourceType", "AppBundle"), ("serviceCode", "appfabric")] {
        assert_eq!(
            answer.body.get(member).and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected {member} {expected:?}, got {}",
            answer.body
        );
    }
    for member in ["resourceId", "quotaCode"] {
        assert!(
            answer
                .body
                .get(member)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected a non-empty {member}, got {}",
            answer.body
        );
    }
}

/// RULE: a Region holds at most `MAX_APP_BUNDLES_PER_REGION` app bundles for the
/// account, so the create that would take it past that is refused with the
/// modelled `ServiceQuotaExceededException` (HTTP 402) and persists nothing.
///
/// The table spans the optional members of both calls and the Region they are
/// signed for, because neither may change the outcome.
#[tokio::test]
async fn a_second_create_in_one_region_is_refused_with_the_quota_error() {
    let bodies = create_bodies();
    for (index, (case, second_body)) in bodies.iter().enumerate() {
        // Each row signs for a different Region, and the FIRST bundle carries a
        // body from the table too (the next row's), so no row depends on one
        // request shape.
        let region = fixture_region(index);
        let (first_case, first_body) = &bodies[(index + 1) % bodies.len()];
        let case = format!("{first_case:?} then {case:?} in {region}");
        let emulator = Emulator::new();

        let mut held: Vec<String> = Vec::new();
        for attempt in 0..MAX_APP_BUNDLES_PER_REGION {
            let created = emulator
                .create_app_bundle_in_region(&region, first_body.clone())
                .await;
            assert_eq!(
                created.status, 201,
                "case {case:?} bundle {attempt}: expected HTTP 201 below the quota, got {} with body {}",
                created.status, created.body
            );
            let arn = arn_of(&created.body, "appBundle");
            assert_eq!(
                arn,
                format!(
                    "arn:aws:appfabric:{region}:{ACCOUNT}:appbundle/{}",
                    uuid_of(&arn)
                ),
                "case {case:?}: a bundle must carry the ARN of the Region it was created in, got {arn:?}"
            );
            held.push(arn);
        }

        let refused = emulator
            .create_app_bundle_in_region(&region, second_body.clone())
            .await;
        assert_quota_refusal(&refused, &case);

        // The refusal persisted nothing: the Region's bundle is the one the first
        // call minted, and it still reads back.
        assert_eq!(
            listed_arns(&emulator, &region, &case).await,
            held,
            "case {case:?}: a refused create must leave the stored bundles alone"
        );
        let read = get_bundle(&emulator, &region, &held[0]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored bundle must survive the refusal, got {} with body {}",
            read.status, read.body
        );
    }
}

/// RULE: the quota counts the bundles of ONE Region, so every Region holds its
/// own — a Region at its quota does not refuse a create signed for another one.
#[tokio::test]
async fn every_region_holds_its_own_app_bundle() {
    let emulator = Emulator::new();
    // Several Regions at once, each filled to its quota and then refused once
    // more, so the count is per Region rather than per store.
    let regions: Vec<String> = (0..5).map(fixture_region).collect();
    let mut created: Vec<String> = Vec::new();

    for region in &regions {
        for attempt in 0..MAX_APP_BUNDLES_PER_REGION {
            let answer = emulator
                .create_app_bundle_in_region(region, json!({}))
                .await;
            assert_eq!(
                answer.status, 201,
                "case {region:?} bundle {attempt}: a Region's own quota must not be spent by \
                 another Region's bundles, got {} with body {}",
                answer.status, answer.body
            );
            let arn = arn_of(&answer.body, "appBundle");
            assert!(
                arn.contains(&format!(":appfabric:{region}:")),
                "case {region:?}: expected an ARN in that Region, got {arn:?}"
            );
            created.push(arn);
        }
    }

    for region in &regions {
        let refused = emulator
            .create_app_bundle_in_region(region, json!({}))
            .await;
        assert_quota_refusal(&refused, region);
    }

    let listed = listed_arns(&emulator, &regions[0], "every region").await;
    assert_eq!(
        listed, created,
        "expected one bundle per Region in creation order {created:?}, got {listed:?}"
    );
}

/// RULE: the quota is on what the account HOLDS, so deleting a Region's bundle
/// frees it and a later create in that Region succeeds again — with a new
/// identifier, because it is a new resource.
///
/// The table spans both `Identifier` forms the delete may name
/// (`^arn:.+$|^[a-f0-9]{8}-…$`) and the create bodies above.
#[tokio::test]
async fn a_delete_frees_the_region_for_another_create() {
    for (index, (body_case, body)) in create_bodies().into_iter().enumerate() {
        for form in ["by arn", "by uuid"] {
            let region = fixture_region(index);
            let case = format!("{body_case:?} deleted {form} in {region}");
            let emulator = Emulator::new();

            let first = emulator
                .create_app_bundle_in_region(&region, body.clone())
                .await;
            assert_eq!(
                first.status, 201,
                "case {case:?}: expected HTTP 201, got {} with body {}",
                first.status, first.body
            );
            let first_arn = arn_of(&first.body, "appBundle");

            let identifier = if form == "by arn" {
                first_arn.clone()
            } else {
                uuid_of(&first_arn)
            };
            let deleted = delete_bundle(&emulator, &region, &identifier).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteAppBundle, got {} with body {}",
                deleted.status, deleted.body
            );

            // The Region is free, so the next create is a create again — and a
            // DIFFERENT resource.
            let again = emulator
                .create_app_bundle_in_region(&region, body.clone())
                .await;
            assert_eq!(
                again.status, 201,
                "case {case:?}: expected HTTP 201 once the Region was freed, got {} with body {}",
                again.status, again.body
            );
            let second_arn = arn_of(&again.body, "appBundle");
            assert_ne!(
                second_arn, first_arn,
                "case {case:?}: a create after a delete must mint a new bundle, got {second_arn:?}"
            );
            assert_eq!(
                listed_arns(&emulator, &region, &case).await,
                vec![second_arn.clone()],
                "case {case:?}: expected only the bundle created after the delete"
            );

            // And the Region is at its quota again.
            let refused = emulator
                .create_app_bundle_in_region(&region, json!({}))
                .await;
            assert_quota_refusal(&refused, &case);
        }
    }
}

/// RULE: an `@idempotencyToken` retry REPLAYS the bundle its first call created
/// rather than asking for another one, so the quota never refuses it, however
/// often it is retried; a create in that Region under a token it has not seen is
/// a second bundle and is refused.
#[tokio::test]
async fn an_idempotent_retry_is_never_refused_by_the_quota() {
    // (case, the create body the token carries, how many times it is retried)
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "a bare token, retried once",
            json!({"clientToken": "11111111-0000-4000-8000-000000000000"}),
            1,
        ),
        (
            "a token with tags, retried three times",
            json!({"clientToken": "22222222-0000-4000-8000-000000000000",
                   "tags": [{"key": "env", "value": "prod"}]}),
            3,
        ),
        (
            "a token with a customer-managed key, retried five times",
            json!({"clientToken": "33333333-0000-4000-8000-000000000000",
                   "customerManagedKeyIdentifier": "11111111-2222-4333-8444-555555555555"}),
            5,
        ),
    ];

    for (index, (case, body, retries)) in cases.into_iter().enumerate() {
        let region = fixture_region(index);
        let emulator = Emulator::new();

        let created = emulator
            .create_app_bundle_in_region(&region, body.clone())
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let arn = arn_of(&created.body, "appBundle");

        for retry in 1..=retries {
            let replay = emulator
                .create_app_bundle_in_region(&region, body.clone())
                .await;
            assert_eq!(
                replay.status, 201,
                "case {case:?} retry {retry}: a replayed token must not meet the quota, got {} with body {}",
                replay.status, replay.body
            );
            assert_eq!(
                arn_of(&replay.body, "appBundle"),
                arn,
                "case {case:?} retry {retry}: expected the replayed bundle {arn:?}, got {}",
                replay.body
            );
        }
        assert_eq!(
            listed_arns(&emulator, &region, case).await,
            vec![arn.clone()],
            "case {case:?}: every retry must have replayed the one stored bundle"
        );

        // A token this Region has not spent asks for a SECOND bundle, which the
        // quota refuses.
        let refused = emulator
            .create_app_bundle_in_region(
                &region,
                json!({"clientToken": "ffffffff-0000-4000-8000-000000000000"}),
            )
            .await;
        assert_quota_refusal(&refused, case);
    }
}
