// SPDX-License-Identifier: CC0-1.0

//! How `agent-registry-control` WORDS the failures of its registry and registry-record
//! operations, driven over HTTP through the real router so every assertion goes through
//! routing, deserialization, the handler (or the boundary validator) and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than for one
//! recorded request:
//!
//! * a resource an identifier named nothing for is stated as one capitalised sentence
//!   that ends in a period and names the resource class, the kind of identifier it was
//!   addressed by and the identifier exactly as the request supplied it — and nothing
//!   else, so the sentence never carries an identifier the request did not address the
//!   resource by;
//! * a name a create asked for that a live resource of the same class already holds in
//!   the same scope is stated with the requested name in SINGLE QUOTES, so a client reads
//!   the conflicting name back out of the sentence;
//! * a value outside its member's `@pattern` is refused with the expression that member
//!   admits, quoted without the whole-value anchors the model wraps it in, and with no
//!   value quoted at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`). The signature itself is never checked: the test router runs with
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// Longer than the service's own asynchronous-transition span, so a registry a test
/// created is `READY` by the time its records are touched.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The expression `RegistryName` and `RegistryRecordName` both admit, as the MODEL writes
/// it — `smithy_query(query="shape", shape="RegistryName,RegistryRecordName")`:
/// `^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$` (`RegistryName` @length 1..64,
/// `RegistryRecordName` @length 1..255).
const NAME_PATTERN: &str = r"^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$";

