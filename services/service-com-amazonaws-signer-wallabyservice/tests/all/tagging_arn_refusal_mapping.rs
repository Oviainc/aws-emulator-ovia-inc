// SPDX-License-Identifier: CC0-1.0

//! The ONE mapping from a tagging `resourceArn` that reaches no signing profile to
//! the modelled error that answers it, driven end to end over HTTP through each of
//! the three operations bound to `/tags/{resourceArn}`.
//!
//! The rule under test: for every tagging operation, a `resourceArn` that is not a
//! signing-profile ARN at all is a bad REQUEST — 400 `BadRequestException`, code
//! `BadRequestException`, "Invalid input resource arn: bad syntax" — while a
//! well-formed signing-profile ARN naming no stored profile is a missing RESOURCE —
//! 404 `NotFoundException`, code `ProfileNotFound`, "profile with arn <arn> does not
//! exists". The condition decides the answer; WHICH of the three operations was
//! called does not, so the three answers to one ARN are the same status, the same
//! error shape, the same code and the same message.
//!
//! That last part is what this suite adds: the mapping is expanded once per
//! operation from a single statement of it, and a copy of it drifting in one handler
//! — a 404 where another answers 400 — is invisible at a call site. Here it is not:
//! every row compares `TagResource`, `UntagResource` and `ListTagsForResource`
//! against each other as well as against the expected answer.
//!
//! The rows span what the rule generalises over: which part of the label makes it
//! unresolvable (no ARN at all, another service's ARN, a Signer ARN naming another
//! resource kind, the version ARN of a profile that does exist, an ARN with an empty
//! region field) and which part makes a well-formed profile ARN name nothing (an
//! unknown name, another region, another account). A control row drives the same
//! three operations with a resolvable ARN, so a handler refusing everything fails
//! this suite instead of passing it.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer,
/// and whose region a profile ARN is built for.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_US_WEST_2`] signs for.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN for `signingMaterial`.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// The status an admitted `TagResource` / `UntagResource` answers with: both
/// response shapes declare no members, so there is nothing to report.
const NO_CONTENT_STATUS: u16 = 204;

/// The status a tagging READ answers with: it reports a document.
const DOCUMENT_STATUS: u16 = 200;

/// The three operations bound to `/tags/{resourceArn}` — each one a former copy of
/// the mapping under test.
const TAGGING_OPERATIONS: [&str; 3] = ["TagResource", "UntagResource", "ListTagsForResource"];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status, JSON body, and the `x-amzn-errortype` header
/// restJson1 resolves an error SHAPE from.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    body: String,
    error_type: Option<String>,
}

impl Answer {
    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_string)
    }

    /// The modelled `message` member, or `None` when the body carries none.
    fn message(&self) -> Option<String> {
        self.json()["message"].as_str().map(str::to_string)
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// `value` percent-encoded for ONE URI segment or query value: `resourceArn` is a
/// non-greedy `@httpLabel` carrying `:` and `/`, so an SDK encodes everything
/// outside the unreserved set, which is what keeps the ARN one segment.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
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
    Answer {
        status,
        body: String::from_utf8_lossy(&bytes).to_string(),
        error_type,
    }
}

/// Create an ACTIVE signing profile and answer the ARN it was stored with.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let put = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": "AWSLambda-SHA384-ECDSA",
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "seeding {name}: expected PutSigningProfile 200, got {} {:?}",
        put.status, put.body
    );
    put.json()["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutSigningProfile must report an arn: {:?}", put.body))
        .to_string()
}

