// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the publication metadata the AWS-MANAGED base-image catalogue
//! reports: the `createdAt`/`updatedAt` of a catalogue entry (`ListManagedMicrovmImages`)
//! and of each generation it publishes (`ListManagedMicrovmImageVersions`).
//!
//! RULE: the instants a published catalogue entry reports are the catalogue's OWN
//! metadata, not the clock of the read — an entry's `createdAt` is the instant its oldest
//! published generation was published and its `updatedAt` the instant its newest one was
//! (so the two are distinct for every image published in more than one generation, with
//! `updatedAt` the later), each published generation reports its own publication instant
//! (never rewritten after publication, so its `createdAt` and `updatedAt` are that one
//! instant), and none of those values moves between reads, between regions, or because of
//! anything an account does.
//!
//! The catalogue table the rule reads from is the service state's own
//! (`state::managed_base_image_publication`,
//! `state::managed_base_image_version_published_at`), so the assertions name the source
//! rather than any epoch value of one recording. The page walk, the identifier forms and
//! the generation ids themselves are the subject of `managed_base_image_catalogue`,
//! `managed_base_image_versions` and `managed_base_image_generations`.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    managed_base_image_arn, managed_base_image_publication,
    managed_base_image_version_published_at, published_managed_base_image_versions,
    MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{as_uri_value, call, call_in_region, emulator, image_body, REGION};

/// The regions the rows span: a managed base image is an AWS-owned REGIONAL resource, so
/// the catalogue publishes the same entries with the same metadata in every one of them.
const REGIONS: &[&str] = &[REGION, "us-east-1", "eu-central-1"];

/// `GET /2025-09-09/managed-microvm-images`, signed for `region`.
async fn list_managed(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        "/2025-09-09/managed-microvm-images",
        None,
    )
    .await
}

/// `GET /2025-09-09/managed-microvm-images/{imageIdentifier}/versions`, signed for
/// `region`.
async fn list_versions(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!(
            "/2025-09-09/managed-microvm-images/{}/versions",
            as_uri_value(identifier)
        ),
        None,
    )
    .await
}

/// The `items` of one answer.
fn items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// The row of `body` whose `imageArn` is `image_arn`.
fn row_for(body: &Value, image_arn: &str, case: &str) -> Value {
    items(body, case)
        .into_iter()
        .find(|item| item["imageArn"].as_str() == Some(image_arn))
        .unwrap_or_else(|| panic!("case {case}: expected a row for {image_arn}, got {body}"))
}

/// The row of `body` whose `imageVersion` is `image_version`.
fn version_row(body: &Value, image_version: &str, case: &str) -> Value {
    items(body, case)
        .into_iter()
        .find(|item| item["imageVersion"].as_str() == Some(image_version))
        .unwrap_or_else(|| {
            panic!("case {case}: expected a row for version {image_version}, got {body}")
        })
}

/// One timestamp member of a row, as the epoch-seconds number this protocol writes a
/// timestamp as.
fn instant(item: &Value, member: &str, case: &str) -> f64 {
    item[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a {member} timestamp, got {item}"))
}

/// The publication instants the catalogue table holds for `image_id`, as the
/// epoch-seconds numbers a listing reports them as.
fn published_pair(image_id: &str) -> (f64, f64) {
    let (created_at, updated_at) = managed_base_image_publication(image_id)
        .unwrap_or_else(|| panic!("the catalogue publishes {image_id}"));
    (created_at.secs() as f64, updated_at.secs() as f64)
}

/// RULE: a catalogue entry's `createdAt` is the publication instant of the oldest
/// generation the catalogue holds of it and its `updatedAt` that of the newest, so an
/// image published in more than one generation reports two DISTINCT instants with
/// `updatedAt` the later — in every region, because the entry is AWS's own.
#[tokio::test]
async fn a_catalogue_entry_reports_the_publication_instants_of_its_generations() {
    let router = emulator();

    // Rows span the whole published catalogue (`state::MANAGED_BASE_IMAGE_IDS`, the set
    // `CreateMicrovmImage$baseImageArn` resolves against) crossed with the regions a
    // request can be signed for.
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let (expected_created, expected_updated) = published_pair(image_id);
        let generations = published_managed_base_image_versions(image_id);

        for region in REGIONS {
            let case = format!("{image_id:?} in {region:?}");
            let image_arn = managed_base_image_arn(region, image_id);
            let (status, body) = list_managed(&router, region).await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

            let row = row_for(&body, &image_arn, &case);
            assert_eq!(
                instant(&row, "createdAt", &case),
                expected_created,
                "case {case}: createdAt is the oldest published generation's instant, got {row}"
            );
            assert_eq!(
                instant(&row, "updatedAt", &case),
                expected_updated,
                "case {case}: updatedAt is the newest published generation's instant, got {row}"
            );

            // More than one generation published means the entry was written again after
            // it was created, so the two instants cannot be the same value.
            match generations.len() > 1 {
                true => assert!(
                    instant(&row, "updatedAt", &case) > instant(&row, "createdAt", &case),
                    "case {case}: {} generations are published, so updatedAt is strictly later \
                     than createdAt, got {row}",
                    generations.len()
                ),
                false => assert_eq!(
                    instant(&row, "updatedAt", &case),
                    instant(&row, "createdAt", &case),
                    "case {case}: one generation is published, so the entry was written once, \
                     got {row}"
                ),
            }
        }
    }
}

