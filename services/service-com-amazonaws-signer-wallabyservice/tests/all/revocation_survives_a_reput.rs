// SPDX-License-Identifier: CC0-1.0

//! A signing profile's revocation survives a later write to that profile, end to
//! end over HTTP.
//!
//! The rule under test: a revocation is a fact about trust that has ALREADY
//! ended, so no later write to the profile restores that trust — a
//! `PutSigningProfile` on a name that is `Revoked` re-versions the profile and
//! replaces what the request carries, and the profile still reads back `Revoked`,
//! with the same `statusReason` and the same one revocation record
//! (`revocationEffectiveFrom`, `revokedAt`, `revokedBy`) the revoking call wrote.
//! `revocationEffectiveFrom` is the instant after which signatures stop being
//! trusted (`smithy_query(query="doc", shape="RevokeSigningProfileRequest",
//! member="effectiveTime")`), so `GetRevocationStatus` still reports every
//! version the profile has had as revoked for a signature made at or after it —
//! including the version the write itself minted — and still reports none of them
//! for a signature made before it.
//!
//! A profile nobody revoked has nothing to carry: a re-put of it leaves it
//! `Active`, with no `statusReason`, no revocation record and nothing revoked.
//!
//! The rows differ along what the rule generalises over: how many writes arrive
//! after the revocation, which platform each of them names (the same one, or
//! another published one, which is what proves the write took effect), which
//! version of the profile the revocation status is asked for (the one the revoke
//! named, an older one, and one minted after it), where the signature's timestamp
//! falls relative to the effective instant (one second before, exactly on it, one
//! second after), the reason recorded (including `RevocationReasonString`'s
//! `@length` minimum of 1 and maximum of 500), the effective instant, and the
//! region the requests are signed for.

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
    let job_arn =
        format!("arn:aws:signer:{region}:{ACCOUNT_ID}:/signing-jobs/no-such-job-of-this-endpoint");
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

/// One revoked profile, and the writes that arrive after its revocation.
struct Case {
    name: &'static str,
    region: &'static str,
    /// The platform the profile is created on.
    platform: &'static str,
    /// The platform each write that lands AFTER the revocation names — one entry
    /// per write, so a row also varies how many of them there are. Every id is a
    /// published one (`SIGNING_PLATFORM_CATALOG`), so the write is accepted and
    /// the stored `platformId` is what shows it took effect.
    reputs: &'static [&'static str],
    /// The reason the ONE revocation records.
    reason: String,
    /// The instant the revocation takes effect, as epoch seconds.
    effective_epoch_secs: i64,
}

fn cases() -> Vec<Case> {
    vec![
        // One write, naming the SAME platform: the smallest re-put there is.
        Case {
            name: "Revoked_Then_Reput_Same_Platform",
            region: "us-west-2",
            platform: "AWSLambda-SHA384-ECDSA",
            reputs: &["AWSLambda-SHA384-ECDSA"],
            reason: "key compromised".to_string(),
            effective_epoch_secs: 1_700_000_000,
        },
        // One write naming ANOTHER published platform, and the shortest reason
        // `RevocationReasonString` admits (`@length(min: 1)`).
        Case {
            name: "Revoked_Then_Reput_Other_Platform",
            region: "eu-west-1",
            platform: "Notation-OCI-SHA384-ECDSA",
            reputs: &["AWSIoTDeviceManagement-SHA256-ECDSA"],
            reason: "x".to_string(),
            effective_epoch_secs: 1_000_000_000,
        },
        // Three writes, across platforms, and the longest reason that shape
        // admits (`@length(max: 500)`): the record is kept, not kept once.
        Case {
            name: "Revoked_Then_Reput_Three_Times",
            region: "us-east-1",
            // The revoke is seeded on this platform, so it is one that publishes
            // the revocation capability; the writes that follow it name platforms
            // that do not, which the revocation already recorded outlives.
            platform: "AWSLambda-SHA384-ECDSA",
            reputs: &[
                "AmazonFreeRTOS-Default",
                "AmazonFreeRTOS-TI-CC3220SF",
                "AWSIoTDeviceManagement-SHA256-ECDSA",
            ],
            reason: "y".repeat(500),
            effective_epoch_secs: 1_600_000_000,
        },
        // A far-future effective instant, in a fourth region.
        Case {
            name: "Revoked_Then_Reput_Future_Effective",
            region: "ap-southeast-1",
            platform: "AWSLambda-SHA384-ECDSA",
            reputs: &["Notation-OCI-SHA384-ECDSA"],
            reason: "certificate material leaked".to_string(),
            effective_epoch_secs: 4_000_000_000,
        },
    ]
}

