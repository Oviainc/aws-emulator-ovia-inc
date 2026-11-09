// SPDX-License-Identifier: CC0-1.0

//! What AWS Signer answers when it refuses a request, end to end over HTTP.
//!
//! Three rules are under test, each over a table rather than over one request:
//!
//! * every error response carries BOTH modelled members — `message` (prose) and
//!   `code` (`ErrorCode`) — and the `code` names the condition that was refused,
//!   which is the only machine-readable discriminator two refusals sharing a
//!   status have;
//! * a signing profile cannot be created without the certificate its signatures
//!   are issued under, so a `PutSigningProfile` on a platform whose profile holds
//!   that certificate and that supplies no `signingMaterial` is refused 400 and
//!   stores nothing — while a platform AWS Signer issues the certificate for
//!   takes none;
//! * a refusal uses one of the shapes the refusing OPERATION's own model
//!   declares, so the condition of a stored resource is never answered with a
//!   shape borrowed from another operation's error set — and where no modelled
//!   shape covers a stored status, the call is not refused at all: the
//!   transition applies where its documented source status holds and a terminal
//!   status is left as it stands.
//!
//! The rows differ along what the rules generalise over: which condition was
//! refused, which operation refused it, the platform, and the profile's status.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::{
    ERROR_CODE_INVALID_NEXT_TOKEN, ERROR_CODE_JOB_NOT_FOUND, ERROR_CODE_NOT_FOUND,
    ERROR_CODE_PROFILE_NOT_FOUND, ERROR_CODE_VALIDATION,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The IAM principal the emulator resolves every signed request as: the one IAM
/// user whose credentials it publishes, which is the identity
/// `sts:GetCallerIdentity` answers with. Spelled out here rather than imported so
/// a revoker invented from the request fails this suite.
fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator")
}

/// The account id every generated emulator presents, spelled out here rather than
/// imported so a wrong account in the handlers fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The signing platforms this suite puts profiles on, and whether a profile on
/// each one must carry `signingMaterial`.
///
/// These are the signing platforms this emulator publishes (the ids of
/// `crate::state::SIGNING_PLATFORM_CATALOG`): the three device-signing platforms,
/// whose profiles hold the certificate the caller brought in ACM, plus the two
/// AWS Signer issues the certificate for itself — Lambda code signing and the
/// container-image platform. Source: the published `requires_signing_material` of
/// each catalog entry. A `platformId` the catalog does not publish is refused
/// outright, which `put_profile_platform_validation.rs` covers.
const PLATFORM_CASES: &[(&str, bool)] = &[
    ("AWSLambda-SHA384-ECDSA", false),
    ("AWSIoTDeviceManagement-SHA256-ECDSA", true),
    ("AmazonFreeRTOS-Default", true),
    ("AmazonFreeRTOS-TI-CC3220SF", true),
    ("Notation-OCI-SHA384-ECDSA", false),
];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status, JSON body, and the `x-amzn-errortype`
/// header restJson1 resolves an error SHAPE from.
struct Answer {
    status: u16,
    body: Value,
    error_type: Option<String>,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or `None` when the body carries none.
    fn message(&self) -> Option<&str> {
        self.body["message"].as_str()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
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
        body,
        error_type,
    }
}

async fn put_profile(router: &MultiServiceRouter, name: &str, request: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(request),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
}

async fn cancel_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    name: &str,
    profile_version: &str,
    reason: &str,
    effective_time: i64,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": profile_version,
            "reason": reason,
            "effectiveTime": effective_time,
        })),
    )
    .await
}

/// Create an ACTIVE profile on a certificate-signing platform, answering its
/// current version.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let put = put_profile(
        router,
        name,
        json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        }),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "seeding {name}: expected 200, got {} {:?}",
        put.status, put.body
    );
    put.str_member("profileVersion")
}

/// A `StartSigningJob` body naming `profile`.
fn start_job_request(profile: &str, token: &str) -> Value {
    json!({
        "source": { "s3": { "bucketName": "src-bucket", "key": "code.bin", "version": "1" } },
        "destination": { "s3": { "bucketName": "dst-bucket", "prefix": "signed/" } },
        "profileName": profile,
        "clientRequestToken": token,
    })
}

/// One refusal: the request, and what the service must answer.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    /// The modelled error SHAPE, which is also the `x-amzn-errortype` value.
    shape: &'static str,
    /// The `code` member naming the condition that was refused.
    code: String,
}

