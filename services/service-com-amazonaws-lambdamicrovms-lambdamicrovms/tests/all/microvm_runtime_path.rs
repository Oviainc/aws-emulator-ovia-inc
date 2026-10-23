// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMicrovms`' filters and `RunMicrovm`'s active-version
//! precondition, driven over the wire through the same router the emulator boots.
//!
//! Each test states a RULE and exercises it over a SET of inputs; the values any one row
//! happens to carry are examples, not the specification. The harness (`emulator`, `call`,
//! `built_image`, …) is the one the lifecycle suite beside this file already defines.

use serde_json::{json, Value};

use super::microvm_lifecycle::{
    as_uri_value, built_image, call, emulator, expected_image_arn, image_body, run_microvm, ACCOUNT,
    PAST_THE_SPAN, REGION,
};

/// RULE: `imageIdentifier` / `imageVersion` are FILTERS on a list operation, so a filter
/// value that selects no MicroVM yields a terminal empty page — HTTP 200 with `items`
/// present as an empty list and no `nextToken` — never a refusal.
#[tokio::test]
async fn list_microvms_serves_an_empty_page_for_a_filter_that_selects_nothing() {
    let router = emulator();
    let populated_arn = built_image(&router, "populated").await;
    let empty_arn = built_image(&router, "unused-image").await;
    let (status, run) = run_microvm(&router, json!({"imageIdentifier": "populated"})).await;
    assert_eq!(status, 200, "seed MicroVM: got {status} {run}");
    let running_id = run["microvmId"].as_str().unwrap_or_default().to_string();

    // Rows differ along the filter member, the identifier FORM (name vs ARN), and why
    // the value selects nothing: no such image, an image that runs nothing, an ARN this
    // service never minted, a version nothing runs.
    let rows: Vec<(&str, String)> = vec![
        (
            "a name no image carries",
            "?imageIdentifier=absent".to_string(),
        ),
        (
            "the longest name the member admits",
            format!("?imageIdentifier={}", "n".repeat(256)),
        ),
        (
            "an ARN of an image that exists but runs no MicroVM",
            format!("?imageIdentifier={}", as_uri_value(&empty_arn)),
        ),
        (
            "an ARN of an image this service never minted",
            format!(
                "?imageIdentifier={}",
                as_uri_value(&expected_image_arn("never-created"))
            ),
        ),
        (
            "another account's ARN",
            format!(
                "?imageIdentifier={}",
                as_uri_value(&format!(
                    "arn:aws:lambda:{REGION}:999999999999:microvm-image:populated"
                ))
            ),
        ),
        (
            "another partition's ARN",
            format!(
                "?imageIdentifier={}",
                as_uri_value(&format!(
                    "arn:aws-cn:lambda:cn-north-1:{ACCOUNT}:microvm-image:populated"
                ))
            ),
        ),
        ("a version nothing runs", "?imageVersion=7.0".to_string()),
        (
            "a present image filtered to a version nothing runs",
            format!(
                "?imageIdentifier={}&imageVersion=7.0",
                as_uri_value(&populated_arn)
            ),
        ),
        (
            "an absent image and an absent version together",
            "?imageIdentifier=absent&imageVersion=7.0".to_string(),
        ),
        (
            "an absent image with a page size",
            "?imageIdentifier=absent&maxResults=1".to_string(),
        ),
    ];

    for (case, query) in &rows {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().cloned(),
            Some(Vec::new()),
            "case {case:?}: expected items to be present and empty, got {:?}",
            body["items"]
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: an empty page is terminal, got {:?}",
            body["nextToken"]
        );
    }

    // The control: the same filter member, given a value that DOES select, still selects.
    for (case, query) in [
        ("by name", "?imageIdentifier=populated".to_string()),
        (
            "by ARN",
            format!("?imageIdentifier={}", as_uri_value(&populated_arn)),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvms{query}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let ids: Vec<&str> = body["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["microvmId"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![running_id.as_str()],
            "case {case:?}: a selecting filter must still select, got {body}"
        );
    }
}

/// RULE: `RunMicrovm` resolves an ACTIVE image version before it mints anything, so a
/// launch for which no such version can be resolved — whatever form the identifier takes
/// and whether or not a version is named — is refused with the modelled 404
/// `ResourceNotFoundException`, and persists no MicroVM.
#[tokio::test]
async fn run_microvm_refuses_a_launch_with_no_active_image_version() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body("building", json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create image: got {status} {created}");
    let image_arn = created["imageArn"].as_str().unwrap_or_default().to_string();

    // Rows differ in the identifier form and in whether a version is named: the image's
    // only version is still building, so no ACTIVE version resolves for any of them.
    let refused: Vec<(&str, Value, String)> = vec![
        (
            "image by name, no version named",
            json!({"imageIdentifier": "building"}),
            format!("No active version found for MicroVM image {image_arn}"),
        ),
        (
            "image by ARN, no version named",
            json!({"imageIdentifier": image_arn.clone()}),
            format!("No active version found for MicroVM image {image_arn}"),
        ),
        (
            "an image this account does not hold",
            json!({"imageIdentifier": "never-created"}),
            "No active version found for MicroVM image never-created".to_string(),
        ),
        (
            "a version the image does not carry",
            json!({"imageIdentifier": "building", "imageVersion": "9.9"}),
            format!("MicroVM image {image_arn} has no version 9.9"),
        ),
    ];

    for (case, body, expected_message) in &refused {
        let (status, response) = run_microvm(&router, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response}"
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected {expected_message:?}, got {response}"
        );
        assert!(
            response["resourceType"].is_null() && response["resourceId"].is_null(),
            "case {case:?}: the optional members stay absent, got {response}"
        );
    }

    // Nothing was launched by any refused request.
    let (status, listed) = call(&router, "GET", "/2025-09-09/microvms", None).await;
    assert_eq!(status, 200, "list: got {status} {listed}");
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "a refused launch must persist no MicroVM, got {listed}"
    );

    // Once the build has made a version active, the same requests launch — and the
    // launch reports the version it resolved.
    tokio::time::sleep(PAST_THE_SPAN).await;
    for (case, body) in [
        ("image by name", json!({"imageIdentifier": "building"})),
        (
            "image by ARN",
            json!({"imageIdentifier": image_arn.clone()}),
        ),
    ] {
        let (status, response) = run_microvm(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once a version is active, got {status} {response}"
        );
        assert_eq!(
            response["imageVersion"].as_str(),
            Some("1.0"),
            "case {case:?}: the launch reports the resolved version, got {response}"
        );
    }
}
