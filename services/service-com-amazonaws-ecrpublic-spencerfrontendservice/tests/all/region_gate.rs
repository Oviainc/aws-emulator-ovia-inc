// SPDX-License-Identifier: CC0-1.0

//! Which ECR Public calls depend on a Region, driven end to end over the wire.
//!
//! The rule under test has two halves. First, every operation of this service EXCEPT the
//! one Region-scoped write is served whatever Region its request names: this emulator
//! publishes one endpoint, a client points at it with whatever Region its own configuration
//! carries, and AWS serves that surface from either published endpoint, so the Region in a
//! request's SigV4 credential scope decides nothing for them. Second, the Region-scoped
//! call — the write of the account-global registry catalog data, which AWS performs in a
//! single Region (`state::API_REGION`) — is served only for that Region: a request signed
//! for any other is answered with the service-common `UnsupportedCommandException`
//! (HTTP 400) naming the operation and the Region that performs it, before the record is
//! touched.
//!
//! `UnsupportedCommandException` ("The action isn't supported in this Region", client,
//! httpError 400) is declared service-wide rather than per operation —
//! `smithy_query(query="digest")` reports it under `common_errors`, so the model allows any
//! of the 23 operations to be refused for a Region and fixes which ones ARE nowhere; what
//! the service does is what this suite pins.
//!
//! The tables below span the class each half generalises over: EVERY implemented operation
//! (a registry read and write, a repository create, read and delete, both policy legs, both
//! catalog legs, the tag read, the push, and all three layer-upload legs) crossed with
//! Regions from several partitions and every Region ECR Public publishes an endpoint in,
//! each in three letter cases; and, for the Region-scoped write, every one of those Regions
//! crossed with the operation names the refusal's wording is built from.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{
    serves_region, unsupported_for_region, API_REGION, ENDPOINT_REGIONS,
};

use crate::repository_image_lifecycle::{call_signed_for, ok, service, ARN_PREFIX};

/// Regions no ECR Public endpoint is published in, one per shape an AWS Region name takes
/// (another `us-` Region, Europe, Asia Pacific, and the China and GovCloud partitions).
pub(crate) const UNSERVED_REGIONS: &[&str] = &[
    "us-east-2",
    "eu-west-1",
    "ap-southeast-2",
    "cn-north-1",
    "us-gov-west-1",
];

/// One Region name in the three letter cases it can arrive in: a Region name is
/// case-insensitive, so all of them name the same Region.
pub(crate) fn region_spellings(region: &str) -> Vec<String> {
    let mut capitalized = region.to_string();
    capitalized[..1].make_ascii_uppercase();
    vec![region.to_string(), region.to_uppercase(), capitalized]
}

/// Every Region a caller can reach this endpoint as: the Regions ECR Public publishes an
/// endpoint in, plus Regions that publish none.
pub(crate) fn every_region() -> Vec<&'static str> {
    ENDPOINT_REGIONS
        .iter()
        .copied()
        .chain(UNSERVED_REGIONS.iter().copied())
        .collect()
}

/// The same set in every letter case a Region name can arrive in.
pub(crate) fn every_region_spelling() -> Vec<String> {
    every_region().into_iter().flat_map(region_spellings).collect()
}

/// The one operation whose Region is part of its contract.
pub(crate) const REGION_SCOPED_OPERATION: &str = "PutRegistryCatalogData";

/// The refusal message a Region-scoped call is answered with: fixed wording whose variable
/// parts are the operation the caller invoked and the Region this instance performs that
/// call in.
pub(crate) fn expected_refusal_message(operation: &str, serving_region: &str) -> String {
    format!("{operation} command is only supported in {serving_region}.")
}

