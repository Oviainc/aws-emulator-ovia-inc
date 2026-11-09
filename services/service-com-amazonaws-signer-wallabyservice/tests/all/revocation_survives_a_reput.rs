// SPDX-License-Identifier: CC0-1.0

//! A signing profile's revocation survives every later attempt to write to that
//! profile, end to end over HTTP.
//!
//! The rule under test: a revocation is a fact about trust that has ALREADY
//! ended, and a signing-profile name is taken for good by the create that first
//! used it — so no later `PutSigningProfile` can replace a revoked profile with a
//! trusted one. Each such create is refused `400 ValidationException`, and the
//! revoked profile reads back unchanged: the same `Revoked` status, the same
//! `statusReason`, the same one revocation record
//! (`revocationEffectiveFrom`, `revokedAt`, `revokedBy`), the same identity (ARN,
//! `profileToken`) and the same version it was revoked at.
//! `revocationEffectiveFrom` is the instant after which signatures stop being
//! trusted (`smithy_query(query="doc", shape="RevokeSigningProfileRequest",
//! member="effectiveTime")`), so `GetRevocationStatus` still reports the
//! profile's version as revoked for a signature made at or after it, and still
//! reports nothing for one made before it.
//!
//! A profile nobody revoked is just as untouched by the refused create: it stays
//! `Active`, with no `statusReason`, no revocation record and nothing revoked.
//!
//! The rows differ along what the rule generalises over: how many creates arrive
//! after the revocation, which platform each of them names (the same one, or
//! another published one), where the signature's timestamp falls relative to the
//! effective instant (one second before, exactly on it, one second after), the
//! reason recorded (including `RevocationReasonString`'s `@length` minimum of 1
//! and maximum of 500), the effective instant, and the region the requests are
//! signed for.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents, spelled out rather than
/// imported so a wrong account in the handlers fails here.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The status a second create under a name this endpoint already holds answers,
/// and the error shape and `code` it carries: a refusal of the request's own
/// values.
const TAKEN_NAME_STATUS: u16 = 400;
/// The error shape that refusal is answered in.
const TAKEN_NAME_SHAPE: &str = "ValidationException";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope naming this service for `region`: it is what routes a
/// request to signer, and its region is the one whose resources are addressed.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `revokedEntities` list, as the strings it carries.
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
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
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
    Answer { status, body }
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

/// `GET /revocations` asking about ONE profile version, at `signature_epoch_secs`.
///
/// The other entities the request has to name are a signing job this endpoint
/// holds none of and one certificate hash — neither is revoked, so the answer is
/// about the profile version alone.
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
    // An SDK sends `:` and `/` unescaped on the query string, which is how an ARN
    // and an ISO 8601 instant arrive.
    let job_arn = format!(
        "arn:aws:signer:{region}:{ACCOUNT_ID}:/signing-jobs/00000000-0000-0000-0000-000000000000"
    );
    let hash = {
        use sha2::Digest;
        format!(
            "{}{}",
            hex::encode(sha2::Sha384::digest(b"leaf")),
            hex::encode(sha2::Sha384::digest(b"parent"))
        )
    };
    let uri = format!(
        "/revocations?signatureTimestamp={signature_timestamp}&platformId={platform_id}\
         &profileVersionArn={profile_version_arn}&jobArn={job_arn}&certificateHashes={hash}"
    );
    call(router, region, "GET", &uri, None).await
}

/// One revoked profile, and the creates that are attempted after its revocation.
struct Case {
    name: &'static str,
    region: &'static str,
    /// The platform the profile is created on.
    platform: &'static str,
    /// The platform each create that arrives AFTER the revocation names — one
    /// entry per attempt, so a row also varies how many of them there are. Every
    /// id is a published one (`SIGNING_PLATFORM_CATALOG`), so nothing but the
    /// taken name can refuse the call.
    attempts: &'static [&'static str],
    /// The reason the ONE revocation records.
    reason: String,
    /// The instant the revocation takes effect, as epoch seconds.
    effective_epoch_secs: i64,
}