/// A `PutSigningProfile` on a revoked profile re-versions it without restoring
/// trust: the profile stays `Revoked` with the one record the revoke wrote, and
/// every version it has had — the revoked one, the older ones and the ones the
/// writes minted — is still reported as revoked for a signature made at or after
/// the effective instant, and still not reported for one made before it.
#[tokio::test]
async fn a_reput_of_a_revoked_profile_keeps_its_revocation() {
    for case in cases() {
        let router = emulator();

        // Two versions before the revoke, so the profile already has an older
        // one the revocation covers.
        let first = put_profile(&router, case.region, case.name, case.platform).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: seeding the profile: {} {:?}",
            case.name, first.status, first.body
        );
        let profile_arn = first.str_member("arn");
        let mut versions = vec![first.str_member("profileVersion")];
        let second = put_profile(&router, case.region, case.name, case.platform).await;
        assert_eq!(
            second.status, 200,
            "case {:?}: re-versioning the profile: {} {:?}",
            case.name, second.status, second.body
        );
        versions.push(second.str_member("profileVersion"));

        let revoked = revoke_profile(
            &router,
            case.region,
            case.name,
            versions.last().expect("the profile has a version"),
            &case.reason,
            case.effective_epoch_secs,
        )
        .await;
        assert_eq!(
            revoked.status, 204,
            "case {:?}: expected RevokeSigningProfile to answer 204, got {} {:?}",
            case.name, revoked.status, revoked.body
        );

        // The ONE revocation, as the profile reports it before any later write.
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

        for (ordinal, platform) in case.reputs.iter().enumerate() {
            let before_version = get_profile(&router, case.region, case.name)
                .await
                .str_member("profileVersion");
            let reput = put_profile(&router, case.region, case.name, platform).await;
            assert_eq!(
                reput.status, 200,
                "case {:?} write {ordinal}: expected the put to answer 200, got {} {:?}",
                case.name, reput.status, reput.body
            );
            let minted = reput.str_member("profileVersion");
            assert_ne!(
                minted, before_version,
                "case {:?} write {ordinal}: expected the put to mint a new version, got {minted:?}",
                case.name
            );
            versions.push(minted.clone());

            let got = get_profile(&router, case.region, case.name).await;
            // The write took effect: the version it minted and the platform it
            // named are what the profile now reports.
            assert_eq!(
                got.str_member("profileVersion"),
                minted,
                "case {:?} write {ordinal}: expected the stored version to be the minted one, \
                 got {:?}",
                case.name,
                got.body["profileVersion"]
            );
            assert_eq!(
                got.str_member("platformId"),
                *platform,
                "case {:?} write {ordinal}: expected the written platform {platform}, got {:?}",
                case.name,
                got.body["platformId"]
            );
            // And it restored no trust: the status, the reason and the one
            // record stand, on the same profile identity.
            assert_eq!(
                got.str_member("status"),
                "Revoked",
                "case {:?} write {ordinal}: expected the profile to stay Revoked, got {:?}",
                case.name,
                got.body["status"]
            );
            assert_eq!(
                got.str_member("statusReason"),
                case.reason,
                "case {:?} write {ordinal}: expected the revoke's reason to stand, got {:?}",
                case.name,
                got.body["statusReason"]
            );
            assert_eq!(
                got.body["revocationRecord"], record,
                "case {:?} write {ordinal}: expected the one revocation record to stand, got {:?} \
                 instead of {record:?}",
                case.name, got.body["revocationRecord"]
            );
            assert_eq!(
                got.str_member("arn"),
                profile_arn,
                "case {:?} write {ordinal}: expected the profile ARN to be unchanged, got {:?}",
                case.name,
                got.body["arn"]
            );
            assert_eq!(
                got.str_member("profileToken"),
                profile_token,
                "case {:?} write {ordinal}: expected the profile's id to be unchanged, got {:?}",
                case.name,
                got.body["profileToken"]
            );

            // Every version the profile has had, including the one this write
            // minted, is still revoked from the effective instant on — and still
            // trusted before it.
            for version in &versions {
                let version_arn = format!("{profile_arn}/{version}");
                for (label, signature_epoch, revoked_then) in [
                    ("one second before the effective instant", -1, false),
                    ("exactly on the effective instant", 0, true),
                    ("one second after the effective instant", 1, true),
                ] {
                    let answer = revocation_status_of(
                        &router,
                        case.region,
                        platform,
                        &version_arn,
                        case.effective_epoch_secs + signature_epoch,
                    )
                    .await;
                    assert_eq!(
                        answer.status, 200,
                        "case {:?} write {ordinal} version {version} {label}: expected \
                         GetRevocationStatus 200, got {} {:?}",
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
                        "case {:?} write {ordinal} version {version} {label}: expected \
                         revokedEntities {expected:?}, got {:?}",
                        case.name,
                        answer.body["revokedEntities"]
                    );
                }
            }
        }
    }
}

