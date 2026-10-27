// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the FORM `MicrovmImageBuildSummary$chipsetGeneration` reports,
//! driven over the same router the emulator boots.
//!
//! RULE: `chipset` and `chipsetGeneration` are modelled as separate members — "The target
//! chipset for the build." and "The target chipset generation for the build." — so the
//! chipset FAMILY is reported only by `chipset` and `chipsetGeneration` carries just the
//! generation of it, as a decimal-digit string, one distinct value per build of the
//! fan-out; every surface that projects or filters on it (`ListMicrovmImageBuilds`, its
//! `chipsetGeneration` httpQuery filter, and `GetMicrovmImageBuild`) therefore speaks the
//! same form, so a generation a client was told can be fed straight back as a filter.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    BUILD_TARGETS, FIRST_IMAGE_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{as_uri_value, call, emulator, image_body};

/// `GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds`,
/// with whatever `query` adds to the query string.
async fn list_builds(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
    query: &str,
) -> (u16, Value) {
    let path = format!(
        "/2025-09-09/microvm-images/{}/versions/{}/builds",
        as_uri_value(identifier),
        as_uri_value(image_version)
    );
    let uri = match query.is_empty() {
        true => path,
        false => format!("{path}?{query}"),
    };
    call(router, "GET", &uri, None).await
}

/// `GET
/// /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds/{buildId}`.
async fn get_build(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
    build_id: &str,
) -> (u16, Value) {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions/{}/builds/{}",
        as_uri_value(identifier),
        as_uri_value(image_version),
        as_uri_value(build_id)
    );
    call(router, "GET", &uri, None).await
}

/// The `items` of one page.
fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// Create an image (named `name`, with whatever `extra` adds to the body) and return the
/// ARN the create minted.
async fn create_image(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, extra)),
    )
    .await;
    assert_eq!(status, 201, "create image {name}: got {status} {body}");
    body["imageArn"].as_str().unwrap_or_default().to_string()
}

/// RULE: every build the fan-out produces reports its `chipsetGeneration` as a
/// decimal-digit string carrying no chipset family, distinct from every sibling build's,
/// while the family itself is reported by `chipset` — and the single-build read agrees
/// with the listing's summary.
#[tokio::test]
async fn every_build_reports_its_chipset_generation_as_a_decimal_number() {
    let router = emulator();

    // Rows differ in which architectures the request asks to be built, which is what the
    // fan-out generalises over; the generations each build reports are the service's own
    // published targets (`state::BUILD_TARGETS`), not this test's.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the architecture the service supplies", "supplied", json!({})),
        (
            "the one architecture modelled, named once",
            "named-once",
            json!({"cpuConfigurations": [{"architecture": "ARM_64"}]}),
        ),
        (
            "the same architecture named twice",
            "named-twice",
            json!({"cpuConfigurations": [
                {"architecture": "ARM_64"},
                {"architecture": "ARM_64"},
            ]}),
        ),
    ];

    for (case, name, extra) in rows {
        let image_arn = create_image(&router, name, extra).await;
        let (status, body) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let items = page_items(&body, case);
        assert!(
            !items.is_empty(),
            "case {case:?}: one create fans out into at least one build, got {body}"
        );
        let mut generations: Vec<String> = Vec::new();
        for item in &items {
            let generation = item["chipsetGeneration"].as_str().unwrap_or_default();
            assert!(
                !generation.is_empty() && generation.chars().all(|c| c.is_ascii_digit()),
                "case {case:?}: chipsetGeneration is the generation NUMBER, got \
                 {generation:?}"
            );
            let chipset = item["chipset"].as_str().unwrap_or_default();
            assert!(
                !chipset.is_empty() && !generation.contains(chipset),
                "case {case:?}: the chipset family is reported by chipset ({chipset:?}), not by \
                 chipsetGeneration, got {generation:?}"
            );
            assert!(
                !generations.contains(&generation.to_string()),
                "case {case:?}: the fan-out is one build per generation, got {generation:?} \
                 twice in {generations:?}"
            );
            generations.push(generation.to_string());

            // The single-build read reports the same tuple the listing summarised.
            let build_id = item["buildId"].as_str().unwrap_or_default();
            let (status, build) =
                get_build(&router, &image_arn, FIRST_IMAGE_VERSION, build_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read {build_id} got {status} {build}"
            );
            for member in ["imageArn", "imageVersion", "chipset", "chipsetGeneration"] {
                assert_eq!(
                    build[member], item[member],
                    "case {case:?}: {member} of {build_id} must agree between the listing and \
                     the read, got {:?} and {:?}",
                    build[member], item[member]
                );
            }
        }

        let mut published: Vec<String> = BUILD_TARGETS
            .iter()
            .filter(|target| target.architecture == "ARM_64")
            .map(|target| target.chipset_generation.to_string())
            .collect();
        published.sort();
        generations.sort();
        assert_eq!(
            generations, published,
            "case {case:?}: expected one build per published generation, got {generations:?}"
        );
    }
}

/// RULE: the `chipsetGeneration` filter speaks the same form the summaries report, so a
/// generation taken from a listing selects exactly the build that carries it, and a
/// generation nothing was built for selects nothing — `items` is `@required`, so that is
/// an empty page and not a refusal.
#[tokio::test]
async fn the_generation_filter_matches_the_form_the_builds_report() {
    let router = emulator();
    let image_arn = create_image(&router, "filtered", json!({})).await;

    let (status, body) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
    assert_eq!(status, 200, "list the builds: got {status} {body}");
    let reported: Vec<String> = page_items(&body, "the whole listing")
        .iter()
        .map(|item| {
            item["chipsetGeneration"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        !reported.is_empty(),
        "the listing the filters are taken from is not empty, got {body}"
    );

    // Rows: every generation the listing reported (each selects exactly its own build),
    // and generations in the same decimal form that nothing was built for — including one
    // just past the published set.
    let unbuilt: Vec<String> = ["0", "99"]
        .iter()
        .map(|generation| (*generation).to_string())
        .filter(|generation| !reported.contains(generation))
        .collect();
    let rows: Vec<(String, usize)> = reported
        .iter()
        .map(|generation| (generation.clone(), 1))
        .chain(unbuilt.into_iter().map(|generation| (generation, 0)))
        .collect();

    for (generation, matches) in rows {
        let case = format!("chipsetGeneration={generation:?}");
        let (status, page) = list_builds(
            &router,
            &image_arn,
            FIRST_IMAGE_VERSION,
            &format!("chipsetGeneration={generation}"),
        )
        .await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {page}");
        let items = page_items(&page, &case);
        assert_eq!(
            items.len(),
            matches,
            "case {case}: expected {matches} build(s), got {page}"
        );
        for item in &items {
            assert_eq!(
                item["chipsetGeneration"].as_str(),
                Some(generation.as_str()),
                "case {case}: the filter selects the generation asked for, got {:?}",
                item["chipsetGeneration"]
            );
        }
    }
}