fn cases() -> Vec<Case> {
    vec![
        // One attempt, naming the SAME platform: the smallest repeated create
        // there is.
        Case {
            name: "Revoked_Then_Recreated_Same_Platform",
            region: "us-west-2",
            platform: "AWSLambda-SHA384-ECDSA",
            attempts: &["AWSLambda-SHA384-ECDSA"],
            reason: "key compromised".to_string(),
            effective_epoch_secs: 1_700_000_000,
        },
        // One attempt naming ANOTHER published platform, and the shortest reason
        // `RevocationReasonString` admits (`@length(min: 1)`).
        Case {
            name: "Revoked_Then_Recreated_Other_Platform",
            region: "eu-west-1",
            platform: "Notation-OCI-SHA384-ECDSA",
            attempts: &["AWSIoTDeviceManagement-SHA256-ECDSA"],
            reason: "x".to_string(),
            effective_epoch_secs: 1_000_000_000,
        },
        // Three attempts, across platforms, and the longest reason that shape
        // admits (`@length(max: 500)`): the name stays taken however often the
        // create is retried.
        Case {
            name: "Revoked_Then_Recreated_Three_Times",
            region: "us-east-1",
            // The revoke is seeded on this platform, so it is one that publishes
            // the revocation capability.
            platform: "AWSLambda-SHA384-ECDSA",
            attempts: &[
                "AmazonFreeRTOS-Default",
                "AmazonFreeRTOS-TI-CC3220SF",
                "AWSIoTDeviceManagement-SHA256-ECDSA",
            ],
            reason: "y".repeat(500),
            effective_epoch_secs: 1_600_000_000,
        },
        // A far-future effective instant, in a fourth region.
        Case {
            name: "Revoked_Then_Recreated_Future_Effective",
            region: "ap-southeast-1",
            platform: "AWSLambda-SHA384-ECDSA",
            attempts: &["Notation-OCI-SHA384-ECDSA"],
            reason: "certificate material leaked".to_string(),
            effective_epoch_secs: 4_000_000_000,
        },
    ]
}

/// A `PutSigningProfile` under a revoked profile's name is refused and restores
/// no trust: the profile stays `Revoked` with the one record the revoke wrote and
/// the one version it was revoked at, which is still reported as revoked for a
/// signature made at or after the effective instant and still not reported for
/// one made before it.
#[tokio::test]
async fn a_recreate_of_a_revoked_profile_keeps_its_revocation() {
    for case in cases() {
        let router = emulator();

        let created = put_profile(&router, case.region, case.name, case.platform).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding the profile: {} {:?}",
            case.name, created.status, created.body
        );
        let profile_arn = created.str_member("arn");
        let version = created.str_member("profileVersion");

        let revoked = revoke_profile(
            &router,
            case.region,
            case.name,
            &version,
            &case.reason,
            case.effective_epoch_secs,
        )
        .await;
        assert_eq!(
            revoked.status, 204,
            "case {:?}: expected RevokeSigningProfile to answer 204, got {} {:?}",
            case.name, revoked.status, revoked.body
        );

        // The ONE revocation, as the profile reports it before any later create.
        let after_revoke = get_profile(&router, case.region, case.name).await;
        assert_eq!(
            after_revoke.str_member("status"),
            "Revoked",
            "case {:?}: expected the revoke to leave the profile Revoked, got {:?}",
            case.name,
            after_revoke.body["status"]
        );
        let record = after_revoke.body["revocationRecord"].clone();
        assert!(
            record.is_object(),
            "case {:?}: expected a revocation record after the revoke, got {record:?}",
            case.name
        );
        let profile_token = after_revoke.str_member("profileToken");

        for (ordinal, platform) in case.attempts.iter().enumerate() {
            let attempt = put_profile(&router, case.region, case.name, platform).await;
            assert_eq!(
                attempt.status, TAKEN_NAME_STATUS,
                "case {:?} attempt {ordinal}: expected HTTP {TAKEN_NAME_STATUS} for the taken \
                 name, got {} {:?}",
                case.name, attempt.status, attempt.body
            );
            assert_eq!(
                attempt.shape(),
                TAKEN_NAME_SHAPE,
                "case {:?} attempt {ordinal}: expected the error shape {TAKEN_NAME_SHAPE}, got \
                 {:?}",
                case.name,
                attempt.body
            );

            let got = get_profile(&router, case.region, case.name).await;
            // The refused create wrote nothing: the version, the platform, the
            // status, the reason, the one record and the identity all stand.
            assert_eq!(
                got.str_member("profileVersion"),
                version,
                "case {:?} attempt {ordinal}: expected the stored version to be unchanged, got \
                 {:?}",
                case.name,
                got.body["profileVersion"]
            );
            assert_eq!(
                got.str_member("platformId"),
                case.platform,
                "case {:?} attempt {ordinal}: expected the created platform {}, got {:?}",
                case.name,
                case.platform,
                got.body["platformId"]
            );
            assert_eq!(
                got.str_member("status"),
                "Revoked",
                "case {:?} attempt {ordinal}: expected the profile to stay Revoked, got {:?}",
                case.name,
                got.body["status"]
            );
            assert_eq!(
                got.str_member("statusReason"),
                case.reason,
                "case {:?} attempt {ordinal}: expected the revoke's reason to stand, got {:?}",
                case.name,
                got.body["statusReason"]
            );
            assert_eq!(
                got.body["revocationRecord"], record,
                "case {:?} attempt {ordinal}: expected the one revocation record to stand, got \
                 {:?} instead of {record:?}",
                case.name, got.body["revocationRecord"]
            );
            assert_eq!(
                got.str_member("arn"),
                profile_arn,
                "case {:?} attempt {ordinal}: expected the profile ARN to be unchanged, got {:?}",
                case.name,
                got.body["arn"]
            );
            assert_eq!(
                got.str_member("profileToken"),
                profile_token,
                "case {:?} attempt {ordinal}: expected the profile's id to be unchanged, got {:?}",
                case.name,
                got.body["profileToken"]
            );

            // The profile's version is still revoked from the effective instant
            // on — and still trusted before it.
            let version_arn = format!("{profile_arn}/{version}");
            for (label, signature_epoch, revoked_then) in [
                ("one second before the effective instant", -1, false),
                ("exactly on the effective instant", 0, true),
                ("one second after the effective instant", 1, true),
            ] {
                let answer = revocation_status_of(
                    &router,
                    case.region,
                    case.platform,
                    &version_arn,
                    case.effective_epoch_secs + signature_epoch,
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {:?} attempt {ordinal} {label}: expected GetRevocationStatus 200, got \
                     {} {:?}",
                    case.name, answer.status, answer.body
                );
                let expected: Vec<String> = if revoked_then {
                    vec![version_arn.clone()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    answer.revoked_entities(),
                    expected,
                    "case {:?} attempt {ordinal} {label}: expected revokedEntities {expected:?}, \
                     got {:?}",
                    case.name,
                    answer.body["revokedEntities"]
                );
            }
        }
    }
}