/// The other side of the rule: a profile nobody revoked has no revocation to
/// carry, so a re-put of it reports `Active`, no `statusReason`, no revocation
/// record, and nothing revoked for any of its versions.
#[tokio::test]
async fn a_reput_of_a_never_revoked_profile_keeps_it_trusted() {
    // (case, region, the platform of the create, the platform each later write
    // names) — rows differ in how many writes arrive and whether they change the
    // platform.
    let rows: [(&str, &str, &str, &[&str]); 3] = [
        (
            "one write, same platform",
            "us-west-2",
            "AWSLambda-SHA384-ECDSA",
            &["AWSLambda-SHA384-ECDSA"],
        ),
        (
            "one write, another platform",
            "eu-west-1",
            "Notation-OCI-SHA384-ECDSA",
            &["AmazonFreeRTOS-Default"],
        ),
        (
            "two writes",
            "us-east-1",
            "AWSIoTDeviceManagement-SHA256-ECDSA",
            &["AmazonFreeRTOS-TI-CC3220SF", "AWSLambda-SHA384-ECDSA"],
        ),
    ];

    for (case, region, platform, reputs) in rows {
        let router = emulator();
        let name = "Never_Revoked_Then_Reput";
        let created = put_profile(&router, region, name, platform).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: seeding the profile: {} {:?}",
            created.status, created.body
        );
        let profile_arn = created.str_member("arn");
        let mut versions = vec![created.str_member("profileVersion")];

        for (ordinal, written) in reputs.iter().enumerate() {
            let reput = put_profile(&router, region, name, written).await;
            assert_eq!(
                reput.status, 200,
                "case {case:?} write {ordinal}: expected the put to answer 200, got {} {:?}",
                reput.status, reput.body
            );
            versions.push(reput.str_member("profileVersion"));

            let got = get_profile(&router, region, name).await;
            assert_eq!(
                got.str_member("status"),
                "Active",
                "case {case:?} write {ordinal}: expected an unrevoked profile to stay Active, \
                 got {:?}",
                got.body["status"]
            );
            assert!(
                got.body["statusReason"].is_null(),
                "case {case:?} write {ordinal}: expected no statusReason on an Active profile, \
                 got {:?}",
                got.body["statusReason"]
            );
            assert!(
                got.body["revocationRecord"].is_null(),
                "case {case:?} write {ordinal}: expected no revocation record on a profile \
                 nobody revoked, got {:?}",
                got.body["revocationRecord"]
            );

            for version in &versions {
                let version_arn = format!("{profile_arn}/{version}");
                let answer =
                    revocation_status_of(&router, region, written, &version_arn, 1_700_000_000)
                        .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?} write {ordinal} version {version}: expected \
                     GetRevocationStatus 200, got {} {:?}",
                    answer.status, answer.body
                );
                assert!(
                    answer.revoked_entities().is_empty(),
                    "case {case:?} write {ordinal} version {version}: expected nothing revoked, \
                     got {:?}",
                    answer.body["revokedEntities"]
                );
            }
        }
    }
}
