// SPDX-License-Identifier: CC0-1.0

//! What `PutSigningProfile` does with the platform a profile is bound to, end to
//! end over HTTP.
//!
//! A signing profile is a template bound to one of the signing platforms the
//! service publishes, and that platform is what supplies the signing
//! configuration a later signing job runs under. Two rules follow, and this suite
//! asserts both over a table rather than over one request:
//!
//! * a `platformId` the published catalog holds no entry for names no platform, so
//!   the create is refused with the same modelled 404 a read of that platform is
//!   refused with, and stores nothing;
//! * an `overrides.signingConfiguration` member SELECTS among the algorithms the
//!   platform publishes as allowed, so a value inside its `allowedValues` is
//!   stored and a value outside them is refused 400, storing nothing.
//!
//! The rows are generated from the published catalog and from the modelled enums,
//! so every platform the emulator publishes is covered and no row is keyed to one
//! recorded request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::{
    ENCRYPTION_ALGORITHM_VALUES, HASH_ALGORITHM_VALUES,
};
use service_com_amazonaws_signer_wallabyservice::state::{
    SigningConfigurationMember, ERROR_CODE_PLATFORM_NOT_FOUND, ERROR_CODE_VALIDATION,
    SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// An ACM certificate ARN for `signingMaterial`, so a put below is never refused
/// for the certificate instead of for the platform or the override under test.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// `platformId`s this service publishes no platform for, with how each one misses.
///
/// Source: the published catalog is exactly the ids of
/// `crate::state::SIGNING_PLATFORM_CATALOG`, so every id below is outside it. The
/// rows differ in HOW they miss: a partner the catalog does not publish at all, a
/// near-miss on the algorithm suffix of a published id, a published id in the
/// wrong case, a published id's partner on its own, a published id with something
/// appended, and a truncation of a published id's target.
const UNPUBLISHED_PLATFORM_IDS: &[(&str, &str)] = &[
    ("PrivatePlatform-SHA256-ECDSA", "an unpublished partner"),
    (
        "Notation-OCI-SHA256-ECDSA",
        "a published partner with another algorithm suffix",
    ),
    ("awslambda-sha384-ecdsa", "a published id in the wrong case"),
    ("AWSLambda", "a published id's partner alone"),
    (
        "AWSLambda-SHA384-ECDSA-v2",
        "a published id with a suffix appended",
    ),
    (
        "AmazonFreeRTOS-TI-CC3220",
        "a truncation of a published id's target",
    ),
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

    /// The modelled `message` member, or the empty string when the body carries
    /// none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
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

async fn get_platform(router: &MultiServiceRouter, platform_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-platforms/{platform_id}"),
        None,
    )
    .await
}

/// A `platformId` outside the published catalog is refused with the modelled 404
/// the platform readers answer, stores no profile, and is reported as unpublished
/// by `GetSigningPlatform` too — whether or not the request names signing
/// material.
#[tokio::test]
async fn an_unpublished_platform_id_refuses_the_create_and_stores_nothing() {
    let router = emulator();

    for (ordinal, (platform, how)) in UNPUBLISHED_PLATFORM_IDS.iter().enumerate() {
        let case = format!("{how} ({platform:?})");

        // The service itself publishes no such platform: the refusal below is the
        // same condition, reported the same way.
        let published = get_platform(&router, platform).await;
        assert_eq!(
            published.status, 404,
            "case {case}: expected GetSigningPlatform to publish no such platform, got {} {:?}",
            published.status, published.body
        );

        for (variant, body) in [
            (
                "with signingMaterial",
                json!({
                    "platformId": platform,
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                }),
            ),
            ("without signingMaterial", json!({ "platformId": platform })),
        ] {
            let name = format!("Unpublished_{ordinal}_{}", variant.replace(' ', "_"));
            let put = put_profile(&router, &name, body).await;
            assert_eq!(
                put.status, 404,
                "case {case} {variant}: expected HTTP 404, got {} {:?}",
                put.status, put.body
            );
            assert_eq!(
                put.shape(),
                "ResourceNotFoundException",
                "case {case} {variant}: expected the modelled 404 shape, got {:?}",
                put.body
            );
            assert_eq!(
                put.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case} {variant}: expected x-amzn-errortype ResourceNotFoundException, got \
                 {:?}",
                put.error_type
            );
            assert_eq!(
                put.code(),
                Some(ERROR_CODE_PLATFORM_NOT_FOUND),
                "case {case} {variant}: expected the modelled code to name the condition, got {:?}",
                put.body
            );
            assert!(
                put.message().contains(*platform),
                "case {case} {variant}: expected the refusal to name the platform, got {:?}",
                put.body["message"]
            );

            let read_back = get_profile(&router, &name).await;
            assert_eq!(
                read_back.status, 404,
                "case {case} {variant}: a refused create must store no profile, got {} {:?}",
                read_back.status, read_back.body
            );
        }
    }
}

