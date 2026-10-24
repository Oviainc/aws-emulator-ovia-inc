// SPDX-License-Identifier: CC0-1.0

//! Which ECR Public calls are scoped to a Region, driven end to end over the wire.
//!
//! The rule under test: ECR Public publishes endpoints in more than one Region and serves
//! its registry and repository surface — repositories, images, layer uploads, tags,
//! policies and the catalog READS — from whichever of them a caller signed for, so none of
//! those calls may be refused for its Region. The one Region-scoped call is the write of
//! the account-global registry catalog data (`PutRegistryCatalogData`), which is accepted
//! only from `state::API_REGION` and refused from any other Region with the service-common
//! `UnsupportedCommandException` (HTTP 400) naming the operation — before the record is
//! touched, so the stored display name is left as the served Region wrote it.
//!
//! `UnsupportedCommandException` ("The action isn't supported in this Region", client,
//! httpError 400) is declared service-wide rather than per operation —
//! `smithy_query(query="digest")` reports it under `common_errors`, so the model allows any
//! of the 23 operations to be refused for a Region and fixes which ones ARE nowhere; what
//! the service does is what this suite pins.
//!
//! The tables below span the class each half of the rule generalises over: EVERY
//! implemented operation (a registry read and write, a repository create, read and delete,
//! both policy legs, both catalog legs, the tag read, the push, and all three layer-upload
//! legs) crossed with Regions from several partitions, plus the Region the catalog write is
//! served from in both letter cases and a request with no credential scope at all.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::API_REGION;
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{call_signed_for, ok, service, ARN_PREFIX};

/// Regions other than the one the registry catalog write is served from, one per shape an
/// AWS Region name takes (another `us-` Region, Europe, Asia Pacific, and the China and
/// GovCloud partitions).
const OTHER_REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-2",
    "eu-west-1",
    "ap-southeast-2",
    "cn-north-1",
    "us-gov-west-1",
];

/// The one operation whose Region is part of its contract.
const REGION_SCOPED_OPERATION: &str = "PutRegistryCatalogData";

/// An `uploadId` with the layout the model gives `UploadId`
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`) that
/// this registry never minted.
const UPLOAD_ID: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// A `sha256:<hex>` digest in the form the model's `LayerDigest` declares.
fn a_layer_digest() -> String {
    format!("sha256:{}", "1".repeat(64))
}

/// Every IMPLEMENTED operation that is NOT Region-scoped, with a request body that
/// satisfies its modelled constraints, so the boundary validator passes it and the
/// handler's own answer is what comes back. The repository and upload the bodies name
/// exist nowhere: each operation's own checks are what must answer, never a Region
/// refusal.
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

/// Assert `operation` signed for `region` is refused for its Region, and answer the body so
/// a caller can assert more about it.
async fn refused_for_its_region(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    region: &str,
) -> Value {
    let case = format!("{operation} signed for {region}");
    let (status, parsed) = call_signed_for(router, operation, body, region).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
    assert!(
        wire_type.contains("UnsupportedCommandException"),
        "case {case:?}: expected UnsupportedCommandException, got {wire_type:?} in {parsed}"
    );
    let actual = parsed["message"].as_str().unwrap_or_default().to_string();
    let expected = format!("{operation} command is only supported in {API_REGION}.");
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    parsed
}

#[tokio::test]
async fn every_operation_but_the_registry_catalog_write_is_served_whatever_region_it_names() {
    let router = service();

    // "Served" is the claim under test, not "succeeds": a body naming a repository that
    // does not exist is refused by the operation's own checks, which is a DIFFERENT error
    // from the Region one. The Region field is compared without ASCII case, so the served
    // Region in capitals is the same Region.
    let regions: Vec<&str> = OTHER_REGIONS
        .iter()
        .copied()
        .chain([API_REGION, "US-EAST-1", "Us-East-1"])
        .collect();
    for region in regions {
        for (operation, body) in operations() {
            let case = format!("{operation} signed for {region}");
            let (status, parsed) = call_signed_for(&router, operation, body, region).await;
            let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
            assert!(
                !wire_type.contains("UnsupportedCommandException"),
                "case {case:?}: this operation is served from every Region, got {status} \
                 with {parsed}"
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
async fn the_registry_catalog_write_is_refused_outside_the_region_it_is_served_from() {
    let router = service();

    for region in OTHER_REGIONS {
        refused_for_its_region(
            &router,
            REGION_SCOPED_OPERATION,
            json!({"displayName": format!("written from {region}")}),
            region,
        )
        .await;
    }

    // The same write from the Region it IS served from, in either letter case, is served.
    for region in [API_REGION, "US-EAST-1", "Us-East-1"] {
        let case = format!("the catalog write signed for {region}");
        let (status, parsed) = call_signed_for(
            &router,
            REGION_SCOPED_OPERATION,
            json!({"displayName": format!("written from {region}")}),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write to be served, got {status} with {parsed}"
        );
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
    // (case, the `Authorization` header to send, whether the call must be refused)
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("no Authorization header at all", None, false),
        (
            "a credential scope for the Region the catalog write is served from",
            Some(
                "AWS4-HMAC-SHA256 Credential=AKID/20260101/us-east-1/ecr-public/aws4_request, \
                 SignedHeaders=host, Signature=00",
            ),
            false,
        ),
        (
            "a credential scope for another Region",
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

    for (case, authorization, must_refuse) in rows {
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
            must_refuse,
            "case {case:?}: expected refused={must_refuse}, got {refusal:?}"
        );
        if let Some(refusal) = refusal {
            assert_eq!(
                refusal.message.as_deref(),
                Some(
                    format!(
                        "{REGION_SCOPED_OPERATION} command is only supported in {API_REGION}."
                    )
                    .as_str()
                ),
                "case {case:?}: the refusal names the operation and the served Region"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_catalog_write_changes_nothing_a_later_read_sees() {
    let router = service();
    let case = "a wrong-Region catalog write beside the served Region's state";

    // The registry's own catalog: written from the Region the write is served from, then
    // written again from every other Region. Each of those writes must be refused and the
    // stored display name must still be the one the served Region wrote — read back
    // through a client signed for a Region the READ is served from, which is any of them.
    ok(
        &router,
        REGION_SCOPED_OPERATION,
        json!({"displayName": "written from the api region"}),
        case,
    )
    .await;
    for region in OTHER_REGIONS {
        refused_for_its_region(
            &router,
            REGION_SCOPED_OPERATION,
            json!({"displayName": format!("written from {region}")}),
            region,
        )
        .await;
        let (status, got) =
            call_signed_for(&router, "GetRegistryCatalogData", json!({}), region).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the catalog read is served from {region}, got {status} with {got}"
        );
        assert_eq!(
            got["registryCatalogData"]["displayName"],
            json!("written from the api region"),
            "case {case:?}: a write signed for {region} must leave the catalog alone, \
             got {got}"
        );
    }

    // A repository create is not Region-scoped, so one signed for any Region creates a
    // repository the registry lists.
    let mut expected_names: Vec<String> = vec!["made-in-the-api-region".to_string()];
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "made-in-the-api-region"}),
        case,
    )
    .await;
    for region in OTHER_REGIONS {
        let name = format!("made-in-{region}");
        let (status, created) =
            call_signed_for(&router, "CreateRepository", json!({"repositoryName": name}), region)
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
