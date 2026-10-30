// SPDX-License-Identifier: CC0-1.0

//! How `GET /revocations` answers a `platformId` outside the character class its
//! published contract declares for one, end to end over HTTP.
//!
//! The rule under test: a value outside a character class the API publishes for
//! an input member is refused at the request boundary — HTTP 400
//! `ValidationException` in the constraint envelope naming the member path and
//! the published expression — BEFORE the value is resolved against anything the
//! service holds. So the two answers a `platformId` can take stay distinct: a
//! value the class excludes is refused for its syntax (it is not a platform id
//! at all), while a value the class admits that names no published platform is
//! reported as a platform that does not exist (well formed, unknown).
//!
//! The cases are tables spanning what the rule generalises over: which character
//! puts the value outside the class (a space, `!`, `/`, `.`, `+`, `:`, a
//! non-ASCII letter), where in the value it sits (leading, inside, trailing),
//! the empty value, how long the value is — and, on the other side, in-class
//! values that are unknown, and every id the catalog actually publishes.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_signer_wallabyservice::state::{
    platform_id_pattern_refusal, REVOCATION_PLATFORM_ID_PATTERN, SIGNING_PLATFORM_CATALOG,
};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The account id every generated emulator presents (`EMULATOR_ACCOUNT_ID`),
/// spelled out here so a wrong account in the handlers fails.
const ACCOUNT_ID: &str = "000000000000";

/// The instant the `signatureTimestamp` of every request below names.
const BASE_EPOCH_SECS: i64 = 1_700_000_000;

/// The character class the route's published contract declares for a
/// `platformId`, written out rather than imported: a refusal quotes the
/// expression, so the text a client matches on is pinned here independently of
/// the service's own copy (which
/// [`the_published_expression_is_the_one_the_service_states`] holds to it).
const PLATFORM_ID_PATTERN: &str = "^[a-zA-Z0-9_-]+$";

/// The query string of one case: the `@httpQuery` parameters it carries.
type Params = Vec<(&'static str, String)>;

/// A SigV4 credential scope naming this service for `region`. It is what routes a
/// request to signer.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

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
    /// The wire error shape (`__type`), or the empty string on a success.
    fn shape(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The modelled `code` member, or the empty string when the body carries
    /// none.
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or_default()
    }

    /// The `message` member, or the empty string when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// `value` percent-encoded for ONE query value, which is what an SDK sends: a
/// `platformId` the class excludes carries characters a query string cannot hold
/// literally (a space, a `+`, a `%`), so everything outside the unreserved set
/// is encoded and the handler sees the value the case names.
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

/// `GET /revocations` with exactly the query parameters given, each value
/// encoded the way a client sends it.
async fn get_revocations(router: &MultiServiceRouter, params: &Params) -> Answer {
    let query = params
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("/revocations?{query}");
    let request = ::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION))
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A `signatureTimestamp` query value: the ISO 8601 (`date-time`) rendering a
/// `@httpQuery`-bound timestamp carries by default.
fn timestamp_param() -> String {
    DateTime::from_secs(BASE_EPOCH_SECS)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// A certificate hash of the documented shape: two hex SHA-384 digests, the
/// subject certificate's TBS hash followed by its parent CA's
/// (`smithy_query(query="doc",shape="GetRevocationStatusRequest",
/// member="certificateHashes")`).
fn certificate_hash(child: &str, parent: &str) -> String {
    use sha2::Digest;
    format!(
        "{}{}",
        hex::encode(sha2::Sha384::digest(child.as_bytes())),
        hex::encode(sha2::Sha384::digest(parent.as_bytes()))
    )
}

/// A published `platformId`, taken from the catalog this service serves rather
/// than written out.
fn published_platform_id() -> &'static str {
    SIGNING_PLATFORM_CATALOG
        .first()
        .expect("this service publishes at least one signing platform")
        .platform_id
}

