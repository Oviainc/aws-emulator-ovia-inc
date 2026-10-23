// SPDX-License-Identifier: CC0-1.0

//! The Region gate every ECR Public operation runs, driven end to end over the wire.
//!
//! The rule under test: ECR Public serves its API from ONE Region, so a call whose SigV4
//! credential scope names any other Region is refused with the service-common
//! `UnsupportedCommandException` (HTTP 400) naming the operation it refused, before the
//! call reads or changes anything — while a call signed for that one Region, or one that
//! carries no credential scope at all and so names no Region, is served.
//!
//! The source of the refusal's existence is the model: `UnsupportedCommandException`
//! ("The action isn't supported in this Region", client, httpError 400) is declared
//! service-wide rather than per operation — `smithy_query(query="digest")` reports it
//! under `common_errors`, so it is bound to all 23 operations — and the Region an
//! operation is served from is `state::API_REGION`.
//!
//! The tables below span the class the rule generalises over: EVERY implemented operation
//! (a registry read and write, a repository create, read and delete, both policy legs,
//! both catalog legs, the tag read, the push, and all three layer-upload legs) crossed
//! with Regions from several partitions, plus the Region the API is served from in both
//! letter cases and a request with no credential scope. The three operations that are
//! still scaffold stubs answer 501 and are excluded — they assert nothing about any
//! behaviour yet.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::API_REGION;
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{call_signed_for, ok, service, ARN_PREFIX};

/// Regions other than the one this API is served from, one per shape an AWS Region name
/// takes (another `us-` Region, Europe, Asia Pacific, and the China and GovCloud
/// partitions). Every one of them must be refused.
const OTHER_REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-2",
    "eu-west-1",
    "ap-southeast-2",
    "cn-north-1",
    "us-gov-west-1",
];

/// An `uploadId` with the layout the model gives `UploadId`
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`) that
/// this registry never minted: the gate must refuse before the upload is looked up.
const UPLOAD_ID: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// A `sha256:<hex>` digest in the form the model's `LayerDigest` declares.
fn a_layer_digest() -> String {
    format!("sha256:{}", "1".repeat(64))
}

/// Every IMPLEMENTED operation with a request body that satisfies its modelled
/// constraints, so the boundary validator passes it and the handler's own gate is what
/// answers. The repository and upload the bodies name exist nowhere: the gate runs before
/// any existence check, so the refusal must still be the Region one.
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
            "PutRegistryCatalogData",
            json!({"displayName": "should-not-apply"}),
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

/// Assert `operation` signed for `region` is refused by the Region gate, and answer the
/// body so a caller can assert more about it.
async fn refused_by_the_gate(
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
async fn every_operation_signed_for_another_region_is_refused_with_the_modelled_error() {
    let router = service();

    for (operation, body) in operations() {
        for region in OTHER_REGIONS {
            refused_by_the_gate(&router, operation, body.clone(), region).await;
        }
    }
}

#[tokio::test]
async fn every_operation_signed_for_the_api_region_passes_the_gate() {
    let router = service();

    // The Region field is compared without ASCII case, so the same Region in capitals is
    // the same Region. "Passes the gate" is the claim under test, not "succeeds": a body
    // naming a repository that does not exist is then refused by the operation's own
    // checks, which is a DIFFERENT error from the Region one.
    for region in [API_REGION, "US-EAST-1", "Us-East-1"] {
        for (operation, body) in operations() {
            let case = format!("{operation} signed for {region}");
            let (status, parsed) = call_signed_for(&router, operation, body, region).await;
            let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
            assert!(
                !wire_type.contains("UnsupportedCommandException"),
                "case {case:?}: the Region gate must not fire, got {status} with {parsed}"
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
async fn a_request_that_names_no_region_has_nothing_to_be_refused_for() {
    // The Region a call carries lives in its SigV4 credential scope and nowhere else, so
    // the gate's own rule is stated over that header. This service speaks awsJson1_1,
    // whose requests route by the scope's service field
    // (`emulator_server::resolve_route_key`), so a request carrying no scope never
    // reaches a handler over the wire — the rule is asserted here against the gate the
    // handlers call.
    //
    // (case, the `Authorization` header to send, whether the gate must refuse)
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("no Authorization header at all", None, false),
        (
            "a credential scope for the Region the API is served from",
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
            "GetRegistryCatalogData",
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
                    format!("GetRegistryCatalogData command is only supported in {API_REGION}.")
                        .as_str()
                ),
                "case {case:?}: the refusal names the operation and the served Region"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_write_changes_nothing_a_later_read_sees() {
    let router = service();
    let case = "a wrong-Region write beside the served Region's state";

    // The registry's own catalog: written from the Region the API is served from, then
    // written again from every other Region. Each of those writes must be refused and the
    // stored display name must still be the one the served Region wrote.
    ok(
        &router,
        "PutRegistryCatalogData",
        json!({"displayName": "written from the api region"}),
        case,
    )
    .await;
    for region in OTHER_REGIONS {
        refused_by_the_gate(
            &router,
            "PutRegistryCatalogData",
            json!({"displayName": format!("written from {region}")}),
            region,
        )
        .await;
        let got = ok(&router, "GetRegistryCatalogData", json!({}), case).await;
        assert_eq!(
            got["registryCatalogData"]["displayName"],
            json!("written from the api region"),
            "case {case:?}: a write signed for {region} must leave the catalog alone, \
             got {got}"
        );
    }

    // A repository create refused by the gate creates nothing, so the registry still
    // lists only what the served Region created.
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "made-in-the-api-region"}),
        case,
    )
    .await;
    for region in OTHER_REGIONS {
        refused_by_the_gate(
            &router,
            "CreateRepository",
            json!({"repositoryName": format!("made-in-{region}")}),
            region,
        )
        .await;
    }
    let listed = ok(&router, "DescribeRepositories", json!({}), case).await;
    let names: Vec<String> = listed["repositories"]
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
    assert_eq!(
        names,
        vec!["made-in-the-api-region".to_string()],
        "case {case:?}: only the repository created from the served Region exists, \
         got {names:?}"
    );
}
