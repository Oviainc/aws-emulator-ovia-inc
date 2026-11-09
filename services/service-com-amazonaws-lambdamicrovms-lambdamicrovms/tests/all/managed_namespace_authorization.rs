// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the authorization boundary around the AWS-OWNED MicroVM image
//! namespace, driven over the same router the emulator boots.
//!
//! RULE: an `imageIdentifier` whose ARN carries `aws` in its account field names a
//! resource AWS owns, so every customer-facing operation addressed at one answers the
//! modelled authorization refusal (HTTP 403 `AccessDeniedException`) BEFORE any
//! own-account lookup — the resource exists and the caller is simply not meant to reach
//! it through this API — while the managed operations that publish the catalogue keep
//! serving the very same ARNs.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    managed_base_image_arn, published_managed_base_image_versions, access_denied_message,
    MANAGED_BASE_IMAGE_IDS,
};

use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, PAST_THE_SPAN, REGION,
};

/// Every customer-facing operation this suite probes with one `imageIdentifier`, as
/// `(operation, method, URI, body)`.
fn customer_calls(identifier: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let path = format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier));
    vec![
        ("GetMicrovmImage", "GET", path.clone(), None),
        (
            "ListMicrovmImageVersions",
            "GET",
            format!("{path}/versions"),
            None,
        ),
        (
            "RunMicrovm",
            "POST",
            "/2025-09-09/microvms".to_string(),
            Some(json!({"imageIdentifier": identifier})),
        ),
        ("DeleteMicrovmImage", "DELETE", path, None),
    ]
}

/// The refusal every denied row must carry.
fn assert_denied(status: u16, body: &Value, case: &str) {
    assert_eq!(status, 403, "case {case}: expected 403, got {status} {body}");
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

/// RULE: every customer-facing operation addressed at an AWS-OWNED image ARN answers 403
/// `AccessDeniedException` — whether the catalogue publishes that ARN or not, and in
/// whatever region or partition it names, because the verdict is on the NAMESPACE and is
/// taken before any lookup.
#[tokio::test]
async fn a_customer_call_on_an_aws_owned_image_arn_is_denied() {
    let router = emulator();

    // An image this account DOES hold, so the denied rows are denied for the namespace
    // they name rather than for an empty store.
    let held = "held-image";
    let (status, created) = call(
        &router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(held, json!({}))),
    )
    .await;
    assert_eq!(status, 201, "create the held image: got {created}");

    // Rows differ in WHICH AWS-owned ARN the identifier is: one the catalogue publishes
    // (the set is the catalogue's own), one it does not, another region's and another
    // partition's — none of them may turn the verdict into a lookup.
    let mut identifiers: Vec<(String, String)> = MANAGED_BASE_IMAGE_IDS
        .iter()
        .map(|image_id| {
            (
                format!("a published managed image {image_id:?}"),
                managed_base_image_arn(REGION, image_id),
            )
        })
        .collect();
    identifiers.extend([
        (
            "a managed ARN the catalogue does not publish".to_string(),
            managed_base_image_arn(REGION, "no-such-base-image"),
        ),
        (
            "a managed ARN in another region".to_string(),
            managed_base_image_arn("eu-central-1", MANAGED_BASE_IMAGE_IDS[0]),
        ),
        (
            "a managed ARN in another partition".to_string(),
            format!(
                "arn:aws-us-gov:lambda:{REGION}:aws:microvm-image:{}",
                MANAGED_BASE_IMAGE_IDS[0]
            ),
        ),
    ]);

    for (namespace, identifier) in &identifiers {
        for (operation, method, uri, body) in customer_calls(identifier) {
            let case = format!("{operation} on {namespace} ({identifier})");
            let (status, response) = call(&router, method, &uri, body).await;
            assert_denied(status, &response, &case);
        }
    }

    // The positive control: this account's own image is served through every one of the
    // same paths, so the rows above are denied by the verdict and not by a dead path.
    let held_arn = expected_image_arn(held);
    tokio::time::sleep(PAST_THE_SPAN).await;
    for (operation, method, uri, body) in customer_calls(&held_arn) {
        let (status, response) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {operation}: this account's own image is served, got {status} {response}"
        );
    }
}

/// RULE: the refusal is an AUTHORIZATION verdict, not a report of absence — so the
/// managed operations that publish the catalogue keep serving the very ARNs the customer
/// API denies, and the version listing of an AWS-owned ARN the catalogue does NOT publish
/// is a terminal empty page rather than a refusal.
#[tokio::test]
async fn the_managed_catalogue_still_serves_what_the_customer_api_denies() {
    let router = emulator();

    let (status, catalogue) = call(
        &router,
        "GET",
        "/2025-09-09/managed-microvm-images",
        None,
    )
    .await;
    assert_eq!(status, 200, "list the catalogue: got {status} {catalogue}");
    let listed: Vec<String> = catalogue["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items is a list, got {catalogue}"))
        .iter()
        .map(|item| item["imageArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !listed.is_empty(),
        "the catalogue the rows are taken from is not empty, got {catalogue}"
    );

    // Rows: every ARN the catalogue publishes (each is denied through the customer API
    // and served through the managed one), plus one AWS-owned ARN in the same form that
    // the catalogue does not publish.
    let unpublished = managed_base_image_arn(REGION, "not-a-published-base-image");
    assert!(
        !listed.contains(&unpublished),
        "the unpublished row must name an id the catalogue does not publish, got {listed:?}"
    );
    let rows: Vec<(String, bool)> = listed
        .iter()
        .map(|arn| (arn.clone(), true))
        .chain([(unpublished, false)])
        .collect();

    for (identifier, published) in rows {
        let case = format!("{identifier:?} published={published}");

        // The customer API denies it whatever the catalogue holds.
        let (status, denied) = call(
            &router,
            "GET",
            &format!("/2025-09-09/microvm-images/{}", as_uri_value(&identifier)),
            None,
        )
        .await;
        assert_denied(status, &denied, &format!("GetMicrovmImage {case}"));

        // The managed version listing answers the versions the catalogue publishes for
        // it, which is an empty page for an ARN it does not publish.
        let (status, versions) = call(
            &router,
            "GET",
            &format!(
                "/2025-09-09/managed-microvm-images/{}/versions",
                as_uri_value(&identifier)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the managed listing serves its own namespace, got {status} {versions}"
        );
        let served: Vec<String> = versions["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: items is a list, got {versions}"))
            .iter()
            .map(|item| {
                item["imageVersion"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = match published {
            true => identifier
                .rsplit(':')
                .next()
                .map(published_managed_base_image_versions)
                .unwrap_or_default()
                .into_iter()
                .map(str::to_string)
                .collect(),
            false => Vec::new(),
        };
        assert_eq!(
            served, expected,
            "case {case}: expected the versions the catalogue publishes, got {versions}"
        );
    }
}
