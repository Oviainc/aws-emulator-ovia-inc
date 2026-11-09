// SPDX-License-Identifier: CC0-1.0

//! A signing profile's revocation is recorded ONCE, end to end over HTTP.
//!
//! The rule under test: a revocation is a fact about trust that has ALREADY
//! ended, so the first record stands — a later `RevokeSigningProfile` on a
//! profile that is already `Revoked` still answers success and changes no stored
//! field of that record (`revocationEffectiveFrom`, `revokedAt`, `revokedBy`) nor
//! the `statusReason` the first call recorded. `revocationEffectiveFrom` is the
//! instant after which signatures stop being trusted
//! (`smithy_query(query="doc", shape="RevokeSigningProfileRequest",
//! member="effectiveTime")`), so letting a later call move it would restore trust
//! in signatures the first revocation had already placed after it.
//!
//! The cases are a table whose rows differ along what the rule generalises over:
//! where the repeat's `effectiveTime` sits relative to the first one's (later,
//! earlier, equal), how many repeats there are, which version of the profile the
//! repeat names, and which identity signs the repeat.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for; a profile is a resource of it.
const REGION: &str = "eu-west-1";

/// The access key id the FIRST revoke of each case is signed with.
const FIRST_REVOKER_KEY: &str = "AKIDFIRSTREVOKER";

/// A second, different access key id, so a repeat can be signed with another
/// credential than the one that revoked.
const OTHER_REVOKER_KEY: &str = "AKIDOTHERREVOKER";

/// The account id every generated emulator presents, spelled out so a wrong
/// account in the handler fails this test.
const ACCOUNT_ID: &str = "000000000000";

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
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// Call the emulator, signed by `access_key` for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    access_key: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/20260101/{REGION}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    );
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
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

async fn put_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        FIRST_REVOKER_KEY,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": "AWSLambda-SHA384-ECDSA" })),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        FIRST_REVOKER_KEY,
        "GET",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    access_key: &str,
    name: &str,
    version: &str,
    reason: &str,
    effective_time: i64,
) -> Answer {
    call(
        router,
        access_key,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": reason,
            "effectiveTime": effective_time,
        })),
    )
    .await
}

/// One repeat revoke to perform after the profile is already revoked.
struct Repeat {
    reason: &'static str,
    /// The `effectiveTime` this repeat sends, as epoch seconds.
    effective_time: i64,
    /// The identity that signs it.
    access_key: &'static str,
}

/// A profile to revoke once and then revoke again, with the repeats to perform.
struct RepeatCase {
    name: &'static str,
    first_reason: &'static str,
    /// The `effectiveTime` the one recorded revocation carries.
    first_effective_time: i64,
    repeats: Vec<Repeat>,
}

fn repeat_cases() -> Vec<RepeatCase> {
    vec![
        // The repeat's effective instant is LATER than the recorded one: the case
        // where overwriting would move the trust boundary forward.
        RepeatCase {
            name: "Repeat_Later_Effective",
            first_reason: "key compromised",
            first_effective_time: 1_000_000_000,
            repeats: vec![Repeat {
                reason: "second call",
                effective_time: 2_000_000_000,
                access_key: FIRST_REVOKER_KEY,
            }],
        },
        // EARLIER than the recorded one.
        RepeatCase {
            name: "Repeat_Earlier_Effective",
            first_reason: "certificate material leaked",
            first_effective_time: 2_000_000_000,
            repeats: vec![Repeat {
                reason: "re-revoked from an earlier instant",
                effective_time: 1_000_000_000,
                access_key: FIRST_REVOKER_KEY,
            }],
        },
        // The SAME effective instant, a different reason: the reason must not be
        // restated either.
        RepeatCase {
            name: "Repeat_Same_Effective",
            first_reason: "rotate-signing-key",
            first_effective_time: 1_700_000_000,
            repeats: vec![Repeat {
                reason: "x",
                effective_time: 1_700_000_000,
                access_key: FIRST_REVOKER_KEY,
            }],
        },
        // Three repeats, including the epoch and a far-future instant: the record
        // is written once, not once per call.
        RepeatCase {
            name: "Repeat_Three_Times",
            first_reason: "revoked once",
            first_effective_time: 1_600_000_000,
            repeats: vec![
                Repeat {
                    reason: "second call",
                    effective_time: 0,
                    access_key: FIRST_REVOKER_KEY,
                },
                Repeat {
                    reason: "third call",
                    effective_time: 4_000_000_000,
                    access_key: OTHER_REVOKER_KEY,
                },
                Repeat {
                    reason: "fourth call",
                    effective_time: 1_600_000_001,
                    access_key: FIRST_REVOKER_KEY,
                },
            ],
        },
        // The repeat is signed by ANOTHER identity: the recorded revoker is the
        // one that ended trust, not the latest caller.
        RepeatCase {
            name: "Repeat_Other_Revoker",
            first_reason: "revoked by the first caller",
            first_effective_time: 1_800_000_000,
            repeats: vec![Repeat {
                reason: "revoked by somebody else",
                effective_time: 1_850_000_000,
                access_key: OTHER_REVOKER_KEY,
            }],
        },
    ]
}

