// SPDX-License-Identifier: CC0-1.0

//! What a Signer signing-profile answer reports, end to end over HTTP: the
//! profile's full member set, its identity, and its revocation.
//!
//! Every case goes through the router, so it exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes.
//! The rules under test:
//!
//! * `GetSigningProfile` reports EVERY member AWS answers with for the stored
//!   profile — including the ones the Smithy model does not declare
//!   (`profileToken`, `platform`) — with `tags` present even when the profile has
//!   none, and `signatureValidityPeriod` reporting the documented default when
//!   the create named none;
//! * `PutSigningProfile`'s answer carries the created profile's resource identity
//!   (`tags.resourceArn`, `tags.internalId`);
//! * `RevokeSigningProfile` is the transition it says it is: the profile moves to
//!   `Revoked`, reports the request's reason and carries a revocation record,
//!   which every later read reports unchanged;
//! * a successful `CancelSigningProfile` answers `204 No Content` with no body.
//!
//! The cases are tables: a row per input class the rules generalise over (which
//! optional members the create carried, tagged or untagged, the validity-period
//! unit, which version is revoked, the revocation reason's length).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::SIGNING_PROFILE_STATUS_VALUES;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. It is what routes a request to
/// signer, and its region is what a profile ARN is built for.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDREVOKER/20260101/eu-west-1/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The IAM principal the emulator resolves every signed request as, whatever
/// access key id the request carried: the one IAM user whose credentials it
/// publishes, which is the identity `sts:GetCallerIdentity` answers with.
fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator")
}

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// The validity period a profile created without one reports. Source: the model's
/// `PutSigningProfileRequest$signatureValidityPeriod` doc — "If unspecified, the
/// default is 135 months." Spelled out rather than imported so a wrong default in
/// the handler fails this test.
fn documented_default_validity_period() -> Value {
    json!({ "value": 135, "type": "MONTHS" })
}

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
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
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
        .header("host", "signer.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
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

async fn revoke_profile(router: &MultiServiceRouter, name: &str, request: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(request),
    )
    .await
}

async fn cancel_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await
}

/// True when `value` has the shape AWS mints opaque ids in: a UUID,
/// `8-4-4-4-12` lowercase hex digits. The VALUE is server-generated and no
/// requirement; the shape is.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

/// A profile shape to create and read back. The rows differ along what the rules
/// generalise over: which optional members the create carries, whether it carries
/// tags, and the validity-period unit (including none at all).
struct ProfileCase {
    name: &'static str,
    request: Value,
}

fn profile_cases() -> Vec<ProfileCase> {
    vec![
        // The smallest request the model admits: no tags, no validity period.
        ProfileCase {
            name: "Minimal_Profile",
            request: json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
        },
        // Tagged, with a MONTHS period — the shape the recorded Lambda flow uses.
        ProfileCase {
            name: "Tagged_Lambda_Profile",
            request: json!({
                "platformId": "AWSLambda-SHA384-ECDSA",
                "signatureValidityPeriod": { "value": 135, "type": "MONTHS" },
                "tags": { "owner": "platform" },
            }),
        },
        // A DAYS period, signing material, parameters and two tags.
        ProfileCase {
            name: "Iot_Profile_2",
            request: json!({
                "platformId": "AWSIoTDeviceManagement-SHA256-ECDSA",
                "signingMaterial": {
                    "certificateArn": "arn:aws:acm:eu-west-1:000000000000:certificate/abc",
                },
                "signatureValidityPeriod": { "value": 7, "type": "DAYS" },
                "signingParameters": { "env": "prod" },
                "tags": { "owner": "iot", "tier": "gold" },
            }),
        },
        // The container-image platform, with a YEARS period. AWS Signer issues
        // the certificate for that platform itself, so its profiles need no
        // `signingMaterial` — but naming one is still accepted, which is what
        // this row carries.
        ProfileCase {
            name: "Container_Profile",
            request: json!({
                "platformId": "Notation-OCI-SHA384-ECDSA",
                "signingMaterial": {
                    "certificateArn": "arn:aws:acm:eu-west-1:000000000000:certificate/abc",
                },
                "signatureValidityPeriod": { "value": 2, "type": "YEARS" },
            }),
        },
    ]
}

