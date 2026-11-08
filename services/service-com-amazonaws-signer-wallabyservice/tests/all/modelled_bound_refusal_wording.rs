// SPDX-License-Identifier: CC0-1.0

//! How this service words a refusal of a value one of its MODELLED bounds
//! rejects, end to end over HTTP.
//!
//! The rule under test: where AWS Signer publishes a rule of its own for a bound
//! the model declares, a request refused for exactly that bound is answered with
//! that rule — composed from what the MODEL declares, and carrying nothing from
//! the request — rather than with the boundary validator's generic constraint
//! clause, which quotes the caller's value and the raw expression. Two bounds have
//! such a rule:
//!
//! * a signing-profile name outside the character class its shape's `@pattern`
//!   admits is answered with that class, stated as the rule a name must meet;
//! * a payload outside the byte window its blob's `@length` declares is answered
//!   with that window — both bounds stated, the offending size withheld.
//!
//! Every other bound keeps the generic clause, and so does a request that broke
//! more than one rule at once: this service publishes no single sentence for
//! those.
//!
//! The rows span what the rule generalises over. For the name: which operation
//! carries it and how it is bound (a path label on four routes across three
//! methods, a body member on two), which character puts the value out of class
//! (hyphen, dot, space, punctuation, a non-ASCII letter) and where in the value
//! that character sits — with in-class names admitted. For the payload: which side
//! of the window the size falls on (below the floor, one byte past the ceiling,
//! far past it), with each published bound itself admitted.

use std::sync::Arc;

use aws_smithy_schema::ShapeType;
use aws_smithy_schema_server::constraints::LengthTrait;
use aws_smithy_types::base64;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::ops::types::{
    PUT_SIGNING_PROFILE_INPUT_SCHEMA, SIGN_PAYLOAD_INPUT_SCHEMA,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A published `platformId` a create names, so a create that is admitted reaches
/// the platform catalog instead of being refused for something else.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The published `platformId` whose profiles sign a PAYLOAD — the workflow
/// `SignPayload` serves, so a request of that operation that is admitted is
/// answered by the signing itself.
const PAYLOAD_PLATFORM_ID: &str = "Notation-OCI-SHA384-ECDSA";

/// The one `payloadFormat` `SignPayload` signs: "The single valid type is
/// application/vnd.cncf.notary.payload.v1+json"
/// (`smithy_query(query="doc", shape="SignPayloadRequest",
/// member="payloadFormat")`).
const PAYLOAD_FORMAT: &str = "application/vnd.cncf.notary.payload.v1+json";

/// An ACM certificate ARN a create names as its signing material, for the same
/// reason.
const CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/12345678-1234-1234-1234-123456789012";

/// The modelled error shape every refusal below is answered with (HTTP 400).
const ERROR_SHAPE_VALIDATION: &str = "ValidationException";

/// The sentence this service states for a signing-profile name outside the
/// published character class, written out here rather than imported so the exact
/// bytes a client reads are pinned independently of the service's own constant.
const PROFILE_NAME_RULE: &str =
    "Profile name must contain only alphanumeric characters and underscore.";

/// The clause the boundary validator states when nothing reworded the refusal —
/// the fragment every generic constraint message carries.
const GENERIC_CLAUSE: &str = "failed to satisfy constraint";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `message` member, or the empty string when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// One request through the router, signed for [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
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
    Answer { status, body }
}

/// The `@pattern` the model declares for a signing-profile name: the expression on
/// the one `@httpLabel` member of the create's input (`/signing-profiles/\
/// {profileName}`), read off the generated schema so the test names the model's
/// expression rather than a copy of it.
fn declared_profile_name_pattern() -> String {
    let member = PUT_SIGNING_PROFILE_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| member.http_label().is_some())
        .expect("the create binds the profile name as a path label");
    let declared = member
        .traits()
        .and_then(|traits| {
            traits.get(&aws_smithy_schema_server::constraints::PatternTrait::TRAIT_ID)
        })
        .and_then(|found| {
            found
                .as_any()
                .downcast_ref::<aws_smithy_schema_server::constraints::PatternTrait>()
        })
        .expect("the profile-name member declares @pattern");
    declared.regex.to_string()
}