/// An `uploadId` with the layout the model gives `UploadId`
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`) that
/// this registry never minted.
const UPLOAD_ID: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// A `sha256:<hex>` digest in the form the model's `LayerDigest` declares.
fn a_layer_digest() -> String {
    format!("sha256:{}", "1".repeat(64))
}

/// Every IMPLEMENTED operation, with a request body that satisfies its modelled
/// constraints, so the boundary validator passes it and the handler's own answer is what
/// comes back. The repository and upload the bodies name exist nowhere: each operation's own
/// checks are what must answer, never a Region refusal.
fn operations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "BatchCheckLayerAvailability",
            json!({"repositoryName": "gate-probe", "layerDigests": [a_layer_digest()]}),
        ),
        (
            "BatchDeleteImage",
            json!({"repositoryName": "gate-probe", "imageIds": [{"imageTag": "v1"}]}),
        ),
        (
            "CompleteLayerUpload",
            json!({
                "repositoryName": "gate-probe",
                "uploadId": UPLOAD_ID,
                "layerDigests": [a_layer_digest()],
            }),
        ),
        ("CreateRepository", json!({"repositoryName": "gate-created"})),
        ("DeleteRepository", json!({"repositoryName": "gate-probe"})),
        (
            "DeleteRepositoryPolicy",
            json!({"repositoryName": "gate-probe"}),
        ),
        ("DescribeImages", json!({"repositoryName": "gate-probe"})),
        ("DescribeImageTags", json!({"repositoryName": "gate-probe"})),
        ("DescribeRegistries", json!({})),
        ("DescribeRepositories", json!({})),
        ("GetAuthorizationToken", json!({})),
        ("GetRegistryCatalogData", json!({})),
        (
            "GetRepositoryCatalogData",
            json!({"repositoryName": "gate-probe"}),
        ),
        (
            "GetRepositoryPolicy",
            json!({"repositoryName": "gate-probe"}),
        ),
        (
            "InitiateLayerUpload",
            json!({"repositoryName": "gate-probe"}),
        ),
        (
            "ListTagsForResource",
            json!({"resourceArn": format!("{ARN_PREFIX}gate-probe")}),
        ),
        (
            "PutImage",
            json!({"repositoryName": "gate-probe", "imageManifest": "{\"schemaVersion\":2}"}),
        ),
        (
            REGION_SCOPED_OPERATION,
            json!({"displayName": "written from this endpoint"}),
        ),
        (
            "PutRepositoryCatalogData",
            json!({"repositoryName": "gate-probe", "catalogData": {"description": "d"}}),
        ),
        (
            "SetRepositoryPolicy",
            json!({"repositoryName": "gate-probe", "policyText": "{}"}),
        ),
        (
            "UploadLayerPart",
            json!({
                "repositoryName": "gate-probe",
                "uploadId": UPLOAD_ID,
                "partFirstByte": 0,
                "partLastByte": 0,
                // awsJson1_1 carries a blob as base64 of its bytes.
                "layerPartBlob": aws_smithy_types::base64::encode([7u8]),
            }),
        ),
    ]
}

#[tokio::test]
async fn every_operation_but_the_region_scoped_write_is_served_whatever_region_it_names() {
    let router = service();

    // "Served" is the claim under test, not "succeeds": a body naming a repository that
    // does not exist is refused by the operation's own checks, which is a DIFFERENT error
    // from the Region one. The Region field is compared without ASCII case, so a Region in
    // capitals is the same Region.
    for region in every_region_spelling() {
        let region = region.as_str();
        for (operation, body) in operations() {
            if operation == REGION_SCOPED_OPERATION {
                continue;
            }
            let case = format!("{operation} signed for {region}");
            let (status, parsed) = call_signed_for(&router, operation, body, region).await;
            let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
            assert!(
                !wire_type.contains("UnsupportedCommandException"),
                "case {case:?}: an operation AWS serves from every published endpoint is \
                 never refused for its Region, got {status} with {parsed}"
            );
            assert!(
                status == 200 || status == 400,
                "case {case:?}: expected the operation's own answer (200 or a modelled \
                 400), got {status} with {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn the_region_scoped_call_is_served_for_its_own_region_and_refused_for_every_other() {
    // The decision function the Region-scoped handler calls, over every Region a caller can
    // sign for, in every letter case: the one Region AWS performs the write in is served
    // and every other is refused — another published endpoint of this service or a Region
    // that publishes none alike.
    for reached_as in every_region_spelling() {
        let reached_as = reached_as.as_str();
        let same_region = reached_as.eq_ignore_ascii_case(API_REGION);
        let case = format!("signed for {reached_as}");

        let served = serves_region(reached_as);
        assert_eq!(
            served, same_region,
            "case {case:?}: expected served={same_region}, got {served}"
        );

        // The refusal's wording is built from the operation the caller named and the
        // Region that performs it — so it is asserted over several operation names as
        // well as every Region.
        for operation in [
            REGION_SCOPED_OPERATION,
            "GetRegistryCatalogData",
            "CreateRepository",
        ] {
            let case = format!("{case}, calling {operation}");
            let refusal = unsupported_for_region(reached_as, operation);
            assert_eq!(
                refusal.is_none(),
                same_region,
                "case {case:?}: expected refused={}, got {refusal:?}",
                !same_region
            );
            if let Some(refusal) = refusal {
                let expected = expected_refusal_message(operation, API_REGION);
                assert_eq!(
                    refusal.message.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {expected:?}, got {:?}",
                    refusal.message
                );
            }
        }
    }
}

#[tokio::test]
async fn a_request_that_names_no_region_has_nothing_to_be_refused_for() {
    // The Region a call carries lives in its SigV4 credential scope and nowhere else, so
    // the rule is stated over that header. This service speaks awsJson1_1, whose requests
    // route by the scope's service field (`emulator_server::resolve_route_key`), so a
    // request carrying no scope never reaches a handler over the wire — the rule is
    // asserted here against the check the Region-scoped handler calls.
    //
    // (case, the `Authorization` header to send, whether the call is refused for its Region)
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("no Authorization header at all", None, false),
        (
            "a credential scope for the Region AWS performs the write in",
            Some(
                "AWS4-HMAC-SHA256 Credential=AKID/20260101/us-east-1/ecr-public/aws4_request, \
                 SignedHeaders=host, Signature=00",
            ),
            false,
        ),
        (
            "a credential scope for the other Region ECR Public publishes an endpoint in",
            Some(
                "AWS4-HMAC-SHA256 Credential=AKID/20260101/us-west-2/ecr-public/aws4_request, \
                 SignedHeaders=host, Signature=00",
            ),
            true,
        ),
        (
            "a credential scope for a Region this service publishes no endpoint in",
            Some(
                "AWS4-HMAC-SHA256 Credential=AKID/20260101/eu-central-1/ecr-public/aws4_request, \
                 SignedHeaders=host, Signature=00",
            ),
            true,
        ),
        (
            "an Authorization header that is not SigV4",
            Some("Basic dXNlcjpwYXNz"),
            false,
        ),
        (
            "a SigV4 header whose credential scope has too few fields",
            Some("AWS4-HMAC-SHA256 Credential=AKID/20260101/eu-central-1, Signature=00"),
            false,
        ),
    ];

    for (case, authorization, refused) in rows {
        let mut builder = http::Request::builder().method("POST").uri("/");
        if let Some(authorization) = authorization {
            builder = builder.header("authorization", authorization);
        }
        let parts = builder
            .body(())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let refusal = service_com_amazonaws_ecrpublic_spencerfrontendservice::state::unsupported_in_region(
            &parts,
            REGION_SCOPED_OPERATION,
        );
        assert_eq!(
            refusal.is_some(),
            refused,
            "case {case:?}: a request that names no Region has nothing to be refused for, \
             and one naming a Region other than the serving one is refused, got {refusal:?}"
        );
    }
}

#[tokio::test]
async fn a_repository_created_from_any_region_belongs_to_the_one_registry() {
    let router = service();
    let case = "repositories created from every Region a caller can sign for";

    // A repository create is not Region-scoped under any deployment, so one signed for any
    // Region creates a repository the single registry lists.
    let mut expected_names: Vec<String> = Vec::new();
    for region in every_region() {
        let name = format!("made-in-{region}");
        let (status, created) = call_signed_for(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a create signed for {region} is served, got {status} with {created}"
        );
        expected_names.push(name);
    }
    expected_names.sort();

    let listed = ok(&router, "DescribeRepositories", json!({}), case).await;
    let mut names: Vec<String> = listed["repositories"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a repositories array, got {listed}"))
        .iter()
        .map(|repository| {
            repository["repositoryName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    names.sort();
    assert_eq!(
        names, expected_names,
        "case {case:?}: every repository created from any Region exists, got {names:?}"
    );
}