/// The create answer reports the new profile's resource identity, and the read
/// answer reports every member AWS answers with — including the two the model does
/// not declare, and `tags` even when the profile has none.
#[tokio::test]
async fn create_and_read_report_the_whole_profile() {
    let router = emulator();

    for case in profile_cases() {
        let put = put_profile(&router, case.name, case.request.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {}: expected PutSigningProfile 200, got {} {:?}",
            case.name, put.status, put.body
        );

        let expected_arn = format!(
            "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/{}",
            case.name
        );
        // The create answer's identity object: the profile ARN, and an opaque
        // server-generated id.
        assert_eq!(
            put.body["tags"]["resourceArn"],
            json!(expected_arn),
            "case {}: expected tags.resourceArn {expected_arn}, got {:?}",
            case.name,
            put.body["tags"]
        );
        let internal_id = put.body["tags"]["internalId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_uuid_shaped(&internal_id),
            "case {}: expected a UUID-shaped tags.internalId, got {internal_id:?}",
            case.name
        );

        let got = get_profile(&router, case.name).await;
        assert_eq!(
            got.status, 200,
            "case {}: expected GetSigningProfile 200, got {} {:?}",
            case.name, got.status, got.body
        );

        // `platform` is the legacy spelling of `platformId`: both report the
        // platform the profile was created on.
        assert_eq!(
            got.body["platform"], case.request["platformId"],
            "case {}: expected platform {:?}, got {:?}",
            case.name, case.request["platformId"], got.body["platform"]
        );
        assert_eq!(
            got.body["platformId"], case.request["platformId"],
            "case {}: expected platformId {:?}, got {:?}",
            case.name, case.request["platformId"], got.body["platformId"]
        );

        // `profileToken` is the profile's opaque id: present, UUID-shaped, and
        // the same id the create reported as its internal id.
        let token = got.str_member("profileToken");
        assert!(
            is_uuid_shaped(&token),
            "case {}: expected a UUID-shaped profileToken, got {token:?}",
            case.name
        );
        assert_eq!(
            token, internal_id,
            "case {}: expected profileToken to be the id the create reported ({internal_id}), got {token}",
            case.name
        );

        // `tags` is always reported — the stored tags, or an empty map.
        let expected_tags = if case.request["tags"].is_null() {
            json!({})
        } else {
            case.request["tags"].clone()
        };
        assert_eq!(
            got.body["tags"], expected_tags,
            "case {}: expected tags {expected_tags:?}, got {:?}",
            case.name, got.body["tags"]
        );

        // A validity period the create named round-trips; one it did not name
        // reports the documented default.
        let expected_period = if case.request["signatureValidityPeriod"].is_null() {
            documented_default_validity_period()
        } else {
            case.request["signatureValidityPeriod"].clone()
        };
        assert_eq!(
            got.body["signatureValidityPeriod"], expected_period,
            "case {}: expected signatureValidityPeriod {expected_period:?}, got {:?}",
            case.name, got.body["signatureValidityPeriod"]
        );

        // A profile nobody revoked carries no revocation record and no reason.
        assert!(
            got.body["revocationRecord"].is_null(),
            "case {}: expected no revocationRecord on a profile nobody revoked, got {:?}",
            case.name,
            got.body["revocationRecord"]
        );
        assert!(
            got.body["statusReason"].is_null(),
            "case {}: expected no statusReason on an Active profile, got {:?}",
            case.name,
            got.body["statusReason"]
        );
    }
}

/// A profile keeps ONE opaque id for its whole life: the create reports it, every
/// read reports the same one, and two profiles never share one — and a refused
/// second create under the name mints no new id.
#[tokio::test]
async fn the_profile_token_is_the_profile_identity() {
    let router = emulator();
    let mut tokens = Vec::new();

    for ordinal in 0..5 {
        let name = format!("Token_Profile_{ordinal}");
        let mut seen: Option<String> = None;
        // One create, then two more under the same name: the name is taken, so
        // the later ones are refused and report no id at all.
        for put_ordinal in 0..3 {
            let put = put_profile(
                &router,
                &name,
                json!({
                    "platformId": "AWSLambda-SHA384-ECDSA",
                    "signingParameters": { "generation": put_ordinal.to_string() },
                }),
            )
            .await;
            let expected_status = if put_ordinal == 0 { 200 } else { 400 };
            assert_eq!(
                put.status, expected_status,
                "case {name} put {put_ordinal}: expected {expected_status}, got {} {:?}",
                put.status, put.body
            );
            let reported = put.body["tags"]["internalId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if put_ordinal == 0 {
                assert!(
                    !reported.is_empty(),
                    "case {name}: expected the create to report an id, got {:?}",
                    put.body
                );
                seen = Some(reported);
            } else {
                assert!(
                    reported.is_empty(),
                    "case {name} put {put_ordinal}: expected the refusal to report no id, got \
                     {reported:?}"
                );
            }
        }

        let token = get_profile(&router, &name).await.str_member("profileToken");
        assert_eq!(
            Some(&token),
            seen.as_ref(),
            "case {name}: expected GetSigningProfile to report the id the create did"
        );
        assert!(
            !tokens.contains(&token),
            "case {name}: expected an id no other profile holds, got {token:?} among {tokens:?}"
        );
        tokens.push(token);
    }
}

