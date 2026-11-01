// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListManagedMicrovmImageVersions`: the versions the
//! AWS-MANAGED base-image catalogue publishes for one of its images.
//!
//! RULE: the listing reports the versions the catalogue publishes for the managed image
//! the `imageIdentifier` names — resolved by either form `MicrovmImageIdentifier`
//! accepts — each carrying that image's AWS-owned ARN in the region the request was
//! signed for, its version, its lifecycle status and the instants the entry was
//! published, NEWEST generation first, paged by the operation's `@paginated` contract; a
//! well-formed reference the catalogue does not publish has an empty published version
//! set and so pages through to an empty page, and a value that is no identifier of this
//! API is HTTP 400.
//!
//! The form of a published generation's id, the set of generations one base image
//! publishes and the architectures each supports are the subject of
//! `managed_base_image_generations`.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    invalid_arn_format_message, managed_base_image_arn, published_managed_base_image_versions,
    MANAGED_BASE_IMAGE_IDS, MAX_LIST_PAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{
    as_uri_value, call, call_in_region, emulator, expected_image_arn, image_body, REGION,
};

/// `GET /2025-09-09/managed-microvm-images/{imageIdentifier}/versions`, with whatever
/// `query` adds to the query string, signed for `region`.
async fn list_versions_in_region(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    query: &str,
) -> (u16, Value) {
    let path = format!(
        "/2025-09-09/managed-microvm-images/{}/versions",
        as_uri_value(identifier)
    );
    let uri = match query.is_empty() {
        true => path,
        false => format!("{path}?{query}"),
    };
    call_in_region(router, region, "GET", &uri, None).await
}

/// The same listing, signed for the suite's own region.
async fn list_versions(
    router: &MultiServiceRouter,
    identifier: &str,
    query: &str,
) -> (u16, Value) {
    list_versions_in_region(router, REGION, identifier, query).await
}

