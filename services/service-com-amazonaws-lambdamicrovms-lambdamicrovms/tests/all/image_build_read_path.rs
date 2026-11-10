// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the image-build READ path — `ListMicrovmImageBuilds` and
//! `GetMicrovmImageBuild` — driven over the same router the emulator boots.
//!
//! RULE: the build-read plane decides its authorization verdict on the image the request
//! names BEFORE looking anything up, so a caller reads builds only through an identifier
//! of its own account's image namespace: an identifier of another namespace is
//! `AccessDeniedException` (HTTP 403) whatever version or build id accompanies it, and a
//! value that is no identifier this API can address is `ValidationException` (HTTP 400).
//! Past that verdict the single-build READ reports what it finds and misses with
//! `ResourceNotFoundException` (HTTP 404), while the paginated LISTING reports the builds
//! held at the address the request named and answers an EMPTY PAGE for an address this
//! account holds none at (`image_refusal_paths` owns that rule).
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_build_not_found_message, image_version_not_found_message, invalid_arn_format_message,
    managed_base_image_arn, published_managed_base_image_versions, access_denied_message,
    BUILD_STATE_IN_PROGRESS, BUILD_STATE_SUCCESSFUL, BUILD_TARGETS,
    DEFAULT_MINIMUM_MEMORY_IN_MIB, FIRST_IMAGE_VERSION, MANAGED_BASE_IMAGE_IDS, MAX_LIST_PAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, ACCOUNT, PAST_THE_SPAN, REGION,
};

