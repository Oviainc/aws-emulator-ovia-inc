// SPDX-License-Identifier: CC0-1.0

//! Whether revoking a signing PROFILE is served, end to end over HTTP.
//!
//! The rule under test: a revocation call is served exactly when the platform the
//! resource was signed on publishes `revocationSupported` — the capability
//! `GetSigningPlatform` reports — and a platform published without it refuses the
//! call with the modelled 400 `ValidationException`, recording nothing, whichever
//! revocation operation the caller used. `RevokeSigningProfile` revokes the
//! profile so that "signatures generated using the signing profile after an
//! effective start date are no longer valid"
//! (`smithy_query(query="doc", shape="RevokeSigningProfile")`), so what it records
//! is a revocation of that platform's signatures and is held to the same
//! capability `RevokeSignature` is.
//!
//! The cases are tables over what the rule generalises over: every platform the
//! published catalog holds (so both sides of the capability are exercised,
//! including entries no issue cited), the revocation reason at both published
//! bounds of `RevocationReasonString`'s `@length(min: 1, max: 500)`, where the
//! effective instant sits relative to the epoch, which version of the profile the
//! request names, and the region the request is signed for.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    SigningWorkflow, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a case is signed for unless its row names another one.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents, spelled out so a wrong
/// account in the handlers fails here.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The error shape both revocation paths refuse an unsupported platform with.
const VALIDATION_SHAPE: &str = "ValidationException";

/// The one `certificateHashes` entry the revocation-status requests below carry.
///
/// `certificateHashes` is required, and this service revokes profiles and
/// signatures rather than certificates, so no hash is ever reported revoked: the
/// value only has to be a hash the request can carry.
const CERTIFICATE_HASH: &str = "aa";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status, the raw body and its JSON.
struct Answer {
    status: u16,
    raw_body: Vec<u8>,
    body: Value,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or the empty string when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The `revokedEntities` a 200 `GetRevocationStatus` answer reports.
    fn revoked_entities(&self) -> Vec<String> {
        self.body["revokedEntities"]
            .as_array()
            .map(|entities| {
                entities
                    .iter()
                    .map(|entity| entity.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header("authorization", authorization);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        raw_body: bytes,
        body,
    }
}

async fn put_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    platform_id: &str,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform_id,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn get_platform(router: &MultiServiceRouter, region: &str, platform_id: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-platforms/{platform_id}"),
        None,
    )
    .await
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: &str,
    reason: &str,
    effective_epoch_secs: i64,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": reason,
            "effectiveTime": effective_epoch_secs,
        })),
    )
    .await
}

async fn start_job(router: &MultiServiceRouter, region: &str, profile_name: &str) -> Answer {
    call(
        router,
        region,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
            "destination": { "s3": { "bucketName": "dst" } },
            "profileName": profile_name,
            "clientRequestToken": format!("tok-{region}-{profile_name}"),
        })),
    )
    .await
}

async fn revoke_signature(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-jobs/{job_id}/revoke"),
        Some(json!({ "reason": "the signing key was compromised" })),
    )
    .await
}

/// `GET /revocations` asking about ONE profile version at `signature_epoch_secs`.
///
/// The other entities the request must name are a signing job this endpoint holds
/// none of and one certificate hash — neither is revoked, so the answer is about
/// the profile version alone.
async fn revocation_status_of(
    router: &MultiServiceRouter,
    region: &str,
    platform_id: &str,
    profile_version_arn: &str,
    signature_epoch_secs: i64,
) -> Answer {
    let signature_timestamp = DateTime::from_secs(signature_epoch_secs)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time");
    let job_arn = format!("arn:aws:signer:{region}:{ACCOUNT_ID}:/signing-jobs/no-such-job");
    let uri = format!(
        "/revocations?signatureTimestamp={signature_timestamp}&platformId={platform_id}\
         &profileVersionArn={profile_version_arn}&jobArn={job_arn}\
         &certificateHashes={CERTIFICATE_HASH}"
    );
    call(router, region, "GET", &uri, None).await
}

/// The refusal prose both revocation paths answer for a platform published
/// without the capability: the condition, then the platform that does not have
/// it.
fn unsupported_message(platform_id: &str) -> String {
    format!("Revocation is not supported for platform {platform_id}")
}

/// One revocation request shape to send on every published platform. The rows
/// differ along the reason's length (both published bounds of
/// `RevocationReasonString`'s `@length(min: 1, max: 500)`), where the effective
/// instant sits, and the region the request is signed for.
struct RequestShape {
    name: &'static str,
    region: &'static str,
    reason: String,
    effective_epoch_secs: i64,
}