/// Every refusal this slice can answer carries the modelled `code` naming its
/// condition, alongside a message and the `x-amzn-errortype` header — so two
/// refusals that share a status are still told apart.
///
/// One row per condition the slice refuses, across five operations and both
/// statuses (404 and 400).
#[tokio::test]
async fn every_refusal_names_its_condition_in_the_modelled_code_member() {
    let router = emulator();
    seed_profile(&router, "Active_Profile").await;
    seed_profile(&router, "Canceled_Profile").await;
    let revoked_version = seed_profile(&router, "Revoked_Profile").await;
    assert_eq!(
        cancel_profile(&router, "Canceled_Profile").await.status,
        204,
        "seeding the canceled profile"
    );
    let revoked = revoke_profile(
        &router,
        "Revoked_Profile",
        &revoked_version,
        "rotate-signing-key",
        1_700_000_000,
    )
    .await;
    assert_eq!(
        revoked.status, 204,
        "seeding the revoked profile: got {} {:?}",
        revoked.status, revoked.body
    );

    // Tokens that are not a `nextToken` this service handed out: too short, with
    // a character the profile-name alphabet excludes, and longer than the 64 a
    // profile name may be.
    let bad_tokens = ["a", "not a profile name", "Bad-Token", &"x".repeat(65)];

    let mut cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "GetSigningProfile on a profile that was never created",
            method: "GET",
            uri: "/signing-profiles/No_Such_Profile".to_string(),
            body: None,
            status: 404,
            shape: "ResourceNotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "GetSigningProfile for another account's profile",
            method: "GET",
            uri: "/signing-profiles/Active_Profile?profileOwner=123456789012".to_string(),
            body: None,
            status: 404,
            shape: "ResourceNotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "CancelSigningProfile on a profile that was never created",
            method: "DELETE",
            uri: "/signing-profiles/No_Such_Profile".to_string(),
            body: None,
            status: 404,
            shape: "ResourceNotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "RevokeSigningProfile on a profile that was never created",
            method: "PUT",
            uri: "/signing-profiles/No_Such_Profile/revoke".to_string(),
            body: Some(json!({
                "profileVersion": "0000000001",
                "reason": "no such profile",
                "effectiveTime": 1_700_000_000,
            })),
            status: 404,
            shape: "ResourceNotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "DescribeSigningJob on a job id that was never minted",
            method: "GET",
            uri: "/signing-jobs/00000000-0000-0000-0000-000000000000".to_string(),
            body: None,
            status: 404,
            // A job-specific code: the same 404 shape as a missing profile, which
            // is exactly why the code has to differ.
            shape: "ResourceNotFoundException",
            code: ERROR_CODE_JOB_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "StartSigningJob against a profile that was never created",
            method: "POST",
            uri: "/signing-jobs".to_string(),
            body: Some(start_job_request("No_Such_Profile", "tok-missing")),
            status: 404,
            shape: "ResourceNotFoundException",
            code: ERROR_CODE_PROFILE_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "StartSigningJob against a canceled profile",
            method: "POST",
            uri: "/signing-jobs".to_string(),
            body: Some(start_job_request("Canceled_Profile", "tok-canceled")),
            status: 404,
            shape: "ResourceNotFoundException",
            // The status is what forbids the call, so the code names it.
            code: "ProfileCanceled".to_string(),
        },
        RefusalCase {
            case: "StartSigningJob against a revoked profile",
            method: "POST",
            uri: "/signing-jobs".to_string(),
            body: Some(start_job_request("Revoked_Profile", "tok-revoked")),
            status: 404,
            shape: "ResourceNotFoundException",
            code: "ProfileRevoked".to_string(),
        },
        RefusalCase {
            case: "PutSigningProfile with no signing material",
            method: "PUT",
            uri: "/signing-profiles/No_Material_Profile".to_string(),
            body: Some(json!({ "platformId": "AWSIoTDeviceManagement-SHA256-ECDSA" })),
            status: 400,
            shape: "ValidationException",
            code: ERROR_CODE_VALIDATION.to_string(),
        },
        RefusalCase {
            case: "RevokeSigningProfile naming a version the profile never had",
            method: "PUT",
            uri: "/signing-profiles/Active_Profile/revoke".to_string(),
            body: Some(json!({
                "profileVersion": "zzzzzzzzzz",
                "reason": "wrong version",
                "effectiveTime": 1_700_000_000,
            })),
            status: 404,
            shape: "ResourceNotFoundException",
            // The PROFILE was resolved and a sub-resource of it was not, which is
            // a different condition from a profile that does not exist — so the
            // code is the shape's own name here, not the profile-not-found code
            // the rows above carry.
            code: ERROR_CODE_NOT_FOUND.to_string(),
        },
        RefusalCase {
            case: "StartSigningJob whose source names no S3 object",
            method: "POST",
            uri: "/signing-jobs".to_string(),
            body: Some(json!({
                "source": {},
                "destination": { "s3": { "bucketName": "dst-bucket" } },
                "profileName": "Active_Profile",
                "clientRequestToken": "tok-no-source",
            })),
            status: 400,
            shape: "ValidationException",
            code: ERROR_CODE_VALIDATION.to_string(),
        },
        RefusalCase {
            case: "StartSigningJob whose destination names no bucket",
            method: "POST",
            uri: "/signing-jobs".to_string(),
            body: Some(json!({
                "source": {
                    "s3": { "bucketName": "src-bucket", "key": "code.bin", "version": "1" },
                },
                "destination": { "s3": { "prefix": "signed/" } },
                "profileName": "Active_Profile",
                "clientRequestToken": "tok-no-destination",
            })),
            status: 400,
            shape: "ValidationException",
            code: ERROR_CODE_VALIDATION.to_string(),
        },
    ];
    for token in bad_tokens {
        cases.push(RefusalCase {
            case: "ListSigningProfiles with a nextToken no page ever returned",
            method: "GET",
            uri: format!(
                "/signing-profiles?nextToken={}",
                token.replace(' ', "%20")
            ),
            body: None,
            status: 400,
            shape: "ValidationException",
            code: ERROR_CODE_INVALID_NEXT_TOKEN.to_string(),
        });
    }

    for row in cases {
        let case = row.case;
        let answer = call(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            row.status, answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(row.code.as_str()),
            "case {case:?}: expected the modelled code {:?}, got {:?}",
            row.code,
            answer.body
        );
        assert!(
            answer.message().is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a message beside the code, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.shape(),
            row.shape,
            "case {case:?}: expected the error shape {}, got {:?}",
            row.shape,
            answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(row.shape),
            "case {case:?}: expected x-amzn-errortype {}, got {:?}",
            row.shape,
            answer.error_type
        );
    }

    // The refused profile was never created: the refusals above wrote nothing.
    assert_eq!(
        get_profile(&router, "No_Material_Profile").await.status,
        404,
        "a refused PutSigningProfile must not have stored a profile"
    );
}