/// The other side of the rule: a profile nobody revoked has no revocation to
/// carry, and the refused create leaves it exactly as it was — `Active`, no
/// `statusReason`, no revocation record, the version it was created with, and
/// nothing revoked for it.
#[tokio::test]
async fn a_recreate_of_a_never_revoked_profile_keeps_it_trusted() {
    // (case, region, the platform of the create, the platform each later attempt
    // names) — rows differ in how many attempts arrive and whether they name
    // another platform.
    let rows: [(&str, &str, &str, &[&str]); 3] = [
        (
            "one attempt, same platform",
            "us-west-2",
            "AWSLambda-SHA384-ECDSA",
            &["AWSLambda-SHA384-ECDSA"],
        ),
        (
            "one attempt, another platform",
            "eu-west-1",
            "Notation-OCI-SHA384-ECDSA",
            &["AmazonFreeRTOS-Default"],
        ),
        (
            "two attempts",
            "us-east-1",
            "AWSIoTDeviceManagement-SHA256-ECDSA",
            &["AmazonFreeRTOS-TI-CC3220SF", "AWSLambda-SHA384-ECDSA"],
        ),
    ];

    for (case, region, platform, attempts) in rows {
        let router = emulator();
        let name = "Never_Revoked_Then_Recreated";
        let created = put_profile(&router, region, name, platform).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: seeding the profile: {} {:?}",
            created.status, created.body
        );
        let profile_arn = created.str_member("arn");
        let version = created.str_member("profileVersion");

        for (ordinal, written) in attempts.iter().enumerate() {
            let attempt = put_profile(&router, region, name, written).await;
            assert_eq!(
                attempt.status, TAKEN_NAME_STATUS,
                "case {case:?} attempt {ordinal}: expected HTTP {TAKEN_NAME_STATUS} for the taken \
                 name, got {} {:?}",
                attempt.status, attempt.body
            );

            let got = get_profile(&router, region, name).await;
            assert_eq!(
                got.str_member("status"),
                "Active",
                "case {case:?} attempt {ordinal}: expected an unrevoked profile to stay Active, \
                 got {:?}",
                got.body["status"]
            );
            assert_eq!(
                got.str_member("profileVersion"),
                version,
                "case {case:?} attempt {ordinal}: expected the stored version to be unchanged, \
                 got {:?}",
                got.body["profileVersion"]
            );
            assert!(
                got.body["statusReason"].is_null(),
                "case {case:?} attempt {ordinal}: expected no statusReason on an Active profile, \
                 got {:?}",
                got.body["statusReason"]
            );
            assert!(
                got.body["revocationRecord"].is_null(),
                "case {case:?} attempt {ordinal}: expected no revocation record on a profile \
                 nobody revoked, got {:?}",
                got.body["revocationRecord"]
            );

            let version_arn = format!("{profile_arn}/{version}");
            let answer =
                revocation_status_of(&router, region, platform, &version_arn, 1_700_000_000).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} attempt {ordinal}: expected GetRevocationStatus 200, got {} {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.revoked_entities().is_empty(),
                "case {case:?} attempt {ordinal}: expected nothing revoked, got {:?}",
                answer.body["revokedEntities"]
            );
        }
    }
}