/// `GET /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}/builds`,
/// with whatever `query` adds to the query string.
pub(crate) async fn list_builds(
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
pub(crate) async fn get_build(
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
pub(crate) fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// The `buildId`s of one page, in the order the page carries them.
pub(crate) fn page_build_ids(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| item["buildId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A hex-encoded `nextToken` candidate, as the opaque token form this listing issues.
fn hex_token(value: &str) -> String {
    ::hex::encode(value.as_bytes())
}

/// Create an image (named `name`, with whatever `extra` adds to the body) and return the
/// ARN the create minted.
pub(crate) async fn create_image(
    router: &MultiServiceRouter,
    name: &str,
    extra: Value,
) -> String {
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

/// The `(chipset, chipsetGeneration)` pairs the service builds the one modelled
/// architecture for, as `state::BUILD_TARGETS` publishes them.
pub(crate) fn arm64_targets() -> Vec<(&'static str, &'static str)> {
    BUILD_TARGETS
        .iter()
        .filter(|target| target.architecture == "ARM_64")
        .map(|target| (target.chipset, target.chipset_generation))
        .collect()
}

/// RULE (the authorization verdict, decided before any lookup): a build read addressed at
/// an identifier OUTSIDE the calling account's own image namespace — the AWS-owned managed
/// catalogue, another account's, another region's or another service's — is refused with
/// `AccessDeniedException` (HTTP 403), on both build reads and whatever version or build
/// id accompanies it, so no caller can use the refusal to learn whether the identifier
/// exists. An identifier OF that namespace holding no builds here is answered about the
/// records instead: the listing serves an empty page.
#[tokio::test]
async fn a_build_read_outside_this_accounts_own_images_is_denied() {
    let router = emulator();

    // An image this account DOES hold, so the denied rows are denied for the identifier
    // they name rather than for an empty store.
    let held = "held-image";
    let held_arn = create_image(&router, held, json!({})).await;
    let managed_id = MANAGED_BASE_IMAGE_IDS[0];
    let managed_version = published_managed_base_image_versions(managed_id)[0];

    // Rows differ in WHICH namespace the well-formed identifier belongs to — the
    // dimension the verdict generalises over. Every one of them is outside this account's
    // own image namespace, which is what the denial is about.
    let identifiers: Vec<(&str, String)> = vec![
        (
            "an AWS-owned managed base image ARN",
            managed_base_image_arn(REGION, managed_id),
        ),
        (
            "a managed ARN the catalogue does not publish",
            managed_base_image_arn(REGION, "no-such-base-image"),
        ),
        (
            "another account's ARN",
            format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:someone-elses"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:image:ami-01234567"),
        ),
    ];

    // Rows differ in the version and build id the request carries, including the version
    // the managed catalogue really publishes and the id form this service mints — none of
    // them may turn the verdict into a lookup.
    let versions = [FIRST_IMAGE_VERSION, managed_version, "9.9"];
    let build_ids = [
        "probe-build-00000000",
        "build-0000000000000000",
        &"b".repeat(256),
    ];

    for (namespace, identifier) in &identifiers {
        for image_version in versions {
            let case = format!("{namespace} {identifier:?} version {image_version:?}");
            let (status, body) = list_builds(&router, identifier, image_version, "").await;
            assert_denied(status, &body, &format!("ListMicrovmImageBuilds {case}"));

            for build_id in build_ids {
                let case = format!("{case} build {build_id:?}");
                let (status, body) = get_build(&router, identifier, image_version, build_id).await;
                assert_denied(status, &body, &format!("GetMicrovmImageBuild {case}"));
            }
        }
    }

    // An identifier of this account's OWN namespace that names no image here holds no
    // builds, and the listing refuses nothing past the verdict: it answers an empty page.
    // The rows differ in HOW the own-namespace identifier fails to name an image here.
    let own_absent: Vec<(&str, String)> = vec![
        (
            "an own-account ARN nobody created",
            expected_image_arn("never-created-image"),
        ),
        (
            "another region's ARN for an image this region holds",
            format!("arn:aws:lambda:eu-west-1:{ACCOUNT}:microvm-image:{held}"),
        ),
    ];
    for (case, identifier) in &own_absent {
        let (status, body) = list_builds(&router, identifier, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: {identifier:?} holds no builds to list, got {status} {body}"
        );
        assert_eq!(
            body["items"],
            json!([]),
            "case {case:?}: an address holding no builds is an empty page, got {body}"
        );
    }

    // The positive control: the image this account holds is readable through the same
    // path, so the rows above are denied by the verdict and not by a dead read path.
    let (status, body) = list_builds(&router, &held_arn, FIRST_IMAGE_VERSION, "").await;
    assert_eq!(
        status, 200,
        "case {held_arn:?}: this account's own image is readable, got {status} {body}"
    );
}

/// The refusal every denied row must carry.
fn assert_denied(status: u16, body: &Value, case: &str) {
    assert_eq!(
        status, 403,
        "case {case}: expected 403, got {status} {body}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(access_denied_message().as_str()),
        "case {case}: expected the modelled denial sentence, got {:?}",
        body["message"]
    );
}

/// RULE: a value that is neither an id this account holds nor a well-formed ARN names
/// nothing this API can address, so it is a request that can never succeed — HTTP 400
/// `ValidationException` naming the identifier it carried — rather than a permission or
/// a not-found verdict.
#[tokio::test]
async fn a_value_that_is_no_identifier_of_this_api_is_refused() {
    let router = emulator();
    create_image(&router, "held-image", json!({})).await;

    // Rows differ in HOW the value fails to address a resource of this API: a bare name
    // this account holds no image under, the AWS-managed catalogue's own id (which is no
    // id of this account), and values that are no identifier at all.
    let rows = [
        ("a bare name this account holds no image under", "missing"),
        ("the managed catalogue's own id", MANAGED_BASE_IMAGE_IDS[0]),
        ("no identifier of this API at all", "not-an-identifier"),
    ];

    for (case, identifier) in rows {
        for (operation, (status, body)) in [
            (
                "ListMicrovmImageBuilds",
                list_builds(&router, identifier, FIRST_IMAGE_VERSION, "").await,
            ),
            (
                "GetMicrovmImageBuild",
                get_build(&router, identifier, FIRST_IMAGE_VERSION, "probe-build").await,
            ),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {body}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?} on {operation}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(invalid_arn_format_message(identifier).as_str()),
                "case {case:?} on {operation}: expected the refusal built from the identifier \
                 the request carried, got {:?}",
                body["message"]
            );
        }
    }
}

/// RULE: a build read of an image this account holds reports the PERSISTED build records
/// of the version it names — one per target the accepted `cpuConfigurations` ask for,
/// each carrying the image's ARN, the version, its own id and target, the state of the
/// asynchronous build (`IN_PROGRESS` until the clock settles it, then `SUCCESSFUL` with
/// the snapshot the version's own memory requirement sizes) — and the listing and the
/// per-build read report the same record.
#[tokio::test]
async fn the_builds_of_a_held_image_are_served_and_read_back() {
    let router = emulator();

    // Rows differ in the build inputs the create carried: the configuration the service
    // supplies when the request names none, an explicit one, and a request that names
    // the same architecture twice (which is one build per TARGET, not a duplicate).
    let rows: Vec<(&str, &str, Value, i32)> = vec![
        (
            "the configuration the service supplies",
            "defaulted",
            json!({}),
            DEFAULT_MINIMUM_MEMORY_IN_MIB,
        ),
        (
            "an explicit architecture and memory floor",
            "explicit",
            json!({
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 512}],
            }),
            512,
        ),
        (
            "the same architecture named twice",
            "repeated",
            json!({
                "cpuConfigurations": [{"architecture": "ARM_64"}, {"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 4096}],
            }),
            4096,
        ),
    ];

    for (case, name, extra, memory_in_mib) in rows {
        let image_arn = create_image(&router, name, extra).await;
        let expected_targets = arm64_targets();

        // Before the clock settles the asynchronous build, the builds are the ones the
        // create scheduled and each is still running.
        let (status, body) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        let running = page_items(&body, case);
        assert_eq!(
            running
                .iter()
                .map(|item| (
                    item["chipset"].as_str().unwrap_or_default().to_string(),
                    item["chipsetGeneration"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                ))
                .collect::<Vec<_>>(),
            expected_targets
                .iter()
                .map(|(chipset, generation)| ((*chipset).to_string(), (*generation).to_string()))
                .collect::<Vec<_>>(),
            "case {case:?}: expected one build per target the service builds, got {body}"
        );
        for item in &running {
            assert_eq!(
                item["buildState"].as_str(),
                Some(BUILD_STATE_IN_PROGRESS),
                "case {case:?}: the build is asynchronous, so it starts running, got {:?}",
                item["buildState"]
            );
            assert_eq!(
                item["imageArn"].as_str(),
                Some(image_arn.as_str()),
                "case {case:?}: every build names its image, got {:?}",
                item["imageArn"]
            );
            assert_eq!(
                item["imageVersion"].as_str(),
                Some(FIRST_IMAGE_VERSION),
                "case {case:?}: every build names its version, got {:?}",
                item["imageVersion"]
            );
            assert_eq!(
                item["architecture"].as_str(),
                Some("ARM_64"),
                "case {case:?}: the build carries the architecture it targets, got {:?}",
                item["architecture"]
            );
            assert!(
                item["createdAt"].as_f64().is_some_and(|at| at > 0.0),
                "case {case:?}: every build carries the instant it was created, got {:?}",
                item["createdAt"]
            );
        }

        // A build that is still running has produced no snapshot yet.
        for build_id in page_build_ids(&body, case) {
            let (status, build) =
                get_build(&router, &image_arn, FIRST_IMAGE_VERSION, &build_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading {build_id} got {status} {build}"
            );
            assert!(
                build["snapshotBuild"].is_null(),
                "case {case:?}: a running build has produced no snapshot, got {:?}",
                build["snapshotBuild"]
            );
        }

        // Once the clock has made the build due, every build has settled and carries the
        // snapshot sized from the version's own memory requirement (MiB -> bytes).
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, settled) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-list got {status} {settled}"
        );
        let expected_bytes = i64::from(memory_in_mib) * 1024 * 1024;
        for item in page_items(&settled, case) {
            let build_id = item["buildId"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                item["buildState"].as_str(),
                Some(BUILD_STATE_SUCCESSFUL),
                "case {case:?}: the settled build succeeded, got {:?}",
                item["buildState"]
            );

            let (status, build) =
                get_build(&router, &image_arn, FIRST_IMAGE_VERSION, &build_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading {build_id} got {status} {build}"
            );
            // The listing and the per-build read report ONE record, so every member the
            // summary carries comes back identical from the read.
            for member in [
                "imageArn",
                "imageVersion",
                "buildId",
                "buildState",
                "architecture",
                "chipset",
                "chipsetGeneration",
                "createdAt",
            ] {
                assert_eq!(
                    build[member], item[member],
                    "case {case:?}: {member} of {build_id} must agree between the listing and \
                     the read, got {:?} and {:?}",
                    build[member], item[member]
                );
            }
            assert_eq!(
                build["snapshotBuild"]["memorySnapshotSizeInBytes"].as_i64(),
                Some(expected_bytes),
                "case {case:?}: the snapshot is sized from the version's memory floor \
                 ({memory_in_mib} MiB), got {:?}",
                build["snapshotBuild"]
            );
        }

        // The build reads are reports: reading them changes nothing a later caller sees.
        let (status, again) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
        assert_eq!(status, 200, "case {case:?}: re-list got {status} {again}");
        assert_eq!(
            again, settled,
            "case {case:?}: reading the builds must not change them, got {again}"
        );
    }
}

/// RULE: past the authorization verdict the lookup is an own-account one, so a version
/// the resolved image does not hold, and a `buildId` that version does not hold, are
/// `ResourceNotFoundException` (HTTP 404) naming what was missing — the only place a
/// build read reports a miss.
#[tokio::test]
async fn a_version_or_build_a_held_image_does_not_hold_is_not_found() {
    let router = emulator();
    let image_arn = create_image(&router, "held-image", json!({})).await;
    let (_, listed) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
    let build_id = page_build_ids(&listed, "held-image")
        .first()
        .cloned()
        .unwrap_or_default();

    // Rows differ in WHICH step of the lookup misses, and in the form of the value that
    // misses it.
    let rows: Vec<(&str, String, String, u16, String)> = vec![
        (
            "a version the image does not hold",
            "2.0".to_string(),
            build_id.clone(),
            404,
            image_version_not_found_message(&image_arn, "2.0"),
        ),
        (
            "a version in no form this service mints",
            "latest".to_string(),
            build_id.clone(),
            404,
            image_version_not_found_message(&image_arn, "latest"),
        ),
        (
            "a build id the version does not hold",
            FIRST_IMAGE_VERSION.to_string(),
            "build-ffffffffffffffff".to_string(),
            404,
            image_build_not_found_message(
                &image_arn,
                FIRST_IMAGE_VERSION,
                "build-ffffffffffffffff",
            ),
        ),
        (
            "a build id in no form this service mints",
            FIRST_IMAGE_VERSION.to_string(),
            "probe-build-1".to_string(),
            404,
            image_build_not_found_message(&image_arn, FIRST_IMAGE_VERSION, "probe-build-1"),
        ),
        (
            "the build the version does hold",
            FIRST_IMAGE_VERSION.to_string(),
            build_id.clone(),
            200,
            String::new(),
        ),
    ];

    for (case, image_version, probe_build_id, expected_status, expected_message) in rows {
        let (status, body) = get_build(&router, &image_arn, &image_version, &probe_build_id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        if expected_status == 404 {
            assert_eq!(
                body["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: expected the refusal naming what was missing, got {:?}",
                body["message"]
            );
        }
    }

    // The LISTING of the same unheld version is not a refusal: the version holds no
    // builds, so the page is empty (`image_refusal_paths` owns that rule).
    let (status, body) = list_builds(&router, &image_arn, "2.0", "").await;
    assert_eq!(
        status, 200,
        "listing an unheld version: expected 200, got {status} {body}"
    );
    assert_eq!(
        body["items"],
        json!([]),
        "listing an unheld version: expected an empty page, got {body}"
    );
}

/// RULE: the listing serves the version's whole build set — every build exactly once, in
/// one stable order, at most `maxResults` per page, with a `nextToken` only while builds
/// remain — and each filter the request names selects the builds matching it.
#[tokio::test]
async fn every_build_is_served_once_over_every_page_size_and_filter() {
    let router = emulator();
    let image_arn = create_image(&router, "paged-image", json!({})).await;
    let targets = arm64_targets();

    let (status, all) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
    assert_eq!(status, 200, "the unfiltered listing got {status} {all}");
    let expected = page_build_ids(&all, "unfiltered");
    assert_eq!(
        expected.len(),
        targets.len(),
        "the version holds one build per target, got {all}"
    );

    // Rows span the page sizes the walk generalises over: no `maxResults` at all, each
    // published bound of `@range(min: 1, max: 50)`, and every size up to the build set's
    // own length.
    let mut sizes: Vec<Option<usize>> = vec![None, Some(1), Some(MAX_LIST_PAGE)];
    sizes.extend((2..=expected.len()).map(Some));

    for size in sizes {
        let case = format!("maxResults={size:?}");
        let cap = size.unwrap_or(MAX_LIST_PAGE);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;

        // One page per build, plus one: a listing that never terminates fails here
        // rather than hanging.
        for _ in 0..=expected.len() {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let (status, body) = list_builds(
                &router,
                &image_arn,
                FIRST_IMAGE_VERSION,
                &query.join("&"),
            )
            .await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

            let page = page_build_ids(&body, &case);
            assert!(
                page.len() <= cap,
                "case {case}: a page carries at most {cap} builds, got {} ({page:?})",
                page.len()
            );
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            match &token {
                Some(issued) => assert!(
                    seen.len() < expected.len(),
                    "case {case}: a nextToken ({issued}) is due only while builds remain, got it \
                     after {seen:?}"
                ),
                None => break,
            }
        }

        assert!(
            token.is_none(),
            "case {case}: the walk must end with no nextToken, got {token:?}"
        );
        assert_eq!(
            seen, expected,
            "case {case}: expected every build exactly once in order, got {seen:?}"
        );
    }

    // Rows span each filter the operation publishes, alone and composed, with a value
    // that matches every build, one that matches a single target, and one that matches
    // none.
    let (one_chipset, one_generation) = targets[0];
    let filters: Vec<(&str, String, usize)> = vec![
        ("the only architecture modelled", "architecture=ARM_64".to_string(), expected.len()),
        ("the only chipset modelled", format!("chipset={one_chipset}"), expected.len()),
        (
            "one chipset generation",
            format!("chipsetGeneration={one_generation}"),
            1,
        ),
        (
            "every filter composed",
            format!("architecture=ARM_64&chipset={one_chipset}&chipsetGeneration={one_generation}"),
            1,
        ),
        (
            "a chipset generation nothing was built for",
            "chipsetGeneration=GRAVITON_NONE".to_string(),
            0,
        ),
        (
            "a matching architecture and a generation nothing was built for",
            "architecture=ARM_64&chipsetGeneration=GRAVITON_NONE".to_string(),
            0,
        ),
    ];

    for (case, query, matches) in filters {
        let (status, body) =
            list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {query} expected 200, got {status} {body}"
        );
        let page = page_build_ids(&body, case);
        assert_eq!(
            page.len(),
            matches,
            "case {case:?}: expected {matches} matching builds, got {page:?}"
        );
        for id in &page {
            assert!(
                expected.contains(id),
                "case {case:?}: a filtered page serves builds of this version, got {id}"
            );
        }
        if matches == 0 {
            assert!(
                body["nextToken"].is_null(),
                "case {case:?}: an empty page issues no nextToken, got {:?}",
                body["nextToken"]
            );
        }
    }
}

/// RULE: a `nextToken` this listing did not issue positions nothing, so it is refused
/// with HTTP 400 `ValidationException` rather than silently restarting the listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let router = emulator();
    let image_arn = create_image(&router, "token-image", json!({})).await;

    // Rows differ in HOW the token fails to be one this listing issued: not the opaque
    // encoding at all, the encoding carrying no cursor of this listing, and another
    // listing's token.
    let rows: Vec<(&str, String)> = vec![
        ("not the opaque encoding", "not-a-token".to_string()),
        ("an empty encoded value", hex_token("")),
        (
            "an encoded cursor with no marker",
            hex_token("build-0000000000000000"),
        ),
        (
            "another listing's token",
            hex_token("ListManagedMicrovmImageVersions/1:1.0"),
        ),
        (
            "a token of a marker this listing never issued",
            hex_token("ListMicrovmImageBuilds/0:build-0000000000000000"),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = list_builds(
            &router,
            &image_arn,
            FIRST_IMAGE_VERSION,
            &format!("nextToken={token}"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: nextToken={token} must be refused, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("nextToken is not a pagination token this service issued"),
            "case {case:?}: expected the refusal to name the token member, got {:?}",
            body["message"]
        );
    }
}