/// Every required query parameter of `GetRevocationStatus`, in the order the
/// model declares them, with `platform_id` as the one under test and every other
/// value one this endpoint serves: well-formed ARNs of this account and region
/// naming nothing stored, which the operation reports as "not revoked" rather
/// than refusing, so a case is only ever answered for its `platformId`.
fn request_for(platform_id: &str) -> Params {
    vec![
        ("signatureTimestamp", timestamp_param()),
        ("platformId", platform_id.to_string()),
        (
            "profileVersionArn",
            format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-profiles/Class_Profile/abcde12345"
            ),
        ),
        (
            "jobArn",
            format!(
                "arn:aws:signer:{REGION}:{ACCOUNT_ID}:/signing-jobs/\
                 1a2b3c4d-5e6f-7890-abcd-ef1234567890"
            ),
        ),
        ("certificateHashes", certificate_hash("child", "root")),
    ]
}

/// The whole refusal a value outside the published class takes, as the boundary
/// validator words a constraint violation: the envelope over one clause naming
/// the member path, echoing the rejected value, and quoting the expression it
/// had to satisfy.
fn pattern_refusal(value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at 'platformId' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {PLATFORM_ID_PATTERN}"
    )
}

/// The refusal an in-class id that names no published platform takes.
fn not_found_refusal(value: &str) -> String {
    format!("Signing platform not found for ID {value}")
}

/// The expression the service states is the published one: the refusal quotes it
/// verbatim, so the two copies cannot drift apart silently.
#[test]
fn the_published_expression_is_the_one_the_service_states() {
    assert_eq!(
        REVOCATION_PLATFORM_ID_PATTERN, PLATFORM_ID_PATTERN,
        "case \"the published expression\": expected {PLATFORM_ID_PATTERN:?}, got {:?}",
        REVOCATION_PLATFORM_ID_PATTERN
    );
}

/// The decision the service takes agrees with the expression it quotes, for
/// EVERY character a value can carry rather than for the handful the tables
/// below send: a one-character value is admitted exactly when that character is
/// one `^[a-zA-Z0-9_-]+$` admits, and the empty value is admitted by neither.
///
/// The expected answer is derived from the class the expression spells
/// (`[a-zA-Z0-9_-]`) rather than from the service's own predicate, so the two
/// cannot agree by construction.
#[test]
fn the_decision_agrees_with_the_expression_for_every_character() {
    let admitted = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    // Every ASCII character, plus characters from outside ASCII that no class of
    // the expression names.
    let candidates = (0u8..128)
        .map(char::from)
        .chain(['é', 'π', 'п', '字', '\u{1F600}']);
    for candidate in candidates {
        let value = candidate.to_string();
        let refusal = platform_id_pattern_refusal("platformId", &value);
        assert_eq!(
            refusal.is_none(),
            admitted(candidate),
            "case {value:?}: expected admitted={}, got refusal {refusal:?}",
            admitted(candidate)
        );
    }
    let empty = platform_id_pattern_refusal("platformId", "");
    assert!(
        empty.is_some(),
        "case \"the empty value\": the expression requires at least one character, got {empty:?}"
    );
}

/// A rejected value is stated in FULL — the caller's own bytes, which is what
/// the clause is built from — up to the one ceiling that keeps an oversized
/// query parameter from becoming an equally large error body, past which it is
/// stated as the prefix that fits plus its whole length.
#[tokio::test]
async fn a_rejected_value_is_stated_whole_up_to_its_bound() {
    let router = emulator();

    // The ceiling this service states for a value a clause carries whole.
    const BOUND: usize = 16 * 1024;

    // (case, the value sent)
    let rows: Vec<(&str, String)> = vec![
        ("a long value well inside the bound", format!("{}.", "a".repeat(1_000))),
        // The last size stated whole: the ceiling is inclusive.
        ("exactly at the bound", format!("{}.", "a".repeat(BOUND - 1))),
        ("one character past the bound", format!("{}.", "a".repeat(BOUND))),
        ("far past the bound", format!("{}.", "a".repeat(BOUND * 2))),
    ];

    for (case, value) in &rows {
        let answer = get_revocations(&router, &request_for(value)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        let expected = if value.len() <= BOUND {
            pattern_refusal(value)
        } else {
            pattern_refusal(&format!("{}... [{} bytes]", &value[..BOUND], value.len()))
        };
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected a message of {} bytes, got one of {} bytes",
            expected.len(),
            answer.message().len()
        );
    }
}