/// One revocation to perform and what it must report. The rows differ along the
/// reason's length (both published bounds of `RevocationReasonString`'s
/// `@length(min: 1, max: 500)`) and the effective time's relation to now.
struct RevocationCase {
    name: &'static str,
    reason: String,
    /// The `effectiveTime` to send, as epoch seconds.
    effective_time: i64,
}

fn revocation_cases() -> Vec<RevocationCase> {
    vec![
        RevocationCase {
            name: "Revoked_Rotating_Key",
            reason: "rotate-signing-key".to_string(),
            effective_time: 1_767_225_600,
        },
        // `RevocationReasonString`'s @length minimum of 1.
        RevocationCase {
            name: "Revoked_Shortest_Reason",
            reason: "x".to_string(),
            effective_time: 0,
        },
        // Its @length maximum of 500.
        RevocationCase {
            name: "Revoked_Longest_Reason",
            reason: "r".repeat(500),
            effective_time: 2_000_000_000,
        },
        // A fourth reason and a fourth effective instant.
        RevocationCase {
            name: "Revoked_Compromised_Material",
            reason: "compromised certificate material".to_string(),
            effective_time: 1_700_000_000,
        },
    ]
}

/// Revoking a profile is a recorded transition: the profile reports `Revoked`, the
/// reason the request gave, and a revocation record — and every later read reports
/// the same thing.
#[tokio::test]
async fn revoke_records_the_transition_and_every_read_reports_it() {
    let router = emulator();

    for case in revocation_cases() {
        let created = put_profile(
            &router,
            case.name,
            json!({ "platformId": "AWSLambda-SHA384-ECDSA", "tags": { "owner": "platform" } }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: seeding the profile: {:?}",
            case.name, created.body
        );
        let version = created.str_member("profileVersion");

        // Before the revoke the profile is Active with nothing revoked.
        let before = get_profile(&router, case.name).await;
        assert_eq!(
            before.str_member("status"),
            "Active",
            "case {}: expected a fresh profile to be Active, got {:?}",
            case.name,
            before.body["status"]
        );

        let revoked = revoke_profile(
            &router,
            case.name,
            json!({
                "profileVersion": version,
                "reason": case.reason,
                "effectiveTime": case.effective_time,
            }),
        )
        .await;
        assert_eq!(
            revoked.status, 204,
            "case {}: expected RevokeSigningProfile 204, got {} {:?}",
            case.name, revoked.status, revoked.body
        );
        assert!(
            revoked.raw_body.is_empty(),
            "case {}: expected an empty body on 204, got {:?}",
            case.name,
            String::from_utf8_lossy(&revoked.raw_body)
        );

        let after = get_profile(&router, case.name).await;
        assert_eq!(
            after.str_member("status"),
            "Revoked",
            "case {}: expected status Revoked, got {:?}",
            case.name,
            after.body["status"]
        );
        assert!(
            SIGNING_PROFILE_STATUS_VALUES.contains(&after.str_member("status").as_str()),
            "case {}: status {:?} is not one of the modelled {SIGNING_PROFILE_STATUS_VALUES:?}",
            case.name,
            after.body["status"]
        );
        assert_eq!(
            after.str_member("statusReason"),
            case.reason,
            "case {}: expected statusReason to be the request's reason, got {:?}",
            case.name,
            after.body["statusReason"]
        );
        let record = &after.body["revocationRecord"];
        assert_eq!(
            record["revocationEffectiveFrom"],
            json!(case.effective_time),
            "case {}: expected revocationEffectiveFrom to be the request's effectiveTime, got {:?}",
            case.name,
            record["revocationEffectiveFrom"]
        );
        assert!(
            record["revokedAt"].is_number(),
            "case {}: expected a revokedAt timestamp, got {:?}",
            case.name,
            record["revokedAt"]
        );
        assert_eq!(
            record["revokedBy"],
            json!(caller_principal()),
            "case {}: expected revokedBy to name the resolved caller principal, got {:?}",
            case.name,
            record["revokedBy"]
        );

        // The profile keeps everything else it had: a revoke is not a rewrite.
        assert_eq!(
            after.str_member("profileVersion"),
            version,
            "case {}: expected the profile to keep its version, got {:?}",
            case.name,
            after.body["profileVersion"]
        );
        assert_eq!(
            after.body["tags"],
            json!({ "owner": "platform" }),
            "case {}: expected the tags to survive the revoke, got {:?}",
            case.name,
            after.body["tags"]
        );

        // Reading does not move anything: a second read reports the same record.
        let again = get_profile(&router, case.name).await;
        assert_eq!(
            again.body["revocationRecord"], after.body["revocationRecord"],
            "case {}: expected the revocation record to be stable across reads, got {:?} then {:?}",
            case.name, after.body["revocationRecord"], again.body["revocationRecord"]
        );
        assert_eq!(
            again.str_member("status"),
            "Revoked",
            "case {}: expected the status to be stable across reads, got {:?}",
            case.name,
            again.body["status"]
        );

        // A revoked profile signs nothing more.
        let refused = call(
            &router,
            "POST",
            "/signing-jobs",
            Some(json!({
                "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
                "destination": { "s3": { "bucketName": "dst" } },
                "profileName": case.name,
                "clientRequestToken": format!("tok-{}", case.name),
            })),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {}: expected a revoked profile to refuse a new job with 404, got {} {:?}",
            case.name, refused.status, refused.body
        );
    }
}

/// What revoking refuses, and with which status and code: a profile or a version
/// this account holds none of is a modelled 404, and nothing is written.
#[tokio::test]
async fn revoking_an_unknown_profile_or_version_is_refused() {
    let router = emulator();
    let put = put_profile(
        &router,
        "Revocable_Profile",
        json!({ "platformId": "AWSLambda-SHA384-ECDSA" }),
    )
    .await;
    assert_eq!(put.status, 200, "seeding the profile: {:?}", put.body);
    let version = put.str_member("profileVersion");

    // (case, profile name, profileVersion, expected status, expected code)
    let cases: Vec<(&str, &str, String, u16, &str)> = vec![
        (
            "an unknown profile name",
            "No_Such_Profile",
            version.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a version the profile never had",
            "Revocable_Profile",
            "ZZZZZZZZZZ".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a profileVersion outside the modelled pattern",
            "Revocable_Profile",
            "not a version".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, name, profile_version, status, code) in cases {
        let answer = revoke_profile(
            &router,
            name,
            json!({
                "profileVersion": profile_version,
                "reason": "because",
                "effectiveTime": 1_700_000_000,
            }),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().contains(code),
            "case {case:?}: expected error code {code}, got {:?}",
            answer.body
        );
    }

    // And the profile the refusals named is untouched.
    let got = get_profile(&router, "Revocable_Profile").await;
    assert_eq!(
        got.str_member("status"),
        "Active",
        "expected the refused revokes to have written nothing, got {:?}",
        got.body
    );
    assert!(
        got.body["revocationRecord"].is_null(),
        "expected the refused revokes to have written no revocation record, got {:?}",
        got.body["revocationRecord"]
    );
}

/// A successful cancel answers `204 No Content` with an empty body, whatever the
/// profile looked like — and the cancel is still recorded.
#[tokio::test]
async fn cancel_answers_204_with_no_body() {
    let router = emulator();

    for case in profile_cases() {
        let put = put_profile(&router, case.name, case.request.clone()).await;
        assert_eq!(
            put.status, 200,
            "case {}: seeding the profile: {:?}",
            case.name, put.body
        );

        let canceled = cancel_profile(&router, case.name).await;
        assert_eq!(
            canceled.status, 204,
            "case {}: expected CancelSigningProfile 204, got {} {:?}",
            case.name, canceled.status, canceled.body
        );
        assert!(
            canceled.raw_body.is_empty(),
            "case {}: expected an empty body on 204, got {:?}",
            case.name,
            String::from_utf8_lossy(&canceled.raw_body)
        );

        let got = get_profile(&router, case.name).await;
        assert_eq!(
            got.str_member("status"),
            "Canceled",
            "case {}: expected the cancel to be recorded, got {:?}",
            case.name,
            got.body["status"]
        );
    }

    // Cancelling what does not exist is still the modelled 404, not a 204.
    let missing = cancel_profile(&router, "No_Such_Profile").await;
    assert_eq!(
        missing.status, 404,
        "expected cancelling an unknown profile to answer 404, got {} {:?}",
        missing.status, missing.body
    );
}
