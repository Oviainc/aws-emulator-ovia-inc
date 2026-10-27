// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SCOPE of the image-build read plane, driven over the same
//! router the emulator boots.
//!
//! RULE: `ListMicrovmImageBuilds` and `GetMicrovmImageBuild` report the builds held by
//! exactly the image version the request names, so a build record never crosses between
//! sibling images: every build a listing serves carries the named image's ARN and
//! version, whatever filter, page size or identifier form (`MicrovmImageIdentifier` is
//! "the unique identifier (ARN or ID)") the request used, and a `buildId` another image
//! holds is `ResourceNotFoundException` (HTTP 404) under this one.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, and the
//! build-plane helpers are the ones `image_build_read_path` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_build_not_found_message, FIRST_IMAGE_VERSION,
};

use crate::image_build_read_path::{
    arm64_targets, create_image, get_build, list_builds, page_build_ids, page_items,
};
use crate::microvm_lifecycle::emulator;

/// One image under test: the case that describes it, the name it was created under, the
/// ARN its create minted, and the `buildId`s its own first version holds.
struct ScopedImage {
    case: &'static str,
    name: &'static str,
    image_arn: String,
    build_ids: Vec<String>,
}

/// RULE: the build plane is scoped to the image version named, so each image's listing
/// serves only the builds its own version holds — identically whether the image is
/// addressed by name or by ARN — and the build ids of two images are disjoint.
#[tokio::test]
async fn each_image_listing_serves_only_its_own_builds() {
    let router = emulator();
    let images = scoped_images(&router).await;

    // Rows: every query this listing publishes, over every image — no query at all, a
    // page smaller than the build set, the architecture filter, and each chipset
    // generation the service builds (`state::BUILD_TARGETS`, via `arm64_targets`).
    let mut queries: Vec<String> = vec![
        String::new(),
        "maxResults=1".to_string(),
        "architecture=ARM_64".to_string(),
    ];
    queries.extend(
        arm64_targets()
            .iter()
            .map(|(_, generation)| format!("chipsetGeneration={generation}")),
    );

    for image in &images {
        for query in &queries {
            let case = format!("{:?} with {query:?}", image.case);
            let (status, body) =
                list_builds(&router, &image.image_arn, FIRST_IMAGE_VERSION, query).await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

            for item in page_items(&body, &case) {
                assert_eq!(
                    item["imageArn"].as_str(),
                    Some(image.image_arn.as_str()),
                    "case {case}: every build served names the image asked for, got {:?}",
                    item["imageArn"]
                );
                assert_eq!(
                    item["imageVersion"].as_str(),
                    Some(FIRST_IMAGE_VERSION),
                    "case {case}: every build served names the version asked for, got {:?}",
                    item["imageVersion"]
                );
                let build_id = item["buildId"].as_str().unwrap_or_default().to_string();
                assert!(
                    image.build_ids.contains(&build_id),
                    "case {case}: expected a build of {:?} ({:?}), got {build_id}",
                    image.name,
                    image.build_ids
                );
            }
        }
    }

    // The build sets of two images are disjoint, so the scope assertions above can tell
    // one image's builds from another's at all.
    for image in &images {
        for sibling in &images {
            if sibling.name == image.name {
                continue;
            }
            for build_id in &image.build_ids {
                assert!(
                    !sibling.build_ids.contains(build_id),
                    "case {:?} vs {:?}: two images hold distinct builds, got {build_id} in both",
                    image.case,
                    sibling.case
                );
            }
        }
    }
}

/// RULE: a `buildId` the named image version does not hold is `ResourceNotFoundException`
/// (HTTP 404) naming what was missing — including a build id a SIBLING image of the same
/// account holds, which this image's listing therefore never serves either.
#[tokio::test]
async fn a_sibling_images_build_is_not_found_under_this_one() {
    let router = emulator();
    let images = scoped_images(&router).await;

    for image in &images {
        let (status, body) = list_builds(&router, &image.image_arn, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: listing got {status} {body}",
            image.case
        );
        let served = page_build_ids(&body, image.case);

        for sibling in &images {
            if sibling.name == image.name {
                continue;
            }
            for build_id in &sibling.build_ids {
                let case = format!("{:?} reading {:?}'s {build_id}", image.case, sibling.case);
                assert!(
                    !served.contains(build_id),
                    "case {case}: a listing serves no sibling's build, got {served:?}"
                );

                let (status, body) =
                    get_build(&router, &image.image_arn, FIRST_IMAGE_VERSION, build_id).await;
                assert_eq!(status, 404, "case {case}: expected 404, got {status} {body}");
                assert_eq!(
                    body["__type"].as_str(),
                    Some("ResourceNotFoundException"),
                    "case {case}: expected ResourceNotFoundException, got {body}"
                );
                let expected =
                    image_build_not_found_message(&image.image_arn, FIRST_IMAGE_VERSION, build_id);
                assert_eq!(
                    body["message"].as_str(),
                    Some(expected.as_str()),
                    "case {case}: expected the refusal naming what was missing, got {:?}",
                    body["message"]
                );
            }
        }
    }
}

/// The images every test here runs over. Rows differ in the build inputs the create
/// carried — what the fan-out generalises over — so the images hold different build sets
/// rather than copies of one, and each row's listing is read back through BOTH identifier
/// forms to prove they address the same version.
async fn scoped_images(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<ScopedImage> {
    let rows: Vec<(&'static str, &'static str, Value)> = vec![
        (
            "the configuration the service supplies",
            "scoped-defaulted",
            json!({}),
        ),
        (
            "an explicit architecture and memory floor",
            "scoped-explicit",
            json!({
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 512}],
            }),
        ),
        (
            "the same architecture named twice",
            "scoped-repeated",
            json!({"cpuConfigurations": [
                {"architecture": "ARM_64"},
                {"architecture": "ARM_64"},
            ]}),
        ),
    ];

    let mut images: Vec<ScopedImage> = Vec::new();
    for (case, name, extra) in rows {
        let image_arn = create_image(router, name, extra).await;

        let (status, by_arn) = list_builds(router, &image_arn, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing by ARN got {status} {by_arn}"
        );
        let (status, by_name) = list_builds(router, name, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing by name got {status} {by_name}"
        );
        assert_eq!(
            by_name, by_arn,
            "case {case:?}: both identifier forms address one version, got {by_name} and {by_arn}"
        );

        let build_ids = page_build_ids(&by_arn, case);
        assert!(
            !build_ids.is_empty(),
            "case {case:?}: one create fans out into at least one build, got {by_arn}"
        );
        images.push(ScopedImage {
            case,
            name,
            image_arn,
            build_ids,
        });
    }
    images
}
