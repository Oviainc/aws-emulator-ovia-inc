// SPDX-License-Identifier: CC0-1.0

//! What `CancelSigningProfile` answers for every status a stored profile can be
//! in, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! credential scope the region is read from, the handler, the persisted record
//! and serialization — the same path a client takes. The rules under test:
//!
//! * a cancel applies the documented transition where its source status holds —
//!   an `Active` profile becomes `Canceled` — and answers success, with the
//!   status left where it stands, for a profile that is already `Canceled`;
//! * a profile whose signatures have been REVOKED cannot be canceled at all: the
//!   call is refused with the recorded 400 whose `code` names the status that
//!   forbade it, and the profile stays `Revoked`;
//! * every other answer carries a status the operation refuses with (see
//!   [`MODELLED_ERROR_STATUSES`]): a profile this region cannot resolve is the
//!   modelled 404 and nothing else.
//!
//! The rows differ along what the rules generalise over: the status the profile
//! is in when the cancel arrives, how it got there (including a re-put, which
//! makes a canceled profile `Active` again and leaves a revoked one `Revoked`),
//! the platform, and the region the request is signed for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::ERROR_CODE_PROFILE_NOT_FOUND;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The HTTP statuses `CancelSigningProfile` may refuse with: the `@httpError` of
/// each shape in the operation's modelled `errors` list
/// (`smithy_query(query="operation", operation="CancelSigningProfile")` —
/// `AccessDeniedException` 403, `ResourceNotFoundException` 404,
/// `TooManyRequestsException` 429, `InternalServiceErrorException` 500), plus
/// the 400 the RECORDED AWS answer refuses a revoked profile with.
const MODELLED_ERROR_STATUSES: [u16; 5] = [400, 403, 404, 429, 500];

/// The status a cancel of a REVOKED profile answers, and the `code` and error
/// shape it carries.
const REVOKED_REFUSAL_STATUS: u16 = 400;
/// The `code` that refusal carries: `Profile` followed by the status that
/// forbade the call.
const ERROR_CODE_PROFILE_REVOKED: &str = "ProfileRevoked";
/// The error shape it is answered in — a 400 shape this SERVICE models.
const REVOKED_REFUSAL_SHAPE: &str = "ValidationException";

/// The status a successful cancel answers: the recorded answer to this `DELETE`.
const SUCCESS_STATUS: u16 = 204;

/// An ACM certificate ARN for `signingMaterial`.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status, JSON body, the raw bytes (so an empty
/// body can be told from a `null` one) and the `x-amzn-errortype` header
/// restJson1 resolves an error SHAPE from.
struct Answer {
    status: u16,
    body: Value,
    raw_body: Bytes,
    error_type: Option<String>,
}

impl Answer {
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or `""` when the body carries none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The modelled `message` member, or `""` when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// A SigV4 `Authorization` header whose credential scope names `region` and the
/// `signer` signing name: what routes the request to this service and what the
/// region of the endpoint is read from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
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
        .header("authorization", authorization(region));
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
    let raw_body = response.into_body();
    let body = if raw_body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw_body).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} in {region} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&raw_body)
            )
        })
    };
    Answer {
        status,
        body,
        raw_body,
        error_type,
    }
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

async fn cancel_profile(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!("/signing-profiles/{name}"),
        None,
    )
    .await
}

/// Create (or re-version) a profile on `platform`, answering its current version.
async fn put_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    platform: &str,
) -> String {
    let put = call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "seeding {name} in {region}: expected 200, got {} {:?}",
        put.status, put.body
    );
    put.str_member("profileVersion")
}

async fn revoke_profile(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    version: &str,
) -> Answer {
    call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": version,
            "reason": "key-compromise",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await
}

/// How a profile reached the status it is in when the cancel arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Created and left alone.
    Put,
    /// Created, then canceled once.
    Canceled,
    /// Created, then revoked.
    Revoked,
    /// Created, canceled, then re-put — which re-versions it to `Active`.
    CanceledThenReput,
    /// Created, revoked, then re-put — which re-versions it but leaves the
    /// revocation standing, so it is still `Revoked`.
    RevokedThenReput,
}

/// One profile a cancel is sent to.
#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    region: &'static str,
    /// A signing platform this service publishes, or one outside its catalog —
    /// either way a profile naming `signingMaterial` is accepted on it.
    platform: &'static str,
    setup: Setup,
    /// The status the profile reports before the cancel.
    status_before: &'static str,
    /// The status it reports after it.
    status_after: &'static str,
}

impl Case {
    /// Whether the status this profile is in forbids the cancel: a revoked
    /// profile cannot be canceled, and no other status refuses the call.
    fn refused(&self) -> bool {
        self.status_before == "Revoked"
    }
}