/// `signingMaterial` is required exactly on the platforms whose profile holds the
/// certificate its signatures are issued under, and refused requests store
/// nothing.
///
/// One row per published platform, each put twice: with the certificate and
/// without it.
#[tokio::test]
async fn signing_material_is_required_exactly_on_the_platforms_that_publish_it() {
    let router = emulator();

    for (ordinal, (platform, requires_material)) in PLATFORM_CASES.iter().enumerate() {
        // With the certificate, every platform accepts the profile.
        let with_name = format!("With_Material_{ordinal}");
        let with = put_profile(
            &router,
            &with_name,
            json!({
                "platformId": platform,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            }),
        )
        .await;
        assert_eq!(
            with.status, 200,
            "case {platform} with signingMaterial: expected 200, got {} {:?}",
            with.status, with.body
        );

        // Without it, only a platform AWS Signer issues the certificate for is
        // accepted.
        let without_name = format!("Without_Material_{ordinal}");
        let without = put_profile(&router, &without_name, json!({ "platformId": platform })).await;
        let expected_status = if *requires_material { 400 } else { 200 };
        assert_eq!(
            without.status, expected_status,
            "case {platform} without signingMaterial: expected {expected_status}, got {} {:?}",
            without.status, without.body
        );

        let read_back = get_profile(&router, &without_name).await;
        if *requires_material {
            assert_eq!(
                without.code(),
                Some(ERROR_CODE_VALIDATION),
                "case {platform} without signingMaterial: expected the modelled code, got {:?}",
                without.body
            );
            assert_eq!(
                read_back.status, 404,
                "case {platform}: a refused put must store nothing, got {} {:?}",
                read_back.status, read_back.body
            );
        } else {
            assert_eq!(
                read_back.status, 200,
                "case {platform}: an accepted put must be readable, got {} {:?}",
                read_back.status, read_back.body
            );
            assert!(
                read_back.body["signingMaterial"].is_null(),
                "case {platform}: expected no signingMaterial on a profile created without it, \
                 got {:?}",
                read_back.body["signingMaterial"]
            );
        }
    }
}