/// Every platform the catalog publishes accepts a create, so the refusal above is
/// about the platform being unpublished rather than about creates in general.
#[tokio::test]
async fn every_published_platform_accepts_a_create() {
    let router = emulator();

    for (ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let case = entry.platform_id;
        let name = format!("Published_Platform_{ordinal}");
        let put = put_profile(
            &router,
            &name,
            json!({
                "platformId": case,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected PutSigningProfile 200, got {} {:?}",
            put.status, put.body
        );

        let read_back = get_profile(&router, &name).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected the created profile to be readable, got {} {:?}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.body["platformId"],
            json!(case),
            "case {case:?}: expected the stored platformId, got {:?}",
            read_back.body["platformId"]
        );
    }
}

/// One signing-configuration override row: the body to put, and whether the
/// platform publishes the value it names as allowed.
struct OverrideCase {
    /// What the row is about, named in every assertion.
    label: String,
    /// The `overrides` member of the create request.
    overrides: Value,
    /// The member whose value falls outside the platform's `allowedValues`, with
    /// that value — `None` for a row the platform admits.
    outside: Option<(SigningConfigurationMember, String)>,
}

/// Every override row for one published platform.
///
/// Both halves are taken from the values the MODEL's enums admit — `HashAlgorithm`
/// is `SHA1 | SHA256` and `EncryptionAlgorithm` is `RSA | ECDSA`
/// (`crate::ops::types::HASH_ALGORITHM_VALUES` / `ENCRYPTION_ALGORITHM_VALUES`) —
/// because a value those enums do not name never reaches the handler: the
/// boundary validator refuses it for the modelled constraint first, which is what
/// happens to the SHA384 some platforms publish, a value this model's enum
/// predates. So the rows partition the EXPRESSIBLE values: the accepted ones are
/// the platform's `allowedValues` the model can carry (each on its own and both
/// together), the refused ones are every expressible value the platform does not
/// publish, plus one row naming an outside value for both members at once.
fn override_cases(
    platform_id: &str,
    hash_allowed: &[&'static str],
    encryption_allowed: &[&'static str],
) -> Vec<OverrideCase> {
    let mut cases = Vec::new();
    let hash_expressible: Vec<&&str> = HASH_ALGORITHM_VALUES
        .iter()
        .filter(|value| hash_allowed.contains(value))
        .collect();
    let encryption_expressible: Vec<&&str> = ENCRYPTION_ALGORITHM_VALUES
        .iter()
        .filter(|value| encryption_allowed.contains(value))
        .collect();

    for hash in &hash_expressible {
        cases.push(OverrideCase {
            label: format!("{platform_id}: allowed hashAlgorithm {hash}"),
            overrides: json!({ "signingConfiguration": { "hashAlgorithm": hash } }),
            outside: None,
        });
    }
    for encryption in &encryption_expressible {
        cases.push(OverrideCase {
            label: format!("{platform_id}: allowed encryptionAlgorithm {encryption}"),
            overrides: json!({ "signingConfiguration": { "encryptionAlgorithm": encryption } }),
            outside: None,
        });
    }
    if let (Some(hash), Some(encryption)) =
        (hash_expressible.first(), encryption_expressible.first())
    {
        cases.push(OverrideCase {
            label: format!("{platform_id}: both allowed ({hash}, {encryption})"),
            overrides: json!({
                "signingConfiguration": {
                    "hashAlgorithm": hash,
                    "encryptionAlgorithm": encryption,
                },
            }),
            outside: None,
        });
        // An override of the signing IMAGE FORMAT is a different member with its
        // own documented options — "a customer can select either of two signing
        // formats, JSONEmbedded or JSONDetached"
        // (`SigningPlatformOverrides$signingImageFormat`) — so it rides along with
        // allowed algorithms without changing the verdict.
        cases.push(OverrideCase {
            label: format!("{platform_id}: allowed algorithms with an image format"),
            overrides: json!({
                "signingConfiguration": {
                    "hashAlgorithm": hash,
                    "encryptionAlgorithm": encryption,
                },
                "signingImageFormat": "JSONDetached",
            }),
            outside: None,
        });
    }

    let outside_hashes: Vec<&&str> = HASH_ALGORITHM_VALUES
        .iter()
        .filter(|value| !hash_allowed.contains(value))
        .collect();
    let outside_encryptions: Vec<&&str> = ENCRYPTION_ALGORITHM_VALUES
        .iter()
        .filter(|value| !encryption_allowed.contains(value))
        .collect();

    for hash in &outside_hashes {
        cases.push(OverrideCase {
            label: format!("{platform_id}: hashAlgorithm {hash} outside allowedValues"),
            overrides: json!({ "signingConfiguration": { "hashAlgorithm": hash } }),
            outside: Some((
                SigningConfigurationMember::HashAlgorithm,
                (**hash).to_string(),
            )),
        });
    }
    for encryption in &outside_encryptions {
        cases.push(OverrideCase {
            label: format!("{platform_id}: encryptionAlgorithm {encryption} outside allowedValues"),
            overrides: json!({ "signingConfiguration": { "encryptionAlgorithm": encryption } }),
            outside: Some((
                SigningConfigurationMember::EncryptionAlgorithm,
                (**encryption).to_string(),
            )),
        });
    }
    // Both members outside at once: the refusal names the hash algorithm, the
    // member the handler judges first.
    if let (Some(hash), Some(encryption)) = (outside_hashes.first(), outside_encryptions.first()) {
        cases.push(OverrideCase {
            label: format!("{platform_id}: both members outside allowedValues"),
            overrides: json!({
                "signingConfiguration": {
                    "hashAlgorithm": hash,
                    "encryptionAlgorithm": encryption,
                },
            }),
            outside: Some((
                SigningConfigurationMember::HashAlgorithm,
                (**hash).to_string(),
            )),
        });
    }

    cases
}

/// An `overrides.signingConfiguration` value the platform publishes as allowed is
/// stored and read back; one outside the platform's `allowedValues` is refused 400
/// `ValidationException` and stores nothing.
///
/// One block of rows per published platform, generated from what that platform
/// publishes and from the modelled enum values.
#[tokio::test]
async fn a_signing_configuration_override_must_fall_inside_the_platforms_allowed_values() {
    let router = emulator();

    let mut refused_rows = 0usize;
    let mut accepted_rows = 0usize;
    for (platform_ordinal, entry) in SIGNING_PLATFORM_CATALOG.iter().enumerate() {
        let platform = entry.platform_id;
        let (mut accepted_here, mut refused_here) = (0usize, 0usize);
        for (ordinal, case) in
            override_cases(platform, entry.hash_algorithms, entry.encryption_algorithms)
                .into_iter()
                .enumerate()
        {
            let name = format!("Override_{platform_ordinal}_{ordinal}");
            let answer = put_profile(
                &router,
                &name,
                json!({
                    "platformId": platform,
                    "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
                    "overrides": case.overrides,
                }),
            )
            .await;

            match &case.outside {
                None => {
                    accepted_rows += 1;
                    accepted_here += 1;
                    assert_eq!(
                        answer.status, 200,
                        "case {}: expected PutSigningProfile 200, got {} {:?}",
                        case.label, answer.status, answer.body
                    );
                    // An accepted override is what a later read reports, member
                    // for member.
                    let read_back = get_profile(&router, &name).await;
                    assert_eq!(
                        read_back.status, 200,
                        "case {}: expected the created profile to be readable, got {} {:?}",
                        case.label, read_back.status, read_back.body
                    );
                    assert_eq!(
                        read_back.body["overrides"], case.overrides,
                        "case {}: expected the overrides to round-trip as {:?}, got {:?}",
                        case.label, case.overrides, read_back.body["overrides"]
                    );
                }
                Some((member, value)) => {
                    refused_rows += 1;
                    refused_here += 1;
                    assert_eq!(
                        answer.status, 400,
                        "case {}: expected HTTP 400, got {} {:?}",
                        case.label, answer.status, answer.body
                    );
                    assert_eq!(
                        answer.shape(),
                        "ValidationException",
                        "case {}: expected the modelled 400 shape, got {:?}",
                        case.label,
                        answer.body
                    );
                    assert_eq!(
                        answer.error_type.as_deref(),
                        Some("ValidationException"),
                        "case {}: expected x-amzn-errortype ValidationException, got {:?}",
                        case.label,
                        answer.error_type
                    );
                    assert_eq!(
                        answer.code(),
                        Some(ERROR_CODE_VALIDATION),
                        "case {}: expected the modelled code, got {:?}",
                        case.label,
                        answer.body
                    );
                    // The refusal says which member carried what the platform
                    // does not publish, on which platform, and what it publishes
                    // instead.
                    for expected in [member.wire_name(), value.as_str(), platform] {
                        assert!(
                            answer.message().contains(expected),
                            "case {}: expected the refusal to name {expected:?}, got {:?}",
                            case.label,
                            answer.body["message"]
                        );
                    }
                    for allowed in member.allowed_values(entry) {
                        assert!(
                            answer.message().contains(allowed),
                            "case {}: expected the refusal to name the allowed value {allowed:?}, \
                             got {:?}",
                            case.label,
                            answer.body["message"]
                        );
                    }

                    let read_back = get_profile(&router, &name).await;
                    assert_eq!(
                        read_back.status, 404,
                        "case {}: a refused create must store no profile, got {} {:?}",
                        case.label, read_back.status, read_back.body
                    );
                }
            }
        }
        // The rows are meant to span both verdicts for EVERY published platform; a
        // generator that silently produced only one kind for one of them would
        // make the assertions above vacuous for that platform.
        assert!(
            accepted_here > 0 && refused_here > 0,
            "case {platform:?}: expected both accepted and refused override rows, got \
             {accepted_here} accepted and {refused_here} refused"
        );
    }

    assert!(
        accepted_rows > 0 && refused_rows > 0,
        "expected the table to carry rows of both verdicts, got {accepted_rows} accepted and \
         {refused_rows} refused"
    );
}