fn request_shapes() -> Vec<RequestShape> {
    vec![
        // A reason of ordinary length, a past instant.
        RequestShape {
            name: "ordinary_reason",
            region: REGION,
            reason: "the signing material is no longer trusted".to_string(),
            effective_epoch_secs: 1_700_000_000,
        },
        // The shortest reason the shape admits, in another region.
        RequestShape {
            name: "shortest_reason",
            region: "eu-west-1",
            reason: "x".to_string(),
            effective_epoch_secs: 1_000_000_000,
        },
        // The longest reason the shape admits, and an instant far in the future,
        // in a third region.
        RequestShape {
            name: "longest_reason_future_instant",
            region: "ap-southeast-1",
            reason: "y".repeat(500),
            effective_epoch_secs: 4_000_000_000,
        },
    ]
}

/// A signing profile is revocable exactly when its platform publishes revocation
/// support: a published platform without it refuses `RevokeSigningProfile` with
/// the modelled 400 and records nothing, so `GetSigningProfile` still reports the
/// profile `Active` and `GetRevocationStatus` reports none of its versions
/// revoked.
///
/// One row per published catalog entry, with the expectation read off the
/// service's own `GetSigningPlatform` answer rather than written out here, crossed
/// with every request shape above.
#[tokio::test]
async fn profile_revocation_follows_the_platforms_published_revocation_support() {
    for entry in SIGNING_PLATFORM_CATALOG {
        for shape in request_shapes() {
            let platform_id = entry.platform_id;
            let case = format!("{platform_id} / {}", shape.name);
            let router = emulator();
            let region = shape.region;
            let name = format!("Cap_{}", platform_id.replace('-', "_"));

            let created = put_profile(&router, region, &name, platform_id).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: seeding the profile: {} {:?}",
                created.status, created.body
            );
            let profile_arn = created.str_member("arn");
            let version = created.str_member("profileVersion");

            // The capability as the service itself publishes it.
            let platform = get_platform(&router, region, platform_id).await;
            assert_eq!(
                platform.status, 200,
                "case {case:?}: expected GetSigningPlatform 200, got {} {:?}",
                platform.status, platform.body
            );
            let supported = platform.body["revocationSupported"]
                .as_bool()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a boolean revocationSupported, got {:?}",
                        platform.body["revocationSupported"]
                    )
                });

            let answer = revoke_profile(
                &router,
                region,
                &name,
                &version,
                &shape.reason,
                shape.effective_epoch_secs,
            )
            .await;
            let after = get_profile(&router, region, &name).await;
            // Asked about at the effective instant itself, the earliest instant a
            // recorded revocation covers.
            let status = revocation_status_of(
                &router,
                region,
                platform_id,
                &format!("{profile_arn}/{version}"),
                shape.effective_epoch_secs,
            )
            .await;
            assert_eq!(
                status.status, 200,
                "case {case:?}: expected GetRevocationStatus 200, got {} {:?}",
                status.status, status.body
            );

            if supported {
                assert_eq!(
                    answer.status, 204,
                    "case {case:?}: a platform publishing revocation support must revoke, got {} \
                     {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    after.str_member("status"),
                    "Revoked",
                    "case {case:?}: expected the revoked profile to report Revoked, got {:?}",
                    after.body["status"]
                );
                assert!(
                    after.body["revocationRecord"].is_object(),
                    "case {case:?}: expected a revocation record, got {:?}",
                    after.body["revocationRecord"]
                );
                assert_eq!(
                    status.revoked_entities(),
                    vec![format!("{profile_arn}/{version}")],
                    "case {case:?}: expected the revoked version to be reported, got {:?}",
                    status.body["revokedEntities"]
                );
            } else {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: a platform publishing no revocation support must refuse with \
                     400, got {} {:?}",
                    answer.status, answer.body
                );
                assert!(
                    answer.shape().contains(VALIDATION_SHAPE),
                    "case {case:?}: expected a {VALIDATION_SHAPE}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.code(),
                    VALIDATION_SHAPE,
                    "case {case:?}: expected code {VALIDATION_SHAPE}, got {:?}",
                    answer.body["code"]
                );
                assert_eq!(
                    answer.message(),
                    unsupported_message(platform_id),
                    "case {case:?}: expected the refusal to state the capability and name the \
                     platform, got {:?}",
                    answer.body["message"]
                );
                // The refused call recorded nothing: the profile is untouched and
                // no version of it is reported revoked.
                assert_eq!(
                    after.str_member("status"),
                    "Active",
                    "case {case:?}: expected the refused revoke to leave the profile Active, got \
                     {:?}",
                    after.body["status"]
                );
                assert!(
                    after.body["revocationRecord"].is_null(),
                    "case {case:?}: expected the refused revoke to write no record, got {:?}",
                    after.body["revocationRecord"]
                );
                assert!(
                    after.body["statusReason"].is_null(),
                    "case {case:?}: expected the refused revoke to write no statusReason, got {:?}",
                    after.body["statusReason"]
                );
                assert!(
                    status.revoked_entities().is_empty(),
                    "case {case:?}: expected nothing revoked, got {:?}",
                    status.body["revokedEntities"]
                );
            }
        }
    }
}

