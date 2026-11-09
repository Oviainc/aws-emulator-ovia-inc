// SPDX-License-Identifier: CC0-1.0

//! A signing-profile NAME is taken by the create that first used it, end to end
//! over HTTP.
//!
//! Every case goes through the router, so it exercises routing, the credential
//! scope the region is read from, body deserialization, the handler, the stored
//! record and serialization — the path a client takes. The rule under test:
//! `PutSigningProfile` creates a profile, and the name it creates it under stays
//! taken, so a second `PutSigningProfile` naming a profile this endpoint already
//! holds is refused `400 ValidationException` and writes nothing. In particular:
//!
//! * the refusal does not depend on what the second request CARRIES — the same
//!   body, another platform, an overriding `signatureValidityPeriod`, tags,
//!   signing parameters: the name is what is taken;
//! * the refusal does not depend on the stored profile's lifecycle status — a
//!   `Canceled` or `Revoked` profile still holds its name
//!   (`SigningProfileStatus` is `Active | Canceled | Revoked`);
//! * nothing is written: the stored profile's `profileVersion`,
//!   `profileVersionArn`, `arn`, `profileToken`, `platformId`, status and tags
//!   read back exactly as the create that took the name left them, and the
//!   listing still reports ONE profile of that name;
//! * the name is taken per REGION, as Signer is a regional service: the same name
//!   is free at another region's endpoint, and the profile it creates there is a
//!   different profile with its own version.
//!
//! The rows differ along what the rule generalises over: the platform the profile
//! is created on, the status it is in when the second create arrives, what that
//! second create carries, and the region the requests are signed for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// An ACM certificate ARN for `signingMaterial`, so a profile can be created on
/// every published platform, including the ones that require it.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The HTTP status a create under a taken name answers.
const TAKEN_NAME_STATUS: u16 = 400;

/// The error shape it is answered in, and the `code` it carries: a refusal of the
/// request's own values carries the modelled shape's own name.
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

    /// The modelled `code` the error carries.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The error's prose.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
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
    request: Value,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(request),
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

/// The smallest create body there is, for `platform`.
fn create_request(platform: &str) -> Value {
    json!({
        "platformId": platform,
        "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
    })
}

/// How the stored profile got to the status it is in when the second create
/// arrives.
#[derive(Debug, Clone, Copy)]
enum Stored {
    /// Created and left alone.
    Active,
    /// Created, then canceled — a status whose name AWS does not release.
    Canceled,
    /// Created, then revoked.
    Revoked,
}