/// The `items` of one page.
fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// The `imageVersion`s of one page, in the order the page carries them.
fn page_versions(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| {
            item["imageVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A hex-encoded `nextToken` candidate, as the opaque token form this listing issues.
fn hex_token(value: &str) -> String {
    ::hex::encode(value.as_bytes())
}

/// RULE: for every image the catalogue publishes, and however the pages are cut, the
/// listing walks that image's whole published version set — every version exactly once,
/// in one stable order, at most `maxResults` per page — and hands back a `nextToken`
/// only while versions remain.
#[tokio::test]
async fn every_published_version_is_served_once_over_every_page_size() {
    let router = emulator();

    // The catalogue ids and the versions each publishes are the service's own published
    // set (`state::MANAGED_BASE_IMAGE_IDS`,
    // `state::published_managed_base_image_versions`).
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let expected = published_managed_base_image_versions(image_id);
        assert!(
            !expected.is_empty(),
            "case {image_id:?}: a published base image publishes at least one version, got \
             {expected:?}"
        );

        // Rows differ in the identifier form and the page size, which is what the walk
        // generalises over: the catalogue id and the AWS-owned ARN, with no `maxResults`
        // at all, each published bound of `@range(min: 1, max: 50)`, and every size up
        // to the version set's own length.
        let identifiers = [
            ("the catalogue id", (*image_id).to_string()),
            ("the managed ARN", managed_base_image_arn(REGION, image_id)),
        ];
        let mut sizes: Vec<Option<usize>> = vec![None, Some(1), Some(MAX_LIST_PAGE)];
        sizes.extend((2..=expected.len()).map(Some));

        for (form, identifier) in &identifiers {
            for size in &sizes {
                let case = format!("{form} {identifier:?} maxResults={size:?}");
                let cap = size.unwrap_or(MAX_LIST_PAGE);
                let mut seen: Vec<String> = Vec::new();
                let mut token: Option<String> = None;

                // One page per version, plus one: a listing that never terminates fails
                // here rather than hanging.
                for _ in 0..=expected.len() {
                    let mut query: Vec<String> = Vec::new();
                    if let Some(size) = size {
                        query.push(format!("maxResults={size}"));
                    }
                    if let Some(token) = &token {
                        query.push(format!("nextToken={token}"));
                    }
                    let (status, body) =
                        list_versions(&router, identifier, &query.join("&")).await;
                    assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

                    let page = page_versions(&body, &case);
                    assert!(
                        page.len() <= cap,
                        "case {case}: a page carries at most {cap} versions, got {} ({page:?})",
                        page.len()
                    );
                    for item in page_items(&body, &case) {
                        assert_eq!(
                            item["imageArn"].as_str(),
                            Some(managed_base_image_arn(REGION, image_id).as_str()),
                            "case {case}: every version carries its image's ARN, got {:?}",
                            item["imageArn"]
                        );
                        // `AVAILABLE` is "the version is available for use"
                        // (`smithy_query(query="shape",
                        // shape="ManagedMicrovmImageVersionStatus")`), which is what a
                        // published version is.
                        assert_eq!(
                            item["status"].as_str(),
                            Some("AVAILABLE"),
                            "case {case}: a published version is available for use, got {:?}",
                            item["status"]
                        );
                        let created_at = item["createdAt"].as_f64();
                        assert!(
                            created_at.is_some_and(|at| at > 0.0),
                            "case {case}: every version carries the instant it was published, \
                             got {:?}",
                            item["createdAt"]
                        );
                        assert_eq!(
                            item["updatedAt"].as_f64(),
                            created_at,
                            "case {case}: nothing has written the version since it was \
                             published, got createdAt {:?} updatedAt {:?}",
                            item["createdAt"],
                            item["updatedAt"]
                        );
                    }
                    seen.extend(page);
                    assert!(
                        seen.len() <= expected.len(),
                        "case {case}: the walk served more than the image publishes, got {seen:?}"
                    );

                    token = body["nextToken"].as_str().map(str::to_string);
                    match &token {
                        Some(issued) => assert!(
                            seen.len() < expected.len(),
                            "case {case}: a nextToken ({issued}) is due only while versions \
                             remain, got it after {seen:?}"
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
                    "case {case}: expected the published versions exactly once in order, got \
                     {seen:?}"
                );
            }
        }
    }
}

/// RULE: a managed base image is an AWS-owned REGIONAL resource, so every region
/// publishes the same versions and each ARN the listing reports carries the region the
/// request was signed for.
#[tokio::test]
async fn the_versions_carry_the_region_the_request_was_signed_for() {
    let router = emulator();
    let image_id = MANAGED_BASE_IMAGE_IDS[0];
    let expected = published_managed_base_image_versions(image_id);

    // Rows span regions in several partitions' naming forms; only the ARN's region field
    // may differ between them.
    for region in [REGION, "us-east-1", "eu-central-1", "ap-southeast-2"] {
        let (status, body) = list_versions_in_region(&router, region, image_id, "").await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            page_versions(&body, region),
            expected,
            "case {region:?}: every region publishes the same versions, got {body}"
        );
        for item in page_items(&body, region) {
            assert_eq!(
                item["imageArn"].as_str(),
                Some(managed_base_image_arn(region, image_id).as_str()),
                "case {region:?}: the ARN carries the region the request named, got {:?}",
                item["imageArn"]
            );
        }
    }
}

/// RULE: the identifier is resolved against the PUBLISHED catalogue — the version set of
/// a well-formed reference it does not publish (an account's own image included) is
/// EMPTY, so the listing is a terminal empty page rather than a refusal (`items` is
/// `@required` on this `@paginated` listing), and only a value that is neither a
/// published id nor an ARN is HTTP 400 `ValidationException`.
#[tokio::test]
async fn an_identifier_the_catalogue_does_not_publish_pages_empty() {
    let router = emulator();

    // An image this ACCOUNT holds: it is addressable through the customer API, and is
    // still no entry of AWS's own catalogue.
    let own = "mine";
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(own, json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create the account's own image: got {created}");

    // Rows differ in HOW the reference fails to name a published managed version set:
    // the right ARN form with an unpublished id, another service's ARN, an account's own
    // image ARN, and values that are no identifier of this API at all.
    let rows: Vec<(&str, String, u16, Option<String>)> = vec![
        (
            "a managed ARN the catalogue does not publish",
            managed_base_image_arn(REGION, "not-a-published-base-image"),
            200,
            None,
        ),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:aws:image:al2023-1"),
            200,
            None,
        ),
        (
            "the account's own image ARN",
            expected_image_arn(own),
            200,
            None,
        ),
        (
            "the account's own image name",
            own.to_string(),
            400,
            Some(invalid_arn_format_message(own)),
        ),
        (
            "no identifier of this API at all",
            "not-an-identifier".to_string(),
            400,
            Some(invalid_arn_format_message("not-an-identifier")),
        ),
    ];

    for (case, identifier, expected_status, expected_message) in rows {
        let (status, body) = list_versions(&router, &identifier, "").await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {identifier:?} expected {expected_status}, got {status} {body}"
        );
        match expected_message {
            None => {
                assert_eq!(
                    page_items(&body, case),
                    Vec::<Value>::new(),
                    "case {case:?}: the catalogue publishes no version of it, so the page is \
                     empty, got {body}"
                );
                assert!(
                    body["nextToken"].is_null(),
                    "case {case:?}: an empty page is terminal, got {:?}",
                    body["nextToken"]
                );
            }
            Some(expected_message) => {
                assert_eq!(
                    body["__type"].as_str(),
                    Some("ValidationException"),
                    "case {case:?}: expected ValidationException, got {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the refusal built from the identifier the request \
                     carried, got {:?}",
                    body["message"]
                );
            }
        }
    }
}

/// RULE: `maxResults` is bounded by the range the model publishes, so each bound is
/// served and a value one past either bound is refused with HTTP 400
/// `ValidationException` — `maxResults` is `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListManagedMicrovmImageVersions")`).
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    let image_id = MANAGED_BASE_IMAGE_IDS[0];

    let rows: Vec<(&str, usize, u16)> = vec![
        ("one below the published minimum", 0, 400),
        ("the published minimum", 1, 200),
        ("the published maximum", MAX_LIST_PAGE, 200),
        ("one past the published maximum", MAX_LIST_PAGE + 1, 400),
    ];

    for (case, max_results, expected_status) in rows {
        let (status, body) =
            list_versions(&router, image_id, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: maxResults={max_results} expected {expected_status}, got {status} \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: a `nextToken` this listing did not issue positions nothing, so it is refused
/// with HTTP 400 `ValidationException` rather than silently restarting the listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let router = emulator();
    let image_id = MANAGED_BASE_IMAGE_IDS[0];

    // Rows differ in HOW the token fails to be one this listing issued: not the opaque
    // encoding at all, the encoding carrying no cursor of this listing, and another
    // listing's token.
    let rows: Vec<(&str, String)> = vec![
        ("not the opaque encoding", "not-a-token".to_string()),
        ("an empty encoded value", hex_token("")),
        (
            "an encoded cursor with no marker",
            hex_token(published_managed_base_image_versions(image_id)[0]),
        ),
        (
            "another listing's token",
            hex_token("ListManagedMicrovmImages/1:al2023-1"),
        ),
        (
            "a token of a marker this listing never issued",
            hex_token("ListManagedMicrovmImageVersions/0:1.0"),
        ),
    ];

    for (case, token) in rows {
        let (status, body) = list_versions(&router, image_id, &format!("nextToken={token}")).await;
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

/// RULE: the version listing is the version side of the ONE managed catalogue — every
/// generation it reports for an image is one a build may PIN, and the newest is the
/// FIRST row it serves — and reading it changes nothing a later caller sees. Which
/// generation an unpinned build records is the subject of
/// `managed_base_image_generations`.
#[tokio::test]
async fn every_listed_version_is_one_a_build_can_pin() {
    let router = emulator();

    // Rows: one per image the catalogue publishes, read through both identifier forms,
    // so the coherence holds however the image is addressed.
    for image_id in MANAGED_BASE_IMAGE_IDS {
        let base_image_arn = managed_base_image_arn(REGION, image_id);
        for (index, identifier) in [(*image_id).to_string(), base_image_arn.clone()]
            .into_iter()
            .enumerate()
        {
            let case = format!("{identifier:?}");
            let (status, first) = list_versions(&router, &identifier, "").await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} {first}");
            let listed = page_versions(&first, &case);
            // The catalogue publishes a base image's generations newest first
            // (`state::published_managed_base_image_versions`), so the newest is the
            // row the listing serves first.
            let newest = listed
                .first()
                .unwrap_or_else(|| panic!("case {case}: the image publishes a version, got {first}"))
                .clone();

            let name = format!("resolved-{image_id}-{index}");
            let (status, created) = call(
                &router,
                "POST",
                "/2025-09-09/microvm-images",
                Some(image_body(
                    &name,
                    json!({
                        "baseImageArn": base_image_arn.clone(),
                        "baseImageVersion": newest.clone(),
                    }),
                )),
            )
            .await;
            assert_eq!(status, 201, "case {case}: create got {status} {created}");
            assert_eq!(
                created["baseImageVersion"].as_str(),
                Some(newest.as_str()),
                "case {case}: a build pins the generation it named, got {:?}",
                created["baseImageVersion"]
            );

            // The listing is a pure report: creating against the catalogue, and reading
            // it again, answers exactly what the first read answered.
            let (status, again) = list_versions(&router, &identifier, "").await;
            assert_eq!(status, 200, "case {case}: re-list got {status} {again}");
            assert_eq!(
                again, first,
                "case {case}: the catalogue is AWS's own and a read does not change it, got \
                 {again}"
            );
        }
    }
}