/// Every status a stored profile can be in when a cancel arrives
/// (`SigningProfileStatus` is `Active | Canceled | Revoked`), reached two ways
/// where a re-put can reach it, across four platforms and four regions.
const CASES: &[Case] = &[
    Case {
        name: "Active_Lambda",
        region: "us-west-2",
        platform: "AWSLambda-SHA384-ECDSA",
        setup: Setup::Put,
        status_before: "Active",
        status_after: "Canceled",
    },
    Case {
        name: "Active_Container",
        region: "eu-west-1",
        platform: "Notation-OCI-SHA384-ECDSA",
        setup: Setup::Put,
        status_before: "Active",
        status_after: "Canceled",
    },
    Case {
        name: "Already_Canceled",
        region: "us-east-1",
        platform: "AmazonFreeRTOS-Default",
        setup: Setup::Canceled,
        status_before: "Canceled",
        status_after: "Canceled",
    },
    Case {
        name: "Already_Revoked",
        region: "ap-southeast-1",
        // Seeded through a revoke, so the platform has to publish the revocation
        // capability (see `profile_revocation_capability.rs`).
        platform: "AWSLambda-SHA384-ECDSA",
        setup: Setup::Revoked,
        // A revocation is already-final: the cancel is refused for it rather
        // than moving the profile to the weaker status.
        status_before: "Revoked",
        status_after: "Revoked",
    },
    Case {
        name: "Canceled_Then_Reput",
        region: "us-west-2",
        platform: "AWSLambda-SHA384-ECDSA",
        setup: Setup::CanceledThenReput,
        status_before: "Active",
        status_after: "Canceled",
    },
    Case {
        name: "Revoked_Then_Reput",
        region: "eu-west-1",
        // Seeded through a revoke, so the platform has to publish the revocation
        // capability (see `profile_revocation_capability.rs`).
        platform: "Notation-OCI-SHA384-ECDSA",
        setup: Setup::RevokedThenReput,
        // A re-put re-versions the profile without restoring trust in what the
        // revocation untrusted, so the revoked status stands — and with it the
        // refusal of the cancel.
        status_before: "Revoked",
        status_after: "Revoked",
    },
];

/// Bring `case`'s profile to the status it must be in before the cancel.
async fn seed(router: &MultiServiceRouter, case: &Case) {
    let version = put_profile(router, case.region, case.name, case.platform).await;
    match case.setup {
        Setup::Put => {}
        Setup::Canceled | Setup::CanceledThenReput => {
            let first = cancel_profile(router, case.region, case.name).await;
            assert_eq!(
                first.status, SUCCESS_STATUS,
                "case {:?}: the seeding cancel must succeed, got {} {:?}",
                case.name, first.status, first.body
            );
        }
        Setup::Revoked | Setup::RevokedThenReput => {
            let revoked = revoke_profile(router, case.region, case.name, &version).await;
            assert_eq!(
                revoked.status, 204,
                "case {:?}: the seeding revoke must succeed, got {} {:?}",
                case.name, revoked.status, revoked.body
            );
        }
    }
    if matches!(
        case.setup,
        Setup::CanceledThenReput | Setup::RevokedThenReput
    ) {
        put_profile(router, case.region, case.name, case.platform).await;
    }

    let got = get_profile(router, case.region, case.name).await;
    assert_eq!(
        got.str_member("status"),
        case.status_before,
        "case {:?}: expected the seeded status {}, got {:?}",
        case.name,
        case.status_before,
        got.body
    );
}

/// A cancel of a stored profile either applies the documented transition or is
/// refused for the ONE status that forbids it: a revoked profile answers the
/// recorded 400 and keeps its status, every other status answers the success
/// status, and the status the profile ends in is the one the row states.
#[tokio::test]
async fn a_cancel_of_a_stored_profile_follows_the_status_it_is_in() {
    for case in CASES {
        let router = emulator();
        seed(&router, case).await;

        let answer = cancel_profile(&router, case.region, case.name).await;
        if case.refused() {
            assert_eq!(
                answer.status, REVOKED_REFUSAL_STATUS,
                "case {:?} ({}): expected HTTP {REVOKED_REFUSAL_STATUS}, got {} {:?}",
                case.name, case.status_before, answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                ERROR_CODE_PROFILE_REVOKED,
                "case {:?}: expected the code to name the status that forbade the call, got {:?}",
                case.name,
                answer.body
            );
            assert_eq!(
                answer.shape(),
                REVOKED_REFUSAL_SHAPE,
                "case {:?}: expected the error shape {REVOKED_REFUSAL_SHAPE}, got {:?}",
                case.name,
                answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some(REVOKED_REFUSAL_SHAPE),
                "case {:?}: expected x-amzn-errortype {REVOKED_REFUSAL_SHAPE}, got {:?}",
                case.name,
                answer.error_type
            );
            assert!(
                answer.message().contains(case.name),
                "case {:?}: expected the refusal to name the profile, got {:?}",
                case.name,
                answer.message()
            );
        } else {
            assert_eq!(
                answer.status, SUCCESS_STATUS,
                "case {:?} ({}): expected HTTP {SUCCESS_STATUS}, got {} {:?}",
                case.name, case.status_before, answer.status, answer.body
            );
            assert!(
                answer.raw_body.is_empty(),
                "case {:?}: expected an empty body on {SUCCESS_STATUS}, got {:?}",
                case.name,
                String::from_utf8_lossy(&answer.raw_body)
            );
            assert!(
                answer.error_type.is_none(),
                "case {:?}: expected no x-amzn-errortype on a success, got {:?}",
                case.name,
                answer.error_type
            );
        }

        let got = get_profile(&router, case.region, case.name).await;
        assert_eq!(
            got.status, 200,
            "case {:?}: a canceled profile stays viewable, got {} {:?}",
            case.name, got.status, got.body
        );
        assert_eq!(
            got.str_member("status"),
            case.status_after,
            "case {:?} ({}): expected status {} after the cancel, got {:?}",
            case.name,
            case.status_before,
            case.status_after,
            got.body["status"]
        );

        // The call is repeatable: a second cancel answers exactly what the first
        // did and moves nothing further.
        let again = cancel_profile(&router, case.region, case.name).await;
        assert_eq!(
            again.status, answer.status,
            "case {:?}: expected a repeated cancel to answer HTTP {}, got {} {:?}",
            case.name, answer.status, again.status, again.body
        );
        let after_again = get_profile(&router, case.region, case.name).await;
        assert_eq!(
            after_again.str_member("status"),
            case.status_after,
            "case {:?}: expected status {} to be unchanged by a repeated cancel, got {:?}",
            case.name,
            case.status_after,
            after_again.body["status"]
        );
        assert_eq!(
            after_again.body["profileVersion"], got.body["profileVersion"],
            "case {:?}: expected the version to survive the cancels, got {:?}",
            case.name, after_again.body["profileVersion"]
        );
    }
}