/// RULE: each published generation reports its own publication instant, and a published
/// generation is never rewritten (AWS publishes another generation instead), so its
/// `createdAt` and `updatedAt` are that one instant — and the generations a version
/// listing pages newest first carry strictly decreasing instants.
#[tokio::test]
async fn every_published_generation_reports_its_own_publication_instant() {
    let router = emulator();

    // Rows span every published generation of every published image
    // (`state::published_managed_base_image_versions`), addressed by both forms
    // `MicrovmImageIdentifier` accepts, in several regions.
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let generations = published_managed_base_image_versions(image_id);
        assert!(
            !generations.is_empty(),
            "case {image_id:?}: a published base image publishes at least one generation"
        );

        for region in REGIONS {
            let identifiers = [
                ("the catalogue id", (*image_id).to_string()),
                ("the managed ARN", managed_base_image_arn(region, image_id)),
            ];

            for (form, identifier) in &identifiers {
                let case = format!("{form} {identifier:?} in {region:?}");
                let (status, body) = list_versions(&router, region, identifier).await;
                assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

                for generation in &generations {
                    let expected = managed_base_image_version_published_at(image_id, generation)
                        .unwrap_or_else(|| {
                            panic!("case {case}: the catalogue publishes generation {generation}")
                        })
                        .secs() as f64;
                    let row = version_row(&body, generation, &case);
                    assert_eq!(
                        instant(&row, "createdAt", &case),
                        expected,
                        "case {case}: generation {generation} reports the instant it was \
                         published, got {row}"
                    );
                    assert_eq!(
                        instant(&row, "updatedAt", &case),
                        expected,
                        "case {case}: generation {generation} was never rewritten after it was \
                         published, got {row}"
                    );
                }

                // Newest generation first is the order the listing pages, so the instants
                // it reports descend.
                let listed: Vec<f64> = items(&body, &case)
                    .iter()
                    .map(|item| instant(item, "createdAt", &case))
                    .collect();
                assert!(
                    listed.windows(2).all(|pair| pair[0] > pair[1]),
                    "case {case}: a newer generation is published later, got {listed:?}"
                );
            }
        }
    }
}

/// RULE: the catalogue's instants are fixed metadata — reading the catalogue, reading an
/// entry's versions, creating an account image or reading from another region moves
/// nothing, so every answer carries the same values as the first one.
#[tokio::test]
async fn the_published_instants_never_move_under_reads_or_account_activity() {
    let router = emulator();
    let image_id = MANAGED_BASE_IMAGE_IDS[0];

    let (status, first_catalogue) = list_managed(&router, REGION).await;
    assert_eq!(
        status, 200,
        "list the catalogue: got {status} {first_catalogue}"
    );
    let (status, first_versions) = list_versions(&router, REGION, image_id).await;
    assert_eq!(
        status, 200,
        "list the versions: got {status} {first_versions}"
    );

    // Rows differ in what happens between two reads of the catalogue: nothing at all,
    // another read of it, a read of its versions, and an account creating its own image
    // on it.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("an immediate re-read", None),
        ("after a plain image create", Some(json!({}))),
        (
            "after a tagged image create",
            Some(json!({"description": "mine", "tags": {"team": "platform"}})),
        ),
    ];

    for (index, (case, extra)) in rows.into_iter().enumerate() {
        if let Some(extra) = extra {
            let name = format!("mine-{index}");
            let (status, created) = call(
                &router,
                "POST",
                "/2025-09-09/microvm-images",
                Some(image_body(&name, extra)),
            )
            .await;
            assert_eq!(status, 201, "case {case:?}: create got {status} {created}");
        }

        let (status, catalogue) = list_managed(&router, REGION).await;
        assert_eq!(status, 200, "case {case:?}: re-list got {status} {catalogue}");
        assert_eq!(
            catalogue, first_catalogue,
            "case {case:?}: the published catalogue metadata is fixed, got {catalogue}"
        );

        let (status, versions) = list_versions(&router, REGION, image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-list versions got {status} {versions}"
        );
        assert_eq!(
            versions, first_versions,
            "case {case:?}: a published generation's metadata is fixed, got {versions}"
        );
    }

    // Another region publishes the same instants: only the ARNs carry the region.
    for region in REGIONS {
        let case = format!("the catalogue in {region:?}");
        let (status, body) = list_managed(&router, region).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");
        let row = row_for(&body, &managed_base_image_arn(region, image_id), &case);
        let (expected_created, expected_updated) = published_pair(image_id);
        assert_eq!(
            (
                instant(&row, "createdAt", &case),
                instant(&row, "updatedAt", &case)
            ),
            (expected_created, expected_updated),
            "case {case}: every region publishes the same entry metadata, got {row}"
        );
    }
}
