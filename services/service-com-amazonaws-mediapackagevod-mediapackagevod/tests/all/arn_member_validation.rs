// SPDX-License-Identifier: CC0-1.0

//! A request refused for an ARN-documented member leaves no trace in ANY projection.
//!
//! `arn_documented_members.rs` states the ARN rule itself and probes the refused
//! resource's own `Describe`. This suite pins the same "nothing was stored" half of
//! the rule across the LISTINGS, which is where a partially applied write shows up
//! even when the describe of one id does not: `ListPackagingGroups`, `ListAssets` and
//! `ListPackagingConfigurations` must answer byte-identically before and after every
//! refusal, so no refused create leaves a half-built record, no refused update
//! disturbs a stored one, and no derived projection (`ApproximateAssetCount`, the
//! resolved package block) moves.
//!
//! The table covers one refusal per ARN-documented path of this service — the CDN
//! authorization of a create and of an update, `CreateAsset`'s two members, and the
//! SPEKE role of each of the four packaging formats — and the listings are taken over
//! a store that already holds a group, an asset and a packaging configuration, so an
//! unchanged listing is a non-trivial fact.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagevod_mediapackagevod as vod;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The group the stored resources belong to and the update rows address.
const GROUP: &str = "vod-group";

/// A well-formed S3 object ARN, for the members a row is not varying.
const GOOD_S3_OBJECT: &str = "arn:aws:s3:::test-bucket/asset.smil";

/// A well-formed IAM role ARN, for the members a row is not varying.
const GOOD_IAM_ROLE: &str = "arn:aws:iam::111122223333:role/MediaPackageVod";

/// A well-formed Secrets Manager secret ARN, for the members a row is not varying.
const GOOD_SECRET: &str = "arn:aws:secretsmanager:us-west-2:111122223333:secret:cdn-AbCdEf";

/// The modelled refusal of an invalid request parameter: error code and `httpError`
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// A router carrying exactly this service over one shared engine.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(vod::EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));
    router
}

/// Send one request and report its status, the `x-amzn-errortype` header and the
/// parsed body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, errortype, parsed)
}

/// A SPEKE key provider naming `role_arn`. `SystemIds` and `Url` are required members
/// of the shape; the system id is Widevine's published DRM system id.
fn speke(role_arn: &str) -> Value {
    json!({
        "roleArn": role_arn,
        "systemIds": ["edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"],
        "url": "https://speke.example.com/v1/copyProtection",
    })
}

/// The package block of `format` carrying an encryption block whose SPEKE role is
/// `role_arn`. The manifest list each format requires is the only other member.
fn encrypted_package(format: &str, role_arn: &str) -> Value {
    let encryption = json!({ "spekeKeyProvider": speke(role_arn) });
    match format {
        "hlsPackage" | "cmafPackage" => {
            json!({ format: { "hlsManifests": [{}], "encryption": encryption } })
        }
        "dashPackage" => json!({ format: { "dashManifests": [{}], "encryption": encryption } }),
        "mssPackage" => json!({ format: { "mssManifests": [{}], "encryption": encryption } }),
        other => panic!("{other:?} is not a modelled package block"),
    }
}

/// The three listings of this service, as one comparable value.
async fn listings(router: &MultiServiceRouter) -> Value {
    let mut snapshot = json!({});
    for (name, path) in [
        ("packagingGroups", "/packaging_groups"),
        ("assets", "/assets"),
        ("packagingConfigurations", "/packaging_configurations"),
    ] {
        let (status, _, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "the {name} listing must answer 200, got {status} body {body}"
        );
        snapshot[name] = body;
    }
    snapshot
}

/// A store holding one group, one asset and one packaging configuration, all created
/// with well-formed ARNs — so a listing that changes after a refusal is visible.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    let (status, _, body) = call(
        &router,
        "POST",
        "/packaging_groups",
        Some(json!({
            "id": GROUP,
            "authorization": {
                "cdnIdentifierSecret": GOOD_SECRET,
                "secretsRoleArn": GOOD_IAM_ROLE,
            },
            "tags": { "team": "video" },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreatePackagingGroup must succeed, got {status} body {body}"
    );

    let (status, _, body) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "id": "stored-asset",
            "packagingGroupId": GROUP,
            "sourceArn": GOOD_S3_OBJECT,
            "sourceRoleArn": GOOD_IAM_ROLE,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateAsset must succeed, got {status} body {body}"
    );

    let mut config = json!({ "id": "stored-cfg", "packagingGroupId": GROUP });
    let Value::Object(members) = encrypted_package("hlsPackage", GOOD_IAM_ROLE) else {
        panic!("a package block must be a JSON object");
    };
    for (key, block) in members {
        config[key] = block;
    }
    let (status, _, body) = call(&router, "POST", "/packaging_configurations", Some(config)).await;
    assert_eq!(
        status, 202,
        "setup: CreatePackagingConfiguration must succeed, got {status} body {body}"
    );

    router
}