/// The same expression without the pair of whole-value anchors, which is how a refusal
/// quotes it back.
const NAME_PATTERN_UNANCHORED: &str = r"[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*";

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The modelled `message` member of the error envelope.
    fn message(&self) -> &str {
        self.str_field("message")
    }
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// Percent-encode a path segment the way an SDK encodes a non-greedy `@httpLabel`, so an
/// ARN (which carries `:` and `/`) can be given where the model allows the ARN form.
fn encode_label(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn id_of(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// A `descriptors` value, whose member names are `Descriptors`' own
/// (`smithy_query(query="shape", shape="Descriptors")`).
fn descriptors() -> Value {
    json!({ "mcpServer": { "data": "{\"tools\":[]}", "dataSchemaVersion": "2025-06-18" } })
}

fn create_record_body(name: &str) -> Value {
    json!({ "name": name, "recordType": "MCP", "descriptors": descriptors() })
}

/// Create a registry and wait out the creation the clock owes, so it is `READY`.
/// Returns `(registryId, registryArn)`.
async fn ready_registry(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = call(router, "POST", "/registries", Some(json!({ "name": name }))).await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateRegistry answers 202, got {} {:?}",
        created.status, created.body
    );
    let registry_arn = created.str_field("registryArn").to_string();
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    (id_of(&registry_arn).to_string(), registry_arn)
}

/// Create a record in a `READY` registry and wait out the creation the clock owes.
/// Returns its record id.
async fn draft_record(router: &MultiServiceRouter, registry_id: &str, name: &str) -> String {
    let created = call(
        router,
        "POST",
        &format!("/registries/{}/records", encode_label(registry_id)),
        Some(create_record_body(name)),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateRegistryRecord answers 202, got {} {:?}",
        created.status, created.body
    );
    let record_arn = created.str_field("recordArn").to_string();
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    id_of(&record_arn).to_string()
}

/// Every operation that resolves a `registryId` label, as (operation, method, path, body)
/// of a request against the identifier given.
fn registry_addressed_calls(identifier: &str, record_id: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let label = encode_label(identifier);
    let record_label = encode_label(record_id);
    vec![
        (
            "GetRegistry",
            "GET",
            format!("/registries/{label}"),
            None,
        ),
        (
            "DeleteRegistry",
            "DELETE",
            format!("/registries/{label}"),
            None,
        ),
        (
            "ListRegistryRecords",
            "POST",
            format!("/registries/{label}/records-list"),
            Some(json!({})),
        ),
        (
            "CreateRegistryRecord",
            "POST",
            format!("/registries/{label}/records"),
            Some(create_record_body("probe-record")),
        ),
        (
            "GetRegistryRecord",
            "GET",
            format!("/registries/{label}/records/{record_label}"),
            None,
        ),
        (
            "DeleteRegistryRecord",
            "DELETE",
            format!("/registries/{label}/records/{record_label}"),
            None,
        ),
        (
            "SubmitRegistryRecordForApproval",
            "POST",
            format!("/registries/{label}/records/{record_label}/submit-for-approval"),
            Some(json!({})),
        ),
    ]
}

/// Every operation that resolves a `recordId` label inside a registry that DOES exist.
fn record_addressed_calls(registry_id: &str, identifier: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let label = encode_label(registry_id);
    let record_label = encode_label(identifier);
    vec![
        (
            "GetRegistryRecord",
            "GET",
            format!("/registries/{label}/records/{record_label}"),
            None,
        ),
        (
            "DeleteRegistryRecord",
            "DELETE",
            format!("/registries/{label}/records/{record_label}"),
            None,
        ),
        (
            "SubmitRegistryRecordForApproval",
            "POST",
            format!("/registries/{label}/records/{record_label}/submit-for-approval"),
            Some(json!({})),
        ),
    ]
}

#[tokio::test]
async fn a_registry_an_identifier_names_nothing_for_is_stated_in_one_sentence() {
    let router = router();
    // Rows differ along the FORM of the identifier the label admits
    // (`RegistryIdentifier`: a bare id of 12..16 alphanumerics, or the resource's ARN in
    // any partition the shape's `aws(-[^:]+)?` admits), since the sentence states back
    // what the request sent.
    let identifiers: Vec<(&str, String)> = vec![
        ("a bare twelve-character id", "zzzzzzzzzzzz".to_string()),
        (
            "a bare sixteen-character id",
            "zzzz9999zzzz9999".to_string(),
        ),
        (
            "the ARN form",
            format!(
                "arn:aws:agent-registry:{REGION}:{}:registry/zzzzzzzzzzzz",
                engine::emulator_account_id()
            ),
        ),
        (
            "the ARN form in another partition",
            "arn:aws-cn:agent-registry:cn-north-1:000000000000:registry/zzzzzzzzzzzz".to_string(),
        ),
    ];

    for (case, identifier) in identifiers {
        let expected = format!("Registry with ARN {identifier} not found.");
        for (operation, method, path, body) in
            registry_addressed_calls(&identifier, "aaaa11112222")
        {
            let reply = call(&router, method, &path, body).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}/{operation}: an absent registry is the modelled 404, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}/{operation}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_record_an_identifier_names_nothing_for_is_stated_in_one_sentence() {
    let router = router();
    let (registry_id, registry_arn) = ready_registry(&router, "wording-registry").await;
    // A record that WAS there and was deleted, and one that never was: both are the same
    // absence, so both are stated the same way.
    let deleted = draft_record(&router, &registry_id, "doomed-record").await;
    let removed = call(
        &router,
        "DELETE",
        &format!(
            "/registries/{}/records/{}",
            encode_label(&registry_id),
            encode_label(&deleted)
        ),
        None,
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "deleting a record answers 2xx, got {} {:?}",
        removed.status, removed.body
    );

    // Rows differ along the FORM of the `recordId` label (`RecordIdentifier`: a bare
    // 12-character id or the record's ARN) and along whether the record ever existed.
    let identifiers: Vec<(&str, String, String)> = vec![
        (
            "a deleted record by its bare id",
            deleted.clone(),
            deleted.clone(),
        ),
        (
            "a deleted record by its ARN form",
            format!("{registry_arn}/record/{deleted}"),
            deleted.clone(),
        ),
        (
            "a record that never existed",
            "aaaa11112222".to_string(),
            "aaaa11112222".to_string(),
        ),
        (
            "a record that never existed, by its ARN form",
            format!("{registry_arn}/record/bbbb33334444"),
            "bbbb33334444".to_string(),
        ),
    ];

    for (case, identifier, stated_id) in identifiers {
        // The sentence states the record's ID — what it says it states — which is what a
        // `RecordIdentifier` carries in either form, and it names the record alone: the
        // registry that was searched is not part of it.
        let expected = format!("Registry record with ID {stated_id} not found.");
        for (operation, method, path, body) in record_addressed_calls(&registry_id, &identifier) {
            let reply = call(&router, method, &path, body).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}/{operation}: an absent record is the modelled 404, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}/{operation}: expected {expected:?}, got {:?}",
                reply.message()
            );
            assert!(
                !reply.message().contains(&registry_id),
                "case {case:?}/{operation}: the sentence states no registry identifier, \
                 got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_duplicate_record_name_states_the_requested_name_in_single_quotes() {
    let router = router();
    let (registry_id, registry_arn) = ready_registry(&router, "conflict-registry").await;

    // Rows differ along the SHAPE and SIZE of the name (`RegistryRecordName` @length
    // 1..255 over the pattern above: a single character, punctuation the pattern admits,
    // and the 255-character bound) and along the form the registry was addressed by,
    // since the sentence states that identifier back as the request sent it.
    let names: Vec<(&str, String)> = vec![
        ("a one-character name", "a".to_string()),
        ("a name with the punctuation the pattern admits", "team.records/alpha-1_x".to_string()),
        ("a name at the model's 255-character bound", "n".repeat(255)),
    ];
    let registry_labels: Vec<(&str, &str)> = vec![
        ("the bare registry id", registry_id.as_str()),
        ("the registry ARN form", registry_arn.as_str()),
    ];

    for (name_case, name) in names {
        let first = call(
            &router,
            "POST",
            &format!("/registries/{}/records", encode_label(&registry_id)),
            Some(create_record_body(&name)),
        )
        .await;
        assert_eq!(
            first.status, 202,
            "case {name_case:?}: the first create answers 202, got {} {:?}",
            first.status, first.body
        );

        for (label_case, label) in &registry_labels {
            let expected = format!("A record with name '{name}' already exists in registry {label}");
            let again = call(
                &router,
                "POST",
                &format!("/registries/{}/records", encode_label(label)),
                Some(create_record_body(&name)),
            )
            .await;
            assert_eq!(
                again.status, 409,
                "case {name_case:?}/{label_case:?}: a taken name is the modelled 409, got {} {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.error_code.as_deref(),
                Some("ConflictException"),
                "case {name_case:?}/{label_case:?}: expected ConflictException, got {:?}",
                again.error_code
            );
            assert_eq!(
                again.message(),
                expected,
                "case {name_case:?}/{label_case:?}: expected {expected:?}, got {:?}",
                again.message()
            );
            assert!(
                again.message().contains(&format!("'{name}'")),
                "case {name_case:?}/{label_case:?}: the conflicting name is single-quoted, \
                 got {:?}",
                again.message()
            );
        }
    }
}

#[tokio::test]
async fn a_duplicate_registry_name_states_the_requested_name_in_single_quotes() {
    let router = router();
    // Rows differ along the shape and size of the name (`RegistryName` @length 1..64).
    let names: Vec<(&str, String)> = vec![
        ("a one-character name", "x".to_string()),
        ("a dotted and slashed name", "org.team/registry-2_a".to_string()),
        ("a name at the model's 64-character bound", "r".repeat(64)),
    ];

    for (case, name) in names {
        let first = call(&router, "POST", "/registries", Some(json!({ "name": name }))).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first create answers 202, got {} {:?}",
            first.status, first.body
        );
        let again = call(&router, "POST", "/registries", Some(json!({ "name": name }))).await;
        let expected = format!("A registry with name '{name}' already exists in this account");
        assert_eq!(
            again.status, 409,
            "case {case:?}: a taken name is the modelled 409, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            again.error_code
        );
        assert_eq!(
            again.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            again.message()
        );
    }
}

#[tokio::test]
async fn a_pattern_violation_states_the_unanchored_expression_and_quotes_no_value() {
    let router = router();
    let (registry_id, _) = ready_registry(&router, "validation-registry").await;

    // Values outside the pattern `RegistryName` and `RegistryRecordName` share: a space
    // and punctuation the pattern does not admit, a leading character outside its first
    // class, and a character outside the trailing class.
    let bad_names: [(&str, &str); 4] = [
        ("a space and a bang", "sim2real probe bad name!"),
        ("a leading underscore", "_leading-underscore"),
        ("a leading dot", ".leading-dot"),
        ("a character outside the trailing class", "name with:colon"),
    ];
    let expected = format!(
        "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {NAME_PATTERN_UNANCHORED}"
    );

    for (case, bad_name) in bad_names {
        for (operation, path, body) in [
            (
                "CreateRegistry",
                "/registries".to_string(),
                json!({ "name": bad_name }),
            ),
            (
                "CreateRegistryRecord",
                format!("/registries/{}/records", encode_label(&registry_id)),
                create_record_body(bad_name),
            ),
        ] {
            let reply = call(&router, "POST", &path, Some(body)).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}/{operation}: a value outside the pattern is the modelled 400, \
                 got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}/{operation}: expected ValidationException, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}/{operation}: expected {expected:?}, got {:?}",
                reply.message()
            );
            assert!(
                !reply.message().contains(bad_name)
                    && !reply.message().contains(NAME_PATTERN)
                    && !reply.message().ends_with('$'),
                "case {case:?}/{operation}: neither the refused value nor an anchor may reach \
                 the sentence, got {:?}",
                reply.message()
            );
        }
    }
}
