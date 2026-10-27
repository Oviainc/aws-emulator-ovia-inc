// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the VALUE a constraint refusal of this service quotes back.
//!
//! The rule under test: an AWS constraint message is built from the request —
//! `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: <requirement>` — and the value between the quotes is the one the caller
//! SUBMITTED, in full. No shortened rendering of it, and no byte-count marker: a caller
//! that compares the echo against what it sent has to see its own bytes, whatever their
//! size and whichever member they were sent on.
//!
//! The rows differ along what the rule generalises over: the SIZE of the submitted value
//! (at a member's modelled maximum, one byte past it, far past it), the MEMBER it was sent
//! on and the OPERATION that refused it (`PutResourcePolicy$Policy`, modelled `len
//! 1..20480`, and `CreateAlias$Description`, modelled `len 0..256` — two different maxima,
//! two different routes, one on a `PUT` with an ARN label and one on a `POST`), and the
//! requirement the value broke. The maxima are the model's own
//! (`smithy_query(query="shape", shape="ResourcePolicy")` and
//! `smithy_query(query="operation", operation="CreateAlias")`), and each is tested AT the
//! bound, one past it and far past it.
//!
//! One bound is this emulator's and not AWS's, and is checked too: the quoted value is
//! capped at [`LARGEST_QUOTED_VALUE_BYTES`] so a response can never be sized by the
//! request. Past that cap the refusal still names the member, the requirement and the
//! modelled limit, and the message stays smaller than the value.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the resource-policy operations are bound
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`).
const RESOURCE_POLICY_URI: &str = "/2026-07-09/resource-policy";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here. Nothing in it is verified: the harness turns
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The modelled maximum of `PutResourcePolicy$Policy` (`ResourcePolicy`, `len 1..20480`).
const MAX_POLICY_BYTES: usize = 20_480;

/// The modelled maximum of `CreateAlias$Description` (`Description`, `len 0..256`).
const MAX_DESCRIPTION_BYTES: usize = 256;

/// The largest single value one clause of this service's refusal quotes back
/// (`service::LARGEST_QUOTED_VALUE_BYTES`): twice the largest `@length` maximum this model
/// puts on a string member. Past it the framework's own shortened rendering stands, which
/// is this emulator's bound on a response's size and not an AWS behaviour.
const LARGEST_QUOTED_VALUE_BYTES: usize = 2 * MAX_POLICY_BYTES;

/// The error code the JSON protocols answer a constraint refusal with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header), as the shape name
    /// alone.
    fn error_code(&self) -> String {
        let raw = self
            .string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string());
        raw.rsplit('#').next().unwrap_or(&raw).to_string()
    }

    /// The `message` of a wire error, however the protocol spells the member.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
    }

    /// The body text, for a failure message — truncated, because a body carrying a quoted
    /// value is as large as the value.
    fn text(&self) -> String {
        let text = String::from_utf8_lossy(&self.body).to_string();
        match text.char_indices().nth(400) {
            Some((at, _)) => format!("{}…", &text[..at]),
            None => text,
        }
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create a function, so the resources the requests below address exist.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// A policy document of exactly `bytes` bytes, padded inside the `Sid` of its one
/// statement so the document stays a policy whatever its size.
fn padded_document(bytes: usize) -> String {
    let skeleton = document_with("");
    let padding = bytes.saturating_sub(skeleton.len());
    let document = document_with(&"A".repeat(padding));
    assert_eq!(
        document.len(),
        bytes.max(skeleton.len()),
        "the padded document must be exactly {bytes} bytes"
    );
    document
}

/// A policy document whose `Sid` carries `padding`.
fn document_with(padding: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": format!("pad{padding}"),
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string()
}

/// The clause a `@length` maximum states for `value` at `path`, with the value quoted in
/// FULL — the sentence AWS answers, built here from the request the test sent and the
/// maximum the model declares.
fn length_clause(value: &str, path: &str, maximum: usize) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must have length less than or equal to {maximum}"
    )
}

/// One row of the quoting table: a request carrying `value` on `member`, and whether that
/// value is inside the member's modelled maximum.
struct QuoteCase {
    name: &'static str,
    /// How the request is sent, which decides the route and the member path.
    member: Member,
    /// The bytes the request carries on that member.
    value: String,
    /// The member's modelled `@length` maximum.
    maximum: usize,
}

/// Which member of which operation a row sends its value on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Member {
    /// `PutResourcePolicy$Policy`, a body member of a `PUT` whose URI label is an ARN.
    Policy,
    /// `CreateAlias$Description`, a body member of a `POST` under the function's name.
    AliasDescription,
}

impl Member {
    /// The member path a clause names this member by — the lowerCamel spelling the
    /// validating walk reports a violation at.
    fn path(self) -> &'static str {
        match self {
            Member::Policy => "policy",
            Member::AliasDescription => "description",
        }
    }
}

/// Send `case`'s value on its member, against the function `function`.
async fn send(router: &MultiServiceRouter, function: &str, case: &QuoteCase) -> Answer {
    match case.member {
        Member::Policy => {
            call(
                router,
                "PUT",
                &format!("{RESOURCE_POLICY_URI}/{}", function_arn(function)),
                &json!({ "Policy": case.value }),
            )
            .await
        }
        Member::AliasDescription => {
            call(
                router,
                "POST",
                &format!("{FUNCTIONS_URI}/{function}/aliases"),
                &json!({
                    "Name": "quoted",
                    "FunctionVersion": "$LATEST",
                    "Description": case.value,
                }),
            )
            .await
        }
    }
}

/// RULE: a value past its member's modelled `@length` maximum is refused with the
/// framework's envelope quoting the value AS SUBMITTED — the whole of it, with no
/// shortened rendering and no byte-count marker — for every member and every size inside
/// the bound this service quotes up to.
#[tokio::test]
async fn a_refusal_quotes_the_whole_value_the_caller_submitted() {
    // One past each published maximum, far past it, and at the ceiling this service
    // quotes up to; the recorded-size overshoot is one row among them, not the rule.
    let cases = vec![
        QuoteCase {
            name: "a policy one byte past its modelled maximum",
            member: Member::Policy,
            value: padded_document(MAX_POLICY_BYTES + 1),
            maximum: MAX_POLICY_BYTES,
        },
        QuoteCase {
            name: "a policy well past its modelled maximum",
            member: Member::Policy,
            value: padded_document(MAX_POLICY_BYTES + 1_560),
            maximum: MAX_POLICY_BYTES,
        },
        QuoteCase {
            name: "a policy at the largest value this service quotes",
            member: Member::Policy,
            value: padded_document(LARGEST_QUOTED_VALUE_BYTES),
            maximum: MAX_POLICY_BYTES,
        },
        QuoteCase {
            name: "a description one byte past its modelled maximum",
            member: Member::AliasDescription,
            value: "d".repeat(MAX_DESCRIPTION_BYTES + 1),
            maximum: MAX_DESCRIPTION_BYTES,
        },
        QuoteCase {
            name: "a description far past its modelled maximum",
            member: Member::AliasDescription,
            value: "wordy ".repeat(900),
            maximum: MAX_DESCRIPTION_BYTES,
        },
    ];

    for case in &cases {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, "quoted-fn").await;

        let answer = send(&router, "quoted-fn", case).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, VALIDATION_EXCEPTION.to_string()),
            "case {:?}: expected 400 {VALIDATION_EXCEPTION}, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        let reported = answer.message();
        let expected = length_clause(&case.value, case.member.path(), case.maximum);
        assert_eq!(
            reported,
            expected,
            "case {:?}: expected the clause quoting all {} submitted bytes, got a {} byte \
             message: {}",
            case.name,
            case.value.len(),
            reported.len(),
            answer.text()
        );
        assert!(
            !reported.contains("bytes]"),
            "case {:?}: expected no byte-count marker in the message, got {}",
            case.name,
            answer.text()
        );
    }
}

/// RULE: a value INSIDE its member's modelled maximum is not refused at all — the quoting
/// rule above changes what a refusal says, not which requests are refused.
#[tokio::test]
async fn a_value_inside_its_modelled_maximum_is_served() {
    let accepted: [(&str, Member, String, u16); 3] = [
        (
            "a policy at its modelled maximum",
            Member::Policy,
            padded_document(MAX_POLICY_BYTES),
            200,
        ),
        (
            "a policy well inside its modelled maximum",
            Member::Policy,
            padded_document(4_096),
            200,
        ),
        (
            "a description at its modelled maximum",
            Member::AliasDescription,
            "d".repeat(MAX_DESCRIPTION_BYTES),
            201,
        ),
    ];

    for (name, member, value, status) in accepted {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, "quoted-fn").await;
        let case = QuoteCase {
            name,
            member,
            value,
            maximum: 0,
        };
        let answer = send(&router, "quoted-fn", &case).await;
        assert_eq!(
            answer.status,
            status,
            "case {name:?}: expected HTTP {status}, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// RULE: past the ceiling this service quotes up to, the refusal still names the member,
/// the requirement and the modelled maximum, and the message is smaller than the value —
/// so no response is sized by the request.
#[tokio::test]
async fn a_value_past_the_quoting_ceiling_is_still_refused_and_bounded() {
    for (name, value) in [
        (
            "one byte past the quoting ceiling",
            padded_document(LARGEST_QUOTED_VALUE_BYTES + 1),
        ),
        (
            "far past the quoting ceiling",
            padded_document(4 * LARGEST_QUOTED_VALUE_BYTES),
        ),
    ] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, "quoted-fn").await;
        let answer = call(
            &router,
            "PUT",
            &format!("{RESOURCE_POLICY_URI}/{}", function_arn("quoted-fn")),
            &json!({ "Policy": value }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, VALIDATION_EXCEPTION.to_string()),
            "case {name:?}: expected 400 {VALIDATION_EXCEPTION}, got {} {}",
            answer.status,
            answer.text()
        );
        let reported = answer.message();
        assert!(
            reported.contains("at 'policy'")
                && reported.contains(&format!(
                    "Member must have length less than or equal to {MAX_POLICY_BYTES}"
                )),
            "case {name:?}: expected the length clause for the policy member, got {}",
            answer.text()
        );
        assert!(
            reported.len() < value.len(),
            "case {name:?}: expected a message smaller than the {} byte value, got {} bytes",
            value.len(),
            reported.len()
        );
    }
}

/// RULE: a refused write changes nothing — the policy the resource already held is still
/// the one a later read reports, whatever the refused document's size.
#[tokio::test]
async fn a_refused_oversized_policy_leaves_the_stored_policy_untouched() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "quoted-fn").await;
    let arn = function_arn("quoted-fn");

    let stored = padded_document(2_048);
    let put = call(
        &router,
        "PUT",
        &format!("{RESOURCE_POLICY_URI}/{arn}"),
        &json!({ "Policy": stored }),
    )
    .await;
    assert_eq!(
        put.status,
        200,
        "the first policy must be stored, got {} {}",
        put.status,
        put.text()
    );
    let revision = put
        .string("RevisionId")
        .expect("a stored policy answers a RevisionId");

    for (name, bytes) in [
        ("one byte past the maximum", MAX_POLICY_BYTES + 1),
        ("past the quoting ceiling", LARGEST_QUOTED_VALUE_BYTES + 1),
    ] {
        let refused = call(
            &router,
            "PUT",
            &format!("{RESOURCE_POLICY_URI}/{arn}"),
            &json!({ "Policy": padded_document(bytes) }),
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {name:?}: expected the oversized document to be refused, got {} {}",
            refused.status,
            refused.text()
        );
        let read = call(
            &router,
            "GET",
            &format!("{RESOURCE_POLICY_URI}/{arn}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (
                read.status,
                read.string("Policy").map(|policy| policy.len()),
                read.string("RevisionId")
            ),
            (200, Some(stored.len()), Some(revision.clone())),
            "case {name:?}: expected the stored policy and its revision unchanged, got {} {}",
            read.status,
            read.text()
        );
    }
}