/// A repeated revoke of an already-revoked profile answers success and leaves the
/// first revocation record — its effective instant, time, revoker and the reason
/// it recorded — exactly as it was.
#[tokio::test]
async fn a_repeated_revoke_leaves_the_first_record_standing() {
    let router = emulator();

    for case in repeat_cases() {
        let created = put_profile(&router, case.name).await;
        assert_eq!(
            created.status, 200,
            "case {}: seeding the profile: {} {:?}",
            case.name, created.status, created.body
        );
        let version = created.str_member("profileVersion");

        let first = revoke_profile(
            &router,
            FIRST_REVOKER_KEY,
            case.name,
            &version,
            case.first_reason,
            case.first_effective_time,
        )
        .await;
        assert_eq!(
            first.status, 204,
            "case {}: expected the first RevokeSigningProfile to answer 204, got {} {:?}",
            case.name, first.status, first.body
        );

        // What the ONE revocation recorded: the first call's instant, the first
        // caller's identity, and its reason.
        let recorded = get_profile(&router, case.name).await;
        let record = recorded.body["revocationRecord"].clone();
        assert_eq!(
            record["revocationEffectiveFrom"],
            json!(case.first_effective_time),
            "case {}: expected revocationEffectiveFrom {}, got {:?}",
            case.name,
            case.first_effective_time,
            record["revocationEffectiveFrom"]
        );
        assert_eq!(
            record["revokedBy"],
            json!(format!("arn:aws:iam::{ACCOUNT_ID}:user/emulator")),
            "case {}: expected revokedBy to name the principal the revoking credentials \
             resolve to, got {:?}",
            case.name,
            record["revokedBy"]
        );
        assert!(
            record["revokedAt"].is_number(),
            "case {}: expected a revokedAt timestamp, got {:?}",
            case.name,
            record["revokedAt"]
        );
        assert_eq!(
            recorded.str_member("statusReason"),
            case.first_reason,
            "case {}: expected statusReason to be the first call's reason, got {:?}",
            case.name,
            recorded.body["statusReason"]
        );

        for (ordinal, repeat) in case.repeats.iter().enumerate() {
            let again = revoke_profile(
                &router,
                repeat.access_key,
                case.name,
                &version,
                repeat.reason,
                repeat.effective_time,
            )
            .await;
            assert_eq!(
                again.status, 204,
                "case {} repeat {ordinal}: expected a repeated RevokeSigningProfile to answer \
                 204, got {} {:?}",
                case.name, again.status, again.body
            );
            assert!(
                again.raw_body.is_empty(),
                "case {} repeat {ordinal}: expected an empty body on 204, got {:?}",
                case.name,
                String::from_utf8_lossy(&again.raw_body)
            );

            let after = get_profile(&router, case.name).await;
            assert_eq!(
                after.body["revocationRecord"], record,
                "case {} repeat {ordinal}: expected the first revocation record to stand, got \
                 {:?} instead of {record:?}",
                case.name, after.body["revocationRecord"]
            );
            assert_eq!(
                after.str_member("statusReason"),
                case.first_reason,
                "case {} repeat {ordinal}: expected the first call's reason to stand, got {:?}",
                case.name,
                after.body["statusReason"]
            );
            assert_eq!(
                after.str_member("status"),
                "Revoked",
                "case {} repeat {ordinal}: expected the profile to stay Revoked, got {:?}",
                case.name,
                after.body["status"]
            );
            // The repeat is not a rewrite of anything else either: the profile
            // keeps the version it had.
            assert_eq!(
                after.str_member("profileVersion"),
                version,
                "case {} repeat {ordinal}: expected the profile to keep its version, got {:?}",
                case.name,
                after.body["profileVersion"]
            );
        }
    }
}

/// The refusals a repeat is still subject to: the already-revoked profile does not
/// make an unknown profile or an unknown version revocable, and a refused repeat
/// writes nothing.
#[tokio::test]
async fn a_repeat_naming_an_unknown_resource_is_still_refused() {
    let router = emulator();
    let put = put_profile(&router, "Revoked_Then_Misnamed").await;
    assert_eq!(put.status, 200, "seeding the profile: {:?}", put.body);
    let version = put.str_member("profileVersion");

    let first = revoke_profile(
        &router,
        FIRST_REVOKER_KEY,
        "Revoked_Then_Misnamed",
        &version,
        "key compromised",
        1_000_000_000,
    )
    .await;
    assert_eq!(
        first.status, 204,
        "expected the first revoke to answer 204, got {} {:?}",
        first.status, first.body
    );
    let record = get_profile(&router, "Revoked_Then_Misnamed").await.body["revocationRecord"]
        .clone();

    // (case, profile name, profileVersion, expected status)
    let cases: Vec<(&str, &str, String, u16)> = vec![
        (
            "a repeat on an unknown profile name",
            "No_Such_Profile",
            version.clone(),
            404,
        ),
        (
            "a repeat naming a version the revoked profile never had",
            "Revoked_Then_Misnamed",
            "ZZZZZZZZZZ".to_string(),
            404,
        ),
        (
            "a repeat whose profileVersion is outside the modelled pattern",
            "Revoked_Then_Misnamed",
            "not a version".to_string(),
            400,
        ),
    ];

    for (case, name, profile_version, status) in cases {
        let answer = revoke_profile(
            &router,
            FIRST_REVOKER_KEY,
            name,
            &profile_version,
            "second call",
            2_000_000_000,
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
    }

    let after = get_profile(&router, "Revoked_Then_Misnamed").await;
    assert_eq!(
        after.body["revocationRecord"], record,
        "expected the refused repeats to have written nothing, got {:?} instead of {record:?}",
        after.body["revocationRecord"]
    );
}