/// A `platformId` the published class excludes is refused for that constraint,
/// whatever character puts it outside the class and wherever that character
/// sits — never as a platform that does not exist.
#[tokio::test]
async fn a_platform_id_outside_the_published_class_is_refused_for_the_constraint() {
    let router = emulator();
    let published = published_platform_id();

    // One row per way a value falls outside `^[a-zA-Z0-9_-]+$`: which character
    // is outside the class, where in the value it sits, and how long the value
    // is. The last rows take ids the catalog DOES publish and put one excluded
    // character in them, so the refusal cannot be coming from the catalog.
    let mut values: Vec<(&str, String)> = vec![
        ("a space inside the value", "bad platform".to_string()),
        ("a space and a bang", "bad platform!".to_string()),
        ("a slash", "a/b".to_string()),
        ("a dot", "plat.form".to_string()),
        ("a colon", "platform:1".to_string()),
        ("a plus", "Notation+OCI".to_string()),
        ("a percent", "plat%form".to_string()),
        ("a leading space", " AWSLambda".to_string()),
        ("a trailing bang", "AWSLambda!".to_string()),
        ("the empty value", String::new()),
        ("a single excluded character", "!".to_string()),
        ("a non-ASCII letter", "платформа".to_string()),
        ("two admitted characters either side of a space", "a b".to_string()),
        (
            "a long value with one excluded character",
            format!("{}.", "a".repeat(200)),
        ),
    ];
    for entry in SIGNING_PLATFORM_CATALOG {
        values.push((
            "a published id with a trailing space",
            format!("{} ", entry.platform_id),
        ));
    }
    // Also a published id broken in the MIDDLE, so the value is neither a
    // prefix nor a suffix of one the catalog holds.
    values.push((
        "a published id with an excluded character inside it",
        published.replace('-', " "),
    ));

    for (case, value) in &values {
        let case = format!("{case}: {value:?}");
        let answer = get_revocations(&router, &request_for(value)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.shape().contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {case:?}: expected the modelled code, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            pattern_refusal(value),
            "case {case:?}: expected {:?}, got {:?}",
            pattern_refusal(value),
            answer.message()
        );
        assert!(
            !answer.message().contains("not found for ID"),
            "case {case:?}: expected the constraint refusal, not the catalog's, got {:?}",
            answer.message()
        );
    }
}

/// A `platformId` the class ADMITS that names no published platform keeps the
/// catalog's answer: the id was well formed and is unknown. The two classes stay
/// separate.
#[tokio::test]
async fn an_in_class_platform_id_naming_nothing_published_is_still_unknown() {
    let router = emulator();

    // One row per in-class shape a value can take: each of the four admitted
    // character kinds on its own, the shortest possible value, a long one, and a
    // near-miss of a published id.
    let published = published_platform_id();
    let values: Vec<(&str, String)> = vec![
        ("letters only", "NotARealPlatform".to_string()),
        ("digits only", "123456".to_string()),
        ("an underscore", "Not_A_Real_Platform".to_string()),
        ("a hyphen", "Not-A-Real-Platform".to_string()),
        ("every admitted kind at once", "No_Such-Platform9".to_string()),
        ("one character", "x".to_string()),
        ("a long in-class value", "a".repeat(200)),
        (
            "a published id with one letter changed",
            format!("Z{}", &published[1..]),
        ),
    ];

    for (case, value) in &values {
        let case = format!("{case}: {value:?}");
        let answer = get_revocations(&router, &request_for(value)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.message(),
            not_found_refusal(value),
            "case {case:?}: expected {:?}, got {:?}",
            not_found_refusal(value),
            answer.message()
        );
    }
}

/// Every id the catalog publishes is served: the class admits all of them, so
/// the check added at the boundary refuses none of the requests that were being
/// answered before it.
#[tokio::test]
async fn every_published_platform_id_is_served() {
    let router = emulator();

    for entry in SIGNING_PLATFORM_CATALOG {
        let answer = get_revocations(&router, &request_for(entry.platform_id)).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} {:?}",
            entry.platform_id, answer.status, answer.body
        );
        let reported = answer.body["revokedEntities"].as_array().unwrap_or_else(|| {
            panic!(
                "case {:?}: expected revokedEntities, got {:?}",
                entry.platform_id, answer.body
            )
        });
        assert!(
            reported.is_empty(),
            "case {:?}: nothing named is revoked, got {reported:?}",
            entry.platform_id
        );
    }
}