/// One name the second create is sent to.
struct Case {
    name: &'static str,
    region: &'static str,
    /// The platform the first create names. Every id is a published one
    /// (`SIGNING_PLATFORM_CATALOG`), so nothing but the taken name can refuse the
    /// second create.
    platform: &'static str,
    stored: Stored,
    /// The status the stored profile reports throughout.
    status: &'static str,
    /// What the second create carries, and a label for it.
    second: (&'static str, Value),
}

fn cases() -> Vec<Case> {
    vec![
        // The recorded shape of the defect: the identical create, repeated.
        Case {
            name: "Taken_Identical_Create",
            region: "us-west-2",
            platform: "AWSLambda-SHA384-ECDSA",
            stored: Stored::Active,
            status: "Active",
            second: (
                "the identical body",
                create_request("AWSLambda-SHA384-ECDSA"),
            ),
        },
        // A second create naming ANOTHER published platform: still the name.
        Case {
            name: "Taken_Other_Platform",
            region: "eu-west-1",
            platform: "Notation-OCI-SHA384-ECDSA",
            stored: Stored::Active,
            status: "Active",
            second: (
                "another platform",
                create_request("AWSIoTDeviceManagement-SHA256-ECDSA"),
            ),
        },
        // A second create overriding the validity period, which
        // `PutSigningProfileRequest$signatureValidityPeriod` documents as
        // overridable ("The default validity period override for any signature
        // generated using this signing profile").
        Case {
            name: "Taken_Validity_Override",
            region: "us-east-1",
            platform: "AWSLambda-SHA384-ECDSA",
            stored: Stored::Active,
            status: "Active",
            second: (
                "an overriding signatureValidityPeriod",
                json!({
                    "platformId": "AWSLambda-SHA384-ECDSA",
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                    "signatureValidityPeriod": { "value": 10, "type": "MONTHS" },
                }),
            ),
        },
        // A second create carrying tags and signing parameters — members a client
        // might expect an update to apply.
        Case {
            name: "Taken_With_Tags",
            region: "ap-southeast-1",
            platform: "AWSLambda-SHA384-ECDSA",
            stored: Stored::Active,
            status: "Active",
            second: (
                "tags and signing parameters",
                json!({
                    "platformId": "AWSLambda-SHA384-ECDSA",
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                    "tags": { "team": "platform" },
                    "signingParameters": { "env": "prod" },
                }),
            ),
        },
        // A CANCELED profile still holds its name: the status AWS schedules for
        // deletion is the one a client is most likely to try to re-create under.
        Case {
            name: "Taken_After_Cancel",
            region: "us-west-2",
            platform: "AWSLambda-SHA384-ECDSA",
            stored: Stored::Canceled,
            status: "Canceled",
            second: (
                "the identical body",
                create_request("AWSLambda-SHA384-ECDSA"),
            ),
        },
        // So does a REVOKED one, on a platform that publishes the revocation
        // capability (see `profile_revocation_capability.rs`).
        Case {
            name: "Taken_After_Revoke",
            region: "eu-west-1",
            platform: "AWSLambda-SHA384-ECDSA",
            stored: Stored::Revoked,
            status: "Revoked",
            second: (
                "another platform",
                create_request("Notation-OCI-SHA384-ECDSA"),
            ),
        },
        // A create that leaves `signingMaterial` out, on a platform Signer issues
        // the certificate for itself: a request the name refuses before anything
        // about the platform is decided.
        Case {
            name: "Taken_Without_Material",
            region: "us-east-1",
            platform: "Notation-OCI-SHA384-ECDSA",
            stored: Stored::Active,
            status: "Active",
            second: (
                "no signingMaterial",
                json!({ "platformId": "Notation-OCI-SHA384-ECDSA" }),
            ),
        },
    ]
}

/// Bring `case`'s profile to the status the row states, answering what the create
/// that took the name reported.
async fn seed(router: &MultiServiceRouter, case: &Case) -> Answer {
    let created = put_profile(
        router,
        case.region,
        case.name,
        create_request(case.platform),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: seeding the profile: {} {:?}",
        case.name, created.status, created.body
    );
    match case.stored {
        Stored::Active => {}
        Stored::Canceled => {
            let canceled = call(
                router,
                case.region,
                "DELETE",
                &format!("/signing-profiles/{}", case.name),
                None,
            )
            .await;
            assert_eq!(
                canceled.status, 204,
                "case {:?}: the seeding cancel must succeed, got {} {:?}",
                case.name, canceled.status, canceled.body
            );
        }
        Stored::Revoked => {
            let revoked = call(
                router,
                case.region,
                "PUT",
                &format!("/signing-profiles/{}/revoke", case.name),
                Some(json!({
                    "profileVersion": created.str_member("profileVersion"),
                    "reason": "key compromised",
                    "effectiveTime": 1_700_000_000,
                })),
            )
            .await;
            assert_eq!(
                revoked.status, 204,
                "case {:?}: the seeding revoke must succeed, got {} {:?}",
                case.name, revoked.status, revoked.body
            );
        }
    }
    created
}

/// A second `PutSigningProfile` under a name this endpoint already holds is the
/// modelled 400, whatever it carries and whatever status the stored profile is
/// in — and it writes nothing.
#[tokio::test]
async fn a_second_create_under_a_taken_name_is_refused_and_writes_nothing() {
    for case in cases() {
        let router = emulator();
        let created = seed(&router, &case).await;
        let before = get_profile(&router, case.region, case.name).await;
        let (label, request) = case.second.clone();

        let answer = put_profile(&router, case.region, case.name, request).await;
        assert_eq!(
            answer.status, TAKEN_NAME_STATUS,
            "case {:?} carrying {label:?}: expected HTTP {TAKEN_NAME_STATUS}, got {} {:?}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            TAKEN_NAME_SHAPE,
            "case {:?} carrying {label:?}: expected the error shape {TAKEN_NAME_SHAPE}, got {:?}",
            case.name,
            answer.body
        );
        assert_eq!(
            answer.code(),
            TAKEN_NAME_SHAPE,
            "case {:?} carrying {label:?}: expected the code {TAKEN_NAME_SHAPE}, got {:?}",
            case.name,
            answer.body
        );
        assert!(
            answer.message().contains(case.name),
            "case {:?} carrying {label:?}: expected the refusal to name the profile, got {:?}",
            case.name,
            answer.body["message"]
        );
        assert!(
            answer.body["profileVersion"].is_null() && answer.body["arn"].is_null(),
            "case {:?} carrying {label:?}: expected the refusal to report no profile, got {:?}",
            case.name,
            answer.body
        );

        // Nothing moved: every member a read of the profile reports is the one
        // the create that took the name stored.
        let after = get_profile(&router, case.region, case.name).await;
        assert_eq!(
            after.body, before.body,
            "case {:?} carrying {label:?}: expected the stored profile to be untouched, got {:?} \
             instead of {:?}",
            case.name, after.body, before.body
        );
        for member in ["profileVersion", "profileVersionArn", "arn"] {
            assert_eq!(
                after.str_member(member),
                created.str_member(member),
                "case {:?} carrying {label:?}: expected {member} to be the created one, got {:?}",
                case.name,
                after.body[member]
            );
        }
        assert_eq!(
            after.str_member("status"),
            case.status,
            "case {:?} carrying {label:?}: expected the status {}, got {:?}",
            case.name,
            case.status,
            after.body["status"]
        );
        assert_eq!(
            after.str_member("platformId"),
            case.platform,
            "case {:?} carrying {label:?}: expected the created platform {}, got {:?}",
            case.name,
            case.platform,
            after.body["platformId"]
        );

        // And the region still holds exactly ONE profile of that name. The
        // listing is asked for the status the row stores, because
        // `ListSigningProfiles` "[r]eturns only profiles with an ACTIVE status
        // unless the includeCanceled request field is set to true" and takes a
        // `statuses` filter for anything else.
        let listed = call(
            &router,
            case.region,
            "GET",
            &format!("/signing-profiles?statuses={}", case.status),
            None,
        )
        .await;
        let held: Vec<&Value> = listed.body["profiles"]
            .as_array()
            .map(|profiles| {
                profiles
                    .iter()
                    .filter(|profile| profile["profileName"] == case.name)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            held.len(),
            1,
            "case {:?} carrying {label:?}: expected one listed profile of that name, got {:?}",
            case.name,
            listed.body["profiles"]
        );
    }
}

/// The name is taken per REGION: a name one region's endpoint has taken is still
/// free at another's, and the profile created there is its own profile.
#[tokio::test]
async fn a_taken_name_is_still_free_in_another_region() {
    const NAME: &str = "Taken_In_One_Region";
    // (case, the region that takes the name, the region asked next, whether that
    // region may create it) — including the same region twice, which may not.
    let rows: [(&str, &str, &str, bool); 4] = [
        ("another region entirely", "us-west-2", "eu-west-1", true),
        ("a third region", "us-west-2", "ap-southeast-1", true),
        ("the same region again", "us-west-2", "us-west-2", false),
        ("the same region, named twice", "eu-west-1", "eu-west-1", false),
    ];

    for (case, taken_in, asked, free) in rows {
        let router = emulator();
        let first = put_profile(
            &router,
            taken_in,
            NAME,
            create_request("AWSLambda-SHA384-ECDSA"),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: seeding in {taken_in}: {} {:?}",
            first.status, first.body
        );

        let second = put_profile(
            &router,
            asked,
            NAME,
            create_request("AWSLambda-SHA384-ECDSA"),
        )
        .await;
        if free {
            assert_eq!(
                second.status, 200,
                "case {case:?}: expected {asked} to create the name, got {} {:?}",
                second.status, second.body
            );
            assert_ne!(
                second.str_member("arn"),
                first.str_member("arn"),
                "case {case:?}: expected {asked}'s profile to carry its own ARN, got {:?}",
                second.body["arn"]
            );
            assert_ne!(
                second.str_member("profileVersion"),
                first.str_member("profileVersion"),
                "case {case:?}: expected {asked}'s profile to carry its own version, got {:?}",
                second.body["profileVersion"]
            );
        } else {
            assert_eq!(
                second.status, TAKEN_NAME_STATUS,
                "case {case:?}: expected {asked} to refuse the taken name, got {} {:?}",
                second.status, second.body
            );
        }

        // Either way the region that took the name reports the profile it
        // created, unchanged.
        let held = get_profile(&router, taken_in, NAME).await;
        assert_eq!(
            held.str_member("profileVersion"),
            first.str_member("profileVersion"),
            "case {case:?}: expected {taken_in} to hold its own version, got {:?}",
            held.body["profileVersion"]
        );
    }
}