/// The two revocation paths answer ONE verdict for a platform's capability: for
/// every platform whose profiles the signing-job workflow serves, revoking the
/// signature and revoking the profile either both succeed or both refuse with the
/// same status, `code` and message.
///
/// A platform published for the container-image workflow has no row: no signing
/// job is started from its profiles, so there is no signature to revoke — see
/// `start_signing_job_platform_workflow.rs`.
#[tokio::test]
async fn both_revocation_paths_agree_on_the_platforms_capability() {
    for entry in SIGNING_PLATFORM_CATALOG
        .iter()
        .filter(|entry| entry.workflow == SigningWorkflow::CodeSigningJob)
    {
        let platform_id = entry.platform_id;
        let router = emulator();
        let name = format!("Agree_{}", platform_id.replace('-', "_"));

        let created = put_profile(&router, REGION, &name, platform_id).await;
        assert_eq!(
            created.status, 200,
            "case {platform_id:?}: seeding the profile: {} {:?}",
            created.status, created.body
        );
        let version = created.str_member("profileVersion");
        let started = start_job(&router, REGION, &name).await;
        assert_eq!(
            started.status, 200,
            "case {platform_id:?}: seeding a job: {} {:?}",
            started.status, started.body
        );
        let job_id = started.str_member("jobId");

        let signature = revoke_signature(&router, REGION, &job_id).await;
        let profile = revoke_profile(
            &router,
            REGION,
            &name,
            &version,
            "both paths are asked the same question",
            1_700_000_000,
        )
        .await;

        let signature_refused = signature.status >= 400;
        let profile_refused = profile.status >= 400;
        assert_eq!(
            signature_refused, profile_refused,
            "case {platform_id:?}: expected the two revocation paths to agree, got {} for the \
             signature ({:?}) and {} for the profile ({:?})",
            signature.status, signature.body, profile.status, profile.body
        );

        if signature_refused {
            assert_eq!(
                signature.status, profile.status,
                "case {platform_id:?}: expected the same refusal status, got {} and {}",
                signature.status, profile.status
            );
            assert_eq!(
                signature.code(),
                profile.code(),
                "case {platform_id:?}: expected the same refusal code, got {:?} and {:?}",
                signature.body["code"],
                profile.body["code"]
            );
            assert_eq!(
                signature.message(),
                profile.message(),
                "case {platform_id:?}: expected the same refusal message, got {:?} and {:?}",
                signature.body["message"],
                profile.body["message"]
            );
            assert_eq!(
                profile.message(),
                unsupported_message(platform_id),
                "case {platform_id:?}: expected the capability refusal, got {:?}",
                profile.body["message"]
            );
        } else {
            assert!(
                profile.raw_body.is_empty(),
                "case {platform_id:?}: expected an empty body on a successful revoke, got {:?}",
                String::from_utf8_lossy(&profile.raw_body)
            );
        }
    }
}

/// What the profile's identity still decides before the capability is consulted:
/// a `profileVersion` the profile has never had names no revocable resource, so
/// it is the modelled 404 even on a platform published without the capability —
/// the same order `RevokeSignature` resolves a job's identity in before judging
/// the platform.
#[tokio::test]
async fn a_version_the_profile_never_had_is_refused_before_the_capability() {
    let router = emulator();
    // A platform the published catalog reports without the capability.
    let platform_id = SIGNING_PLATFORM_CATALOG
        .iter()
        .find(|entry| !entry.revocation_supported)
        .expect("the catalog publishes a platform without revocation support")
        .platform_id;
    let name = "Order_Of_Refusals";
    let created = put_profile(&router, REGION, name, platform_id).await;
    assert_eq!(
        created.status, 200,
        "seeding the profile: {} {:?}",
        created.status, created.body
    );

    let stored_version = created.str_member("profileVersion");
    // (case, the profileVersion the request names, the expected status)
    let rows: [(&str, &str, u16); 2] = [
        (
            "a version of the right form the profile never had",
            "0000000000",
            404,
        ),
        ("the version the profile does have", &stored_version, 400),
    ];

    for (case, version, expected) in rows {
        let answer = revoke_profile(
            &router,
            REGION,
            name,
            version,
            "ordering case",
            1_700_000_000,
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            answer.status, answer.body
        );
    }

    let after = get_profile(&router, REGION, name).await;
    assert_eq!(
        after.str_member("status"),
        "Active",
        "expected the refused revokes to have written nothing, got {:?}",
        after.body["status"]
    );
}