/// The byte window the model declares for the payload `SignPayload` signs: the
/// `@length` of its one blob input member, read off the generated schema so the
/// expectation below is the model's bounds and not a second copy of them.
fn declared_payload_window() -> (i64, i64) {
    let member = SIGN_PAYLOAD_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| matches!(member.shape_type(), ShapeType::Blob))
        .expect("SignPayload declares a blob input member");
    let declared = member
        .traits()
        .and_then(|traits| traits.get(&LengthTrait::TRAIT_ID))
        .and_then(|found| found.as_any().downcast_ref::<LengthTrait>())
        .expect("the payload member declares @length");
    (
        declared.min.expect("the payload @length declares a minimum"),
        declared.max.expect("the payload @length declares a maximum"),
    )
}

/// The sentence this service states for a payload outside that window.
fn payload_window_rule() -> String {
    let (min, max) = declared_payload_window();
    format!("Payload does not meet size requirements: [{min}B - {max}B]")
}

/// Every request that carries `name` as the signing-profile name, one per
/// operation and binding under test, with every OTHER member well formed so the
/// only rule the request can break is the one about the name.
///
/// A `name` reaching a path label is sent LITERALLY, so every name handed to this
/// function has to be one a URI path segment carries as it is (RFC 3986 `pchar`);
/// a name outside that is driven through [`body_requests_carrying`] instead.
fn requests_carrying(name: &str) -> Vec<(String, &'static str, String, Option<Value>)> {
    let mut requests = vec![
        (
            format!("PutSigningProfile carrying {name:?} as a path label"),
            "PUT",
            format!("/signing-profiles/{name}"),
            Some(json!({
                "platformId": PLATFORM_ID,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        ),
        (
            format!("GetSigningProfile carrying {name:?} as a path label"),
            "GET",
            format!("/signing-profiles/{name}"),
            None,
        ),
        (
            format!("CancelSigningProfile carrying {name:?} as a path label"),
            "DELETE",
            format!("/signing-profiles/{name}"),
            None,
        ),
        (
            format!("ListProfilePermissions carrying {name:?} as a subresource label"),
            "GET",
            format!("/signing-profiles/{name}/permissions"),
            None,
        ),
    ];
    requests.extend(body_requests_carrying(name));
    requests
}

/// The requests that carry `name` as a BODY member, where any character the
/// modelled string admits travels as JSON — no URI escaping involved.
fn body_requests_carrying(name: &str) -> Vec<(String, &'static str, String, Option<Value>)> {
    vec![
        (
            format!("StartSigningJob carrying {name:?} as a body member"),
            "POST",
            "/signing-jobs".to_string(),
            Some(json!({
                "source": { "s3": { "bucketName": "src", "key": "code.bin", "version": "1" } },
                "destination": { "s3": { "bucketName": "dst", "prefix": "signed/" } },
                "profileName": name,
                "clientRequestToken": "a-token-for-one-row",
            })),
        ),
        (
            format!("SignPayload carrying {name:?} as a body member"),
            "POST",
            "/signing-jobs/with-payload".to_string(),
            Some(json!({
                "profileName": name,
                "payload": base64::encode([7_u8].as_slice()),
                "payloadFormat": "application/octet-stream",
            })),
        ),
    ]
}

/// A name outside the published character class is refused with the class, on
/// every operation that carries a name and whichever character put it out.
#[tokio::test]
async fn a_profile_name_outside_the_published_class_is_refused_with_that_class() {
    let router = emulator();
    let pattern = declared_profile_name_pattern();

    // Two tables, both spanning the character that puts a name out of class and
    // where in the value it sits. Every row is inside the modelled length bounds
    // (2..64), so the class is the ONLY rule it breaks.
    //
    // These names are ones a URI path segment carries literally, so each is driven
    // through every binding: four path labels and both body members.
    let in_a_segment = [
        "Bad-Name",
        "bad.name",
        "sim2real-bad-name!",
        "trailing-",
        "-leading",
        "pro$file",
        "a~b",
        "x,y",
        "p:q",
        "u@v",
    ];
    // And these a URI segment does not, so they are driven through the body
    // members, where the modelled string's characters travel as JSON.
    let in_a_body = ["has space", "caf\u{e9}_profile", "tab\tseparated"];

    let rows = in_a_segment
        .iter()
        .flat_map(|name| {
            requests_carrying(name)
                .into_iter()
                .map(move |row| (*name, row))
        })
        .chain(in_a_body.iter().flat_map(|name| {
            body_requests_carrying(name)
                .into_iter()
                .map(move |row| (*name, row))
        }));

    for (name, (case, method, uri, body)) in rows {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected a {ERROR_SHAPE_VALIDATION}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            PROFILE_NAME_RULE,
            "case {case:?}: expected the published class, got {:?}",
            answer.message()
        );
        // The sentence is the RULE: it quotes neither the rejected value nor the
        // expression the walk matched against.
        assert!(
            !answer.message().contains(name),
            "case {case:?}: expected the rejected name withheld, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains(&pattern),
            "case {case:?}: expected the modelled expression withheld, got {:?}",
            answer.message()
        );
    }
}

/// A name the published class admits is not refused for the class — on the create,
/// which answers 200 and stores a profile. The rows span the class: letters of
/// both cases, digits, the underscore, the shortest admitted name and the longest.
#[tokio::test]
async fn a_profile_name_inside_the_published_class_is_admitted() {
    let router = emulator();
    let names = [
        "Ab".to_string(),
        "a_1".to_string(),
        "ALL_CAPS_0123456789".to_string(),
        "_".repeat(2),
        "A".repeat(64),
    ];

    for name in names {
        let answer = call(
            &router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(json!({
                "platformId": PLATFORM_ID,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected HTTP 200, got {} {:?}",
            answer.status, answer.body
        );

        // And it reads back through the implemented reader, so what was admitted
        // was stored.
        let read = call(
            &router,
            "GET",
            &format!("/signing-profiles/{name}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: expected the admitted profile to read back, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["profileName"].as_str(),
            Some(name.as_str()),
            "case {name:?}: expected the stored name reported, got {:?}",
            read.body
        );
    }
}

/// A payload outside the modelled byte window is refused with the WINDOW: both
/// bounds, read off the model, and no mention of the size the request carried.
#[tokio::test]
async fn a_payload_outside_the_modelled_window_is_refused_with_that_window() {
    let router = emulator();
    let (min, max) = declared_payload_window();
    let floor = usize::try_from(min).expect("the modelled minimum is a byte count");
    let ceiling = usize::try_from(max).expect("the modelled maximum is a byte count");

    // The rows differ along which side of the window the size falls on, and by how
    // far: the empty payload (one byte below the floor), one byte past the
    // ceiling, and far past it.
    let sizes = [floor - 1, ceiling + 1, ceiling * 4];

    for size in sizes {
        let case = format!("a {size}-byte payload");
        let answer = call(
            &router,
            "POST",
            "/signing-jobs/with-payload",
            Some(json!({
                "profileName": "Payload_Window_Profile",
                "payload": base64::encode(vec![7_u8; size].as_slice()),
                "payloadFormat": "application/octet-stream",
            })),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            ERROR_SHAPE_VALIDATION,
            "case {case:?}: expected a {ERROR_SHAPE_VALIDATION}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            payload_window_rule(),
            "case {case:?}: expected the modelled window stated, got {:?}",
            answer.message()
        );
        // The window is the requirement; the measurement of this request is not
        // part of it, in the form the boundary validator would have rendered it
        // or in any other.
        assert!(
            !answer.message().contains(&format!("{size} bytes")),
            "case {case:?}: expected the offending size withheld, got {:?}",
            answer.message()
        );
    }
}

/// A payload ON either published bound is not refused for its size: the request
/// gets past the boundary validator and is SERVED by the operation, which signs a
/// payload of either bound under a profile of a platform that signs payloads.
#[tokio::test]
async fn a_payload_on_a_published_bound_is_not_refused_for_its_size() {
    let router = emulator();
    let (min, max) = declared_payload_window();
    let bounds = [
        usize::try_from(min).expect("the modelled minimum is a byte count"),
        usize::try_from(max).expect("the modelled maximum is a byte count"),
    ];

    // A profile the operation can sign with, so a row that is admitted is answered
    // by the signing itself rather than refused for anything else about it.
    let profile = "Payload_Window_Profile";
    let seeded = call(
        &router,
        "PUT",
        &format!("/signing-profiles/{profile}"),
        Some(json!({ "platformId": PAYLOAD_PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "seeding the profile: got {} {:?}",
        seeded.status, seeded.body
    );

    for size in bounds {
        let case = format!("a {size}-byte payload, on a published bound");
        let answer = call(
            &router,
            "POST",
            "/signing-jobs/with-payload",
            Some(json!({
                "profileName": profile,
                "payload": base64::encode(vec![7_u8; size].as_slice()),
                "payloadFormat": PAYLOAD_FORMAT,
            })),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected a published bound admitted and signed, got {} {:?}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.message(),
            payload_window_rule(),
            "case {case:?}: expected no size refusal, got {:?}",
            answer.message()
        );
    }
}

/// The rewording is NARROW: a bound this service publishes no sentence for, and a
/// request that broke more than one rule at once, both keep the boundary
/// validator's generic clause.
#[tokio::test]
async fn a_bound_with_no_published_sentence_keeps_the_generic_clause() {
    let router = emulator();
    let (_, max) = declared_payload_window();
    let ceiling = usize::try_from(max).expect("the modelled maximum is a byte count");

    // (case, method, uri, body)
    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            // Longer than the modelled maximum, but every character is in class:
            // the only rule broken is the length ceiling, for which this service
            // publishes no sentence of its own.
            "a profile name past the modelled length ceiling",
            "PUT",
            format!("/signing-profiles/{}", "A".repeat(65)),
            Some(json!({
                "platformId": PLATFORM_ID,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        ),
        (
            // One character: below the modelled length floor AND outside the
            // class, which is two broken rules in one request.
            "a profile name that breaks the floor and the class at once",
            "PUT",
            "/signing-profiles/A".to_string(),
            Some(json!({
                "platformId": PLATFORM_ID,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        ),
        (
            // An oversized payload beside an out-of-class name: two rules again,
            // each of which has a sentence on its own.
            "an oversized payload beside an out-of-class name",
            "POST",
            "/signing-jobs/with-payload".to_string(),
            Some(json!({
                "profileName": "Bad-Name",
                "payload": base64::encode(vec![7_u8; ceiling + 1].as_slice()),
                "payloadFormat": "application/octet-stream",
            })),
        ),
        (
            // A paging bound on a listing: outside its modelled `@range`, and
            // nothing this service rewords.
            "a page size below the modelled range",
            "GET",
            "/signing-jobs?maxResults=0".to_string(),
            None,
        ),
    ];

    for (case, method, uri, body) in cases {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains(GENERIC_CLAUSE),
            "case {case:?}: expected the generic clause kept, got {:?}",
            answer.message()
        );
        assert_ne!(
            answer.message(),
            PROFILE_NAME_RULE,
            "case {case:?}: expected the class sentence not used, got {:?}",
            answer.message()
        );
        assert_ne!(
            answer.message(),
            payload_window_rule(),
            "case {case:?}: expected the window sentence not used, got {:?}",
            answer.message()
        );
    }
}