/// Every answer this operation gives carries a status it refuses with and a
/// shape the service models: a profile it cannot resolve is
/// `ResourceNotFoundException`/404, and a revoked one the 400 above — nothing
/// else.
#[tokio::test]
async fn every_cancel_answer_stays_inside_the_refusal_set() {
    let router = emulator();

    // Profiles in every status, all in one region, plus the same names in
    // another region, which holds none of them.
    let holding_region = "us-west-2";
    let empty_region = "eu-central-1";
    for case in CASES {
        let seeded = Case {
            region: holding_region,
            ..*case
        };
        seed(&router, &seeded).await;
    }

    // Names no region holds: the shortest and the longest a `profileName` may be
    // (`ProfileName` is `@length(min: 2, max: 64)`, pattern `^[a-zA-Z0-9_]{2,}$`),
    // and a plain unknown name.
    let unknown: [String; 3] = ["Ab".to_string(), "N".repeat(64), "No_Such_Profile".to_string()];

    /// What a row expects of the answer.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Expected {
        /// The profile is stored here and its status admits the cancel.
        Success,
        /// The profile is stored here and is revoked.
        Revoked,
        /// This region holds no profile of that name.
        Unresolvable,
    }

    let mut rows: Vec<(String, &str, Expected)> = Vec::new();
    for case in CASES {
        // Stored here: resolvable, so a success unless its status forbids it.
        rows.push((
            case.name.to_string(),
            holding_region,
            if case.refused() {
                Expected::Revoked
            } else {
                Expected::Success
            },
        ));
        // The same name through a region that holds no such profile.
        rows.push((case.name.to_string(), empty_region, Expected::Unresolvable));
    }
    for name in &unknown {
        rows.push((name.clone(), holding_region, Expected::Unresolvable));
    }

    for (name, region, expected) in rows {
        let answer = cancel_profile(&router, region, &name).await;
        let case = format!("{name} in {region}");
        if expected == Expected::Success {
            assert_eq!(
                answer.status, SUCCESS_STATUS,
                "case {case:?}: expected HTTP {SUCCESS_STATUS}, got {} {:?}",
                answer.status, answer.body
            );
            continue;
        }
        assert!(
            MODELLED_ERROR_STATUSES.contains(&answer.status),
            "case {case:?}: expected a status the operation refuses with \
             ({MODELLED_ERROR_STATUSES:?}), got {} {:?}",
            answer.status,
            answer.body
        );
        let (status, shape, code) = if expected == Expected::Revoked {
            (
                REVOKED_REFUSAL_STATUS,
                REVOKED_REFUSAL_SHAPE,
                ERROR_CODE_PROFILE_REVOKED,
            )
        } else {
            (
                404,
                "ResourceNotFoundException",
                ERROR_CODE_PROFILE_NOT_FOUND,
            )
        };
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            shape,
            "case {case:?}: expected the error shape {shape}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some(shape),
            "case {case:?}: expected x-amzn-errortype to name the shape, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.code(),
            code,
            "case {case:?}: expected the code naming the condition, got {:?}",
            answer.body
        );
    }

    // The region that holds nothing still holds nothing: the refused cancels
    // created no profile there.
    for case in CASES {
        let got = get_profile(&router, empty_region, case.name).await;
        assert_eq!(
            got.status, 404,
            "case {:?}: a refused cancel must store nothing, got {} {:?}",
            case.name, got.status, got.body
        );
    }
}
