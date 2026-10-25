// SPDX-License-Identifier: CC0-1.0

//! The encoding `CreateWorkerConfiguration` holds `propertiesFileContent` to, end
//! to end over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON.
//!
//! The rule under test: a member the model documents as carrying base64-encoded
//! content is DECODED by the service to read what it carries, so a value no
//! base64 encoding produces — a character outside the alphabet, or a length no
//! encoding yields — is refused with the operation's modelled
//! `BadRequestException` (HTTP 400) naming that member, and nothing is stored;
//! every value a legal encoding produces is accepted and read back verbatim,
//! whatever bytes it decodes to.
//!
//! `propertiesFileContent` (`CreateWorkerConfigurationRequest`, "Base64 encoded
//! contents of connect-distributed.properties file") is the only member of this
//! service the model documents that way, so it is the one the table drives. The
//! rows differ along what the rule generalises over: the padded form, the length,
//! which alphabet characters the body carries, how big it is, and — on the refused
//! side — which way the value leaves the encoding.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the three it needs.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// carries. No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-1";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A member of a JSON object as a string, or `""`.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every base64 body the service must accept, each with the reason it is a legal
/// encoding.
///
/// The alphabet and the padding rules are RFC 4648 section 4's, the encoding the
/// member's documentation names ("Base64 encoded contents of
/// connect-distributed.properties file"); its last two characters are `+` and
/// `/`, which one of the rows is made of.
fn accepted() -> Vec<(&'static str, String)> {
    vec![
        (
            // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
            "a padded one-line properties file",
            "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=".to_string(),
        ),
        (
            // Base64 of a two-line properties file, so the decoded body carries a newline.
            "a padded multi-line properties file",
            "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXIKdmFsdWUuY29udmVydGVyPW9yZy5hcGFjaGUua2Fma2EuY29ubmVjdC5zdG9yYWdlLlN0cmluZ0NvbnZlcnRlcgo=".to_string(),
        ),
        // `QUJD` is base64 of `ABC`: three bytes, so the encoding needs no padding.
        ("a body needing no padding", "QUJD".to_string()),
        // `QQ==` is base64 of `A`, and `QQ` the same unit written without its
        // padding: a decoder reads both.
        ("a unit carrying two padding characters", "QQ==".to_string()),
        ("the same unit left unpadded", "QQ".to_string()),
        // `QUJDQUI=` is base64 of `ABCAB`: a whole unit and one padded to a
        // single `=`.
        (
            "a unit carrying one padding character",
            "QUJDQUI=".to_string(),
        ),
        // The two non-alphanumeric characters of the standard alphabet, 62 and 63.
        ("a body of the alphabet's `+` and `/`", "+/+/".to_string()),
        // `__sensitiveString` carries no length constraint, so every size from
        // empty upwards is a legal request.
        ("an empty body: nothing encoded", String::new()),
        ("kilobytes of properties", "QUJD".repeat(1024)),
    ]
}

/// Every value the service must refuse, each with the way it leaves the base64
/// encoding.
fn refused() -> Vec<(&'static str, String)> {
    vec![
        // Spaces and `!` are outside the alphabet.
        ("a sentence that is not base64", "not base64!!".to_string()),
        // The properties file sent as-is: `.` and `=` mid-value are not base64.
        (
            "the properties file sent unencoded",
            "key.converter=org.apache.kafka.connect.storage.StringConverter".to_string(),
        ),
        ("a single character: no unit at all", "Q".to_string()),
        ("one character past a whole unit", "QUJDQ".to_string()),
        ("padding where a unit needs none", "QUJD=".to_string()),
        ("more padding than any unit takes", "QUJD====".to_string()),
        ("a space inside the body", "QU JD".to_string()),
        // No encoding leaves non-zero spare bits in its final unit: `QR==` is not
        // the encoding of any byte (`QQ==` is the encoding of `A`).
        (
            "a final unit carrying non-zero trailing bits",
            "QR==".to_string(),
        ),
        ("a line break inside the body", "a2V5\nLmNvbnZlcnRlcg==".to_string()),
        // `-` and `_` are the URL-safe alphabet's 62 and 63, not the standard one's.
        ("the URL-safe alphabet", "QUJD-_8=".to_string()),
        // A character outside ASCII cannot be in any base64 alphabet.
        ("a character outside ASCII", "QUJ\u{00c9}".to_string()),
    ]
}

/// The `CreateWorkerConfiguration` body for `name` carrying `properties`.
fn worker_request(name: &str, properties: &str) -> Value {
    json!({"name": name, "propertiesFileContent": properties})
}

#[tokio::test]
async fn a_properties_file_that_is_legal_base64_is_stored_and_reads_back_verbatim() {
    let emulator = emulator();

    for (index, (case, properties)) in accepted().into_iter().enumerate() {
        let name = format!("accepted-{index}");
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/worker-configurations",
            worker_request(&name, &properties),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a legal base64 body must be accepted, got {status} {created}"
        );

        // And it is the STORED body a read hands back, byte for byte.
        let arn = text(&created, "workerConfigurationArn");
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the created configuration must read back, got {status} {described}"
        );
        let read_back = text(&described["latestRevision"], "propertiesFileContent");
        assert_eq!(
            read_back, properties,
            "case {case:?}: the stored properties file must come back unchanged, got a \
             {}-byte body",
            read_back.len()
        );
    }
}

#[tokio::test]
async fn a_properties_file_that_is_not_base64_is_refused_and_nothing_is_stored() {
    let emulator = emulator();

    for (index, (case, properties)) in refused().into_iter().enumerate() {
        let name = format!("refused-{index}");
        let (status, answered) = call(
            &emulator,
            "POST",
            "/v1/worker-configurations",
            worker_request(&name, &properties),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a propertiesFileContent that is not base64 is a 400, got \
             {status} {answered}"
        );
        let code = text(&answered, "__type");
        assert!(
            code.contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {code:?} in {answered}"
        );

        // The refusal names the member it blames, in this service's one
        // parameter-rejection form, and quotes none of the value: the member is
        // modelled sensitive.
        let message = text(&answered, "message");
        let expected_prefix = "Invalid parameter propertiesFileContent: ";
        assert!(
            message.starts_with(expected_prefix) && message.len() > expected_prefix.len(),
            "case {case:?}: expected a message starting {expected_prefix:?} and carrying a \
             reason, got {message:?}"
        );
        if properties.len() >= 8 {
            assert!(
                !message.contains(properties.as_str()),
                "case {case:?}: the refusal must not echo the sensitive body, got {message:?}"
            );
        }

        // Nothing was stored: the same name with a legal body is created rather
        // than refused as a duplicate, and reads back with THAT body.
        let legal = "QUJD";
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/worker-configurations",
            worker_request(&name, legal),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused create stored nothing, so the name is free; got \
             {status} {created}"
        );
        let arn = text(&created, "workerConfigurationArn");
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accepted configuration must read back, got {status} {described}"
        );
        assert_eq!(
            text(&described["latestRevision"], "propertiesFileContent"),
            legal,
            "case {case:?}: the stored body is the accepted one, not the refused one, got \
             {described}"
        );
    }
}