/// What a cancel does to each status a profile can be in: an `Active` profile
/// becomes `Canceled`, a profile already `Canceled` keeps that status and still
/// answers the recorded 204 — and a REVOKED profile is refused, because a
/// revocation cannot be weakened into a cancel. A profile that was revoked and
/// then RE-PUT keeps its revocation, so it is still `Revoked` and the cancel is
/// refused there too: the answer follows the stored status, and a write to the
/// profile does not restore the trust the revocation ended.
#[tokio::test]
async fn a_cancel_moves_only_a_profile_its_source_status_holds_for() {
    // One row per status a profile can be in when the cancel arrives, plus the
    // row whose create is repeated after a revoke. 204 is the recorded success
    // for this DELETE; 400 with the code naming the status is the recorded
    // refusal.
    let rows: [(&str, &str, u16, &str, &str); 4] = [
        ("Active", "Active_Cancel", 204, "Canceled", ""),
        ("Canceled", "Canceled_Cancel", 204, "Canceled", ""),
        ("Revoked", "Revoked_Cancel", 400, "Revoked", "ProfileRevoked"),
        (
            "Revoked then re-created",
            "Recreated_Cancel",
            400,
            "Revoked",
            "ProfileRevoked",
        ),
    ];

    for (status_before, name, expected_status, status_after, expected_code) in rows {
        let router = emulator();
        let version = seed_profile(&router, name).await;
        match status_before {
            "Canceled" => {
                assert_eq!(
                    cancel_profile(&router, name).await.status,
                    204,
                    "case {status_before}: the first cancel must succeed"
                );
            }
            "Revoked" | "Revoked then re-created" => {
                let revoked =
                    revoke_profile(&router, name, &version, "key-compromise", 1_700_000_000).await;
                assert_eq!(
                    revoked.status, 204,
                    "case {status_before}: the revoke must succeed, got {:?}",
                    revoked.body
                );
            }
            _ => {}
        }
        if status_before == "Revoked then re-created" {
            // The name is taken for good, so the create is refused and the
            // revoked profile stands — and is still refused the cancel.
            let refused = put_profile(
                &router,
                name,
                json!({
                    "platformId": "AWSLambda-SHA384-ECDSA",
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                }),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {status_before}: expected the repeated create to be refused, got {} {:?}",
                refused.status, refused.body
            );
        }

        let answer = cancel_profile(&router, name).await;
        assert_eq!(
            answer.status, expected_status,
            "case {status_before}: expected HTTP {expected_status}, got {} {:?}",
            answer.status, answer.body
        );
        if expected_code.is_empty() {
            assert!(
                answer.error_type.is_none(),
                "case {status_before}: expected no x-amzn-errortype on a success, got {:?}",
                answer.error_type
            );
        } else {
            assert_eq!(
                answer.code(),
                Some(expected_code),
                "case {status_before}: expected the code {expected_code}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("ValidationException"),
                "case {status_before}: expected x-amzn-errortype on a refusal, got {:?}",
                answer.error_type
            );
        }

        // The status the cancel left behind.
        let got = get_profile(&router, name).await;
        assert_eq!(
            got.str_member("status"),
            status_after,
            "case {status_before}: expected status {status_after} after the cancel, got {:?}",
            got.body["status"]
        );
    }
}

/// `RevokeSigningProfile` records WHY and WHEN on the profile, and
/// `GetSigningProfile` reads it back: the reason becomes `statusReason`, and the
/// effective time the request named is what the revocation record carries.
///
/// Rows vary the reason (including at `RevocationReasonString`'s `@length`
/// minimum of 1 and maximum of 500) and the effective instant.
#[tokio::test]
async fn revoking_a_profile_records_the_reason_and_the_effective_time() {
    let rows: [(&str, String, i64); 3] = [
        ("Revoked_Rotate", "rotate-signing-key".to_string(), 1_700_000_000),
        ("Revoked_Minimum", "x".to_string(), 0),
        ("Revoked_Maximum", "y".repeat(500), 2_000_000_000),
    ];

    for (name, reason, effective_time) in rows {
        let router = emulator();
        let version = seed_profile(&router, name).await;
        let revoked = revoke_profile(&router, name, &version, &reason, effective_time).await;
        assert_eq!(
            revoked.status, 204,
            "case {name}: expected RevokeSigningProfile 204, got {} {:?}",
            revoked.status, revoked.body
        );

        let got = get_profile(&router, name).await;
        assert_eq!(
            got.status, 200,
            "case {name}: a revoked profile stays viewable, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.str_member("status"),
            "Revoked",
            "case {name}: expected status Revoked, got {:?}",
            got.body["status"]
        );
        assert_eq!(
            got.str_member("statusReason"),
            reason,
            "case {name}: expected statusReason to be the reason given, got {:?}",
            got.body["statusReason"]
        );
        let record = &got.body["revocationRecord"];
        assert_eq!(
            record["revocationEffectiveFrom"].as_f64(),
            Some(effective_time as f64),
            "case {name}: expected revocationEffectiveFrom {effective_time}, got {record:?}"
        );
        assert!(
            record["revokedAt"].is_number(),
            "case {name}: expected a revokedAt timestamp, got {record:?}"
        );
        assert_eq!(
            record["revokedBy"].as_str(),
            Some(caller_principal().as_str()),
            "case {name}: expected revokedBy to name the resolved caller principal, got {record:?}"
        );

        // The record is a read, not a write: a second reader sees the same one.
        let again = get_profile(&router, name).await;
        assert_eq!(
            again.body["revocationRecord"], got.body["revocationRecord"],
            "case {name}: expected the revocation record to be stable across reads"
        );
    }
}