/// The `profileVersion` the stored profile carries now.
async fn profile_version(router: &MultiServiceRouter, name: &str) -> String {
    let got = call(router, "GET", &format!("/signing-profiles/{name}"), None).await;
    got.json()["profileVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("GetSigningProfile must report a profileVersion: {:?}", got.body))
        .to_string()
}

/// Drive `operation` against `resource_arn`. Each call carries the values that make
/// it a well-formed request of that operation, so the only thing left to refuse is
/// the ARN.
async fn tagging_call(
    router: &MultiServiceRouter,
    operation: &str,
    resource_arn: &str,
) -> Answer {
    let path = format!("/tags/{}", encode(resource_arn));
    match operation {
        "TagResource" => {
            let tags: Map<String, Value> = [("env".to_string(), Value::String("prod".to_string()))]
                .into_iter()
                .collect();
            call(
                router,
                "POST",
                &path,
                Some(json!({ "tags": Value::Object(tags) })),
            )
            .await
        }
        "UntagResource" => {
            call(
                router,
                "DELETE",
                &format!("{path}?tagKeys={}", encode("env")),
                None,
            )
            .await
        }
        "ListTagsForResource" => call(router, "GET", &path, None).await,
        other => panic!("unknown tagging operation {other}"),
    }
}

/// A `resourceArn` no tagging call can resolve, and the ONE answer every tagging
/// operation owes it.
struct Unresolvable {
    case: &'static str,
    resource_arn: String,
    status: u16,
    /// The modelled error SHAPE, which is also the `x-amzn-errortype` value.
    shape: &'static str,
    /// The `code` member naming the condition that was refused.
    code: &'static str,
    /// The prose beside it.
    message: String,
}

/// The syntax refusal's prose: it is a statement about the LABEL, so it carries
/// none of the value.
const BAD_SYNTAX: &str = "Invalid input resource arn: bad syntax";

/// The missing-resource prose, built from the ARN the request carried.
fn no_such_profile(resource_arn: &str) -> String {
    format!("profile with arn {resource_arn} does not exists")
}

/// A label that is not a signing-profile ARN is a bad REQUEST (400
/// `BadRequestException`), a well-formed profile ARN naming nothing stored is a
/// missing RESOURCE (404 `NotFoundException`), and every one of the three tagging
/// operations answers one ARN the same status, shape, code and message.
#[tokio::test]
async fn every_tagging_operation_answers_one_unresolvable_arn_identically() {
    let router = emulator();
    let profile = "Shared_Mapping";
    let arn = seed_profile(&router, profile).await;
    let version = profile_version(&router, profile).await;

    let rows = vec![
        Unresolvable {
            case: "a label that is not an ARN at all",
            resource_arn: "not-an-arn".to_string(),
            status: 400,
            shape: "BadRequestException",
            code: "BadRequestException",
            message: BAD_SYNTAX.to_string(),
        },
        Unresolvable {
            case: "an ARN of another service",
            resource_arn: format!("arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/abc"),
            status: 400,
            shape: "BadRequestException",
            code: "BadRequestException",
            message: BAD_SYNTAX.to_string(),
        },
        Unresolvable {
            case: "a signer ARN naming another resource kind",
            resource_arn: format!("arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/abc"),
            status: 400,
            shape: "BadRequestException",
            code: "BadRequestException",
            message: BAD_SYNTAX.to_string(),
        },
        Unresolvable {
            // Tags belong to the profile, not to one of its versions.
            case: "the version ARN of a profile that does exist",
            resource_arn: format!("{arn}/{version}"),
            status: 400,
            shape: "BadRequestException",
            code: "BadRequestException",
            message: BAD_SYNTAX.to_string(),
        },
        Unresolvable {
            case: "an ARN whose region field is empty",
            resource_arn: format!("arn:aws:signer::{ACCOUNT_ID}:/signing-profiles/{profile}"),
            status: 400,
            shape: "BadRequestException",
            code: "BadRequestException",
            message: BAD_SYNTAX.to_string(),
        },
        Unresolvable {
            case: "a well-formed ARN naming a profile that was never created",
            resource_arn: format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile"
            ),
            status: 404,
            shape: "NotFoundException",
            code: "ProfileNotFound",
            message: no_such_profile(&format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/No_Such_Profile"
            )),
        },
        Unresolvable {
            case: "the stored profile's name in ANOTHER region",
            resource_arn: format!(
                "arn:aws:signer:eu-west-1:{ACCOUNT_ID}:/signing-profiles/{profile}"
            ),
            status: 404,
            shape: "NotFoundException",
            code: "ProfileNotFound",
            message: no_such_profile(&format!(
                "arn:aws:signer:eu-west-1:{ACCOUNT_ID}:/signing-profiles/{profile}"
            )),
        },
        Unresolvable {
            case: "the stored profile's name in ANOTHER account",
            resource_arn: format!(
                "arn:aws:signer:{REGION}:123456789012:/signing-profiles/{profile}"
            ),
            status: 404,
            shape: "NotFoundException",
            code: "ProfileNotFound",
            message: no_such_profile(&format!(
                "arn:aws:signer:{REGION}:123456789012:/signing-profiles/{profile}"
            )),
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut answers: Vec<(&str, Answer)> = Vec::with_capacity(TAGGING_OPERATIONS.len());
        for operation in TAGGING_OPERATIONS {
            let answer = tagging_call(&router, operation, &row.resource_arn).await;
            assert_eq!(
                answer.status, row.status,
                "case {case:?} on {operation}: expected HTTP {}, got {} {:?}",
                row.status, answer.status, answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some(row.shape),
                "case {case:?} on {operation}: expected x-amzn-errortype {}, got {:?}",
                row.shape,
                answer.error_type
            );
            assert_eq!(
                answer.code().as_deref(),
                Some(row.code),
                "case {case:?} on {operation}: expected the modelled code {}, got {:?}",
                row.code,
                answer.body
            );
            assert_eq!(
                answer.message().as_deref(),
                Some(row.message.as_str()),
                "case {case:?} on {operation}: expected the message {:?}, got {:?}",
                row.message,
                answer.message()
            );
            answers.push((operation, answer));
        }

        // The condition decides the answer, not which operation was called: one
        // handler answering 404 where another answers 400 for the same ARN is the
        // drift this compares away.
        let (first_operation, first) = &answers[0];
        for (operation, answer) in &answers[1..] {
            assert_eq!(
                (answer.status, answer.code(), answer.message()),
                (first.status, first.code(), first.message()),
                "case {case:?}: expected {operation} to answer exactly what {first_operation} \
                 answers, got {answer:?} against {first:?}"
            );
            assert_eq!(
                answer.error_type, first.error_type,
                "case {case:?}: expected {operation} to answer the same error shape as \
                 {first_operation}, got {:?} against {:?}",
                answer.error_type, first.error_type
            );
        }
    }

    // The control: the SAME three operations, driven with the ARN the profile was
    // stored under, are served. Without it a handler that refused every ARN would
    // satisfy every row above.
    let served: BTreeMap<&str, u16> = [
        ("TagResource", NO_CONTENT_STATUS),
        ("UntagResource", NO_CONTENT_STATUS),
        ("ListTagsForResource", DOCUMENT_STATUS),
    ]
    .into_iter()
    .collect();
    for operation in TAGGING_OPERATIONS {
        let answer = tagging_call(&router, operation, &arn).await;
        let expected = served[operation];
        assert_eq!(
            answer.status, expected,
            "case \"the stored profile's own ARN\" on {operation}: expected HTTP {expected}, got \
             {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, None,
            "case \"the stored profile's own ARN\" on {operation}: expected no error shape, got \
             {:?}",
            answer.error_type
        );
    }
}