/// `(case, method, path, body)` — one refused request per ARN-documented path of this
/// service. Each body is well formed except for the one member it makes junk, and the
/// junk values differ in form (not an ARN, an empty value, an ARN of another service,
/// an ARN of the right service and the wrong resource kind).
fn refused_requests() -> Vec<(String, &'static str, String, Value)> {
    let mut rows: Vec<(String, &'static str, String, Value)> = vec![
        (
            "CreatePackagingGroup with a junk cdnIdentifierSecret".to_string(),
            "POST",
            "/packaging_groups".to_string(),
            json!({
                "id": "new-group",
                "authorization": {
                    "cdnIdentifierSecret": "not-an-arn",
                    "secretsRoleArn": GOOD_IAM_ROLE,
                },
            }),
        ),
        (
            "UpdatePackagingGroup with an empty secretsRoleArn".to_string(),
            "PUT",
            format!("/packaging_groups/{GROUP}"),
            json!({
                "authorization": { "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": "" },
            }),
        ),
        (
            "CreateAsset with an IAM role as its sourceArn".to_string(),
            "POST",
            "/assets".to_string(),
            json!({
                "id": "new-asset",
                "packagingGroupId": GROUP,
                "sourceArn": GOOD_IAM_ROLE,
                "sourceRoleArn": GOOD_IAM_ROLE,
            }),
        ),
        (
            "CreateAsset with an IAM policy as its sourceRoleArn".to_string(),
            "POST",
            "/assets".to_string(),
            json!({
                "id": "new-asset",
                "packagingGroupId": GROUP,
                "sourceArn": GOOD_S3_OBJECT,
                "sourceRoleArn": "arn:aws:iam::111122223333:policy/VodIngest",
            }),
        ),
    ];
    // One row per packaging format: the same member under four different wire paths.
    for format in ["hlsPackage", "dashPackage", "cmafPackage", "mssPackage"] {
        let mut body = json!({ "id": "new-cfg", "packagingGroupId": GROUP });
        let Value::Object(members) = encrypted_package(format, "speke-role") else {
            panic!("a package block must be a JSON object");
        };
        for (key, block) in members {
            body[key] = block;
        }
        rows.push((
            format!("CreatePackagingConfiguration with a junk {format} SPEKE role"),
            "POST",
            "/packaging_configurations".to_string(),
            body,
        ));
    }
    rows
}

/// Every refusal leaves all three listings exactly as they were.
#[tokio::test]
async fn a_refused_arn_member_changes_no_listing() {
    let (code, status) = UNPROCESSABLE;
    for (case, method, path, body) in refused_requests() {
        // A fresh store per row, so a row cannot observe another row's effect.
        let router = fixture().await;
        let before = listings(&router).await;

        let (answered, errortype, answer) = call(&router, method, &path, Some(body)).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} body {answer}"
        );
        assert_eq!(
            errortype, code,
            "case {case:?}: expected x-amzn-errortype {code}, got {errortype:?}"
        );

        let after = listings(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refusal must leave every listing unchanged, got {after} \
             instead of {before}"
        );
    }

    // Not a blanket answer: a well-formed request of each kind DOES change the
    // listings, so an unchanged listing above is a fact about the refusal.
    let router = fixture().await;
    let before = listings(&router).await;
    let (status, _, body) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "id": "another-asset",
            "packagingGroupId": GROUP,
            "sourceArn": GOOD_S3_OBJECT,
            "sourceRoleArn": GOOD_IAM_ROLE,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "a well-formed CreateAsset must be accepted, got {status} body {body}"
    );
    let after = listings(&router).await;
    assert_ne!(
        after, before,
        "an accepted create must change the listings, got {after}"
    );
}
