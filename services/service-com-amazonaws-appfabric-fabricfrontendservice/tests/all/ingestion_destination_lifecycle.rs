// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppFabric's ingestion-destination operations
//! (`CreateIngestionDestination`, `GetIngestionDestination`,
//! `UpdateIngestionDestination`), driven over the wire (restJson1) through the
//! same router the emulator boots.
//!
//! Each test states a RULE over a TABLE of inputs rather than one recorded call:
//!
//! * an update REPLACES the stored `destinationConfiguration` of the destination
//!   it names, leaves every other member of the record as it was, and what it
//!   stored is what every later read returns;
//! * a destination resolves by either modelled `Identifier` form (ARN or bare
//!   UUID) at every level of the chain;
//! * a WRITE naming an ingestion-chain child that does not exist is refused with
//!   HTTP 400 `ValidationException`, while the READ path answers HTTP 404
//!   `ResourceNotFoundException` for the same identifier;
//! * a value outside a modelled bound is refused with HTTP 400.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is how the router picks
/// the claiming service. No authenticator is registered, so the signature itself
/// is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/appfabric/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region the `AUTHORIZATION` scope above signs for, which is therefore the
/// region every ARN the emulator mints under it carries.
const REGION: &str = "us-east-1";

/// The account segment of every ARN a generated emulator emits.
const ACCOUNT: &str = "000000000000";

/// One emulator: the shared engine plus this service mounted on a router.
struct Emulator {
    router: MultiServiceRouter,
}

/// A response, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`) of an error envelope, or `""`.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The `ingestionDestination` member of a success body.
    fn destination(&self) -> &Value {
        self.body
            .get("ingestionDestination")
            .unwrap_or_else(|| panic!("expected an ingestionDestination in {}", self.body))
    }

    /// One member of the returned `ingestionDestination`.
    fn member(&self, name: &str) -> &Value {
        self.destination()
            .get(name)
            .unwrap_or_else(|| panic!("expected {name} in {}", self.body))
    }
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer { status, body }
    }

    async fn create_app_bundle(&self) -> Answer {
        self.call("POST", "/appbundles", Some(json!({}))).await
    }

    async fn create_ingestion(&self, bundle: &str) -> Answer {
        self.call(
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": "SLACK", "tenantId": "t-1", "ingestionType": "auditLog"})),
        )
        .await
    }

    async fn create_destination(&self, bundle: &str, ingestion: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(body),
        )
        .await
    }

    async fn get_destination(&self, bundle: &str, ingestion: &str, destination: &str) -> Answer {
        self.call("GET", &destination_uri(bundle, ingestion, destination), None)
            .await
    }

    async fn update_destination(
        &self,
        bundle: &str,
        ingestion: &str,
        destination: &str,
        body: Value,
    ) -> Answer {
        self.call(
            "PATCH",
            &destination_uri(bundle, ingestion, destination),
            Some(body),
        )
        .await
    }

    /// A bundle with one ingestion in it, which is the parent chain every
    /// ingestion-destination call needs. Returns `(bundle arn, ingestion arn)`.
    async fn bundle_with_ingestion(&self) -> (String, String) {
        let bundle = arn_of(&self.create_app_bundle().await.body, "appBundle");
        let ingestion = arn_of(&self.create_ingestion(&bundle).await.body, "ingestion");
        (bundle, ingestion)
    }
}

/// The modelled URI of one ingestion destination.
fn destination_uri(bundle: &str, ingestion: &str, destination: &str) -> String {
    format!(
        "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
        encode(bundle),
        encode(ingestion),
        encode(destination)
    )
}

/// Percent-encode an `@httpLabel` the way an AWS SDK does: every byte outside
/// RFC 3986's unreserved set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) becomes
/// `%XX`. An identifier in its ARN form carries `:` and `/`, and `/` would
/// otherwise split a non-greedy label across two path segments.
fn encode(label: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(label.len());
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            // Infallible: writing to a String never fails.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn arn_of(body: &Value, resource: &str) -> String {
    body.get(resource)
        .and_then(|r| r.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected {resource}.arn in {body}"))
        .to_string()
}

/// The bare UUID at the end of an ARN, which is the other identifier form the
/// model's `Identifier` shape accepts for the same resource.
fn uuid_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("an ARN with a resource id, got {arn}"))
        .to_string()
}

/// A UUID-shaped identifier no create call ever minted.
fn unknown_uuid() -> String {
    "ffffffff-ffff-4fff-8fff-ffffffffffff".to_string()
}

/// A fresh client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), built
/// from `seed` so a test can name the same token twice.
fn client_token(seed: u32) -> String {
    format!("{seed:08x}-0000-4000-8000-000000000000")
}

/// An `auditLog` processing configuration. `schema` and `format` come from the
/// model's own closed enums — `Schema = ocsf | raw` and `Format = json |
/// parquet` (`SCHEMA_VALUES` / `FORMAT_VALUES` in `types/shared_types.rs`).
fn processing(schema: &str, format: &str) -> Value {
    json!({"auditLog": {"schema": schema, "format": format}})
}

/// An `auditLog` destination configuration delivering to an S3 bucket.
fn s3_destination(bucket: &str, prefix: Option<&str>) -> Value {
    let mut s3 = json!({"bucketName": bucket});
    if let Some(prefix) = prefix {
        s3["prefix"] = json!(prefix);
    }
    json!({"auditLog": {"destination": {"s3Bucket": s3}}})
}

/// An `auditLog` destination configuration delivering to a Firehose stream.
fn firehose_destination(stream: &str) -> Value {
    json!({"auditLog": {"destination": {"firehoseStream": {"streamName": stream}}}})
}

/// RULE: `UpdateIngestionDestination` REPLACES the stored
/// `destinationConfiguration` of the destination it names, leaves every other
/// member of the record (ARN, `ingestionArn`, `processingConfiguration`,
/// `createdAt`, `status`) as it was, and what it stored is what a later
/// `GetIngestionDestination` returns.
///
/// The table spans what the rule generalises over: the destination union member
/// (`s3Bucket` / `firehoseStream`) before and after the update, the optional
/// `prefix` appearing, changing and disappearing, and both `processingConfiguration`
/// enum members.
#[tokio::test]
async fn an_update_replaces_the_destination_configuration_and_persists_it() {
    let emulator = Emulator::new();

    // (case, processingConfiguration, created destinationConfiguration, updated one)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "s3 without prefix -> s3 with prefix",
            processing("ocsf", "json"),
            s3_destination("audit-logs-bucket", None),
            s3_destination("audit-logs-bucket", Some("slack/")),
        ),
        (
            "s3 prefix changes",
            processing("raw", "json"),
            s3_destination("audit-logs-bucket", Some("slack/")),
            s3_destination("audit-logs-bucket", Some("slack/2026/")),
        ),
        (
            "s3 prefix disappears",
            processing("ocsf", "parquet"),
            s3_destination("audit-logs-bucket", Some("slack/")),
            s3_destination("other-audit-bucket", None),
        ),
        (
            "s3 -> firehose",
            processing("raw", "parquet"),
            s3_destination("audit-logs-bucket", Some("slack/")),
            firehose_destination("audit-log-stream"),
        ),
        (
            "firehose -> s3",
            processing("ocsf", "json"),
            firehose_destination("audit-log-stream"),
            s3_destination("audit-logs-bucket", Some("slack/")),
        ),
        (
            "firehose stream changes",
            processing("ocsf", "json"),
            firehose_destination("audit-log-stream"),
            firehose_destination("other-audit-stream"),
        ),
        (
            "bucket name at the published minimum length of 3",
            processing("raw", "json"),
            s3_destination("abc", None),
            s3_destination("abc", Some("p")),
        ),
        (
            "bucket name at the published maximum length of 63",
            processing("raw", "json"),
            s3_destination(&"b".repeat(63), None),
            s3_destination(&"c".repeat(63), None),
        ),
    ];

    for (case, processing_configuration, created_destination, updated_destination) in cases {
        let (bundle, ingestion) = emulator.bundle_with_ingestion().await;
        let created = emulator
            .create_destination(
                &bundle,
                &ingestion,
                json!({
                    "processingConfiguration": processing_configuration,
                    "destinationConfiguration": created_destination,
                }),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestionDestination, got {} with body {}",
            created.status, created.body
        );
        let arn = arn_of(&created.body, "ingestionDestination");
        let id = uuid_of(&arn);
        assert_eq!(
            arn,
            format!(
                "arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{}/ingestion/{}/ingestiondestination/{id}",
                uuid_of(&bundle),
                uuid_of(&ingestion)
            ),
            "case {case:?}: expected a nested ingestiondestination ARN, got {arn:?}"
        );
        assert_eq!(
            created.member("status"),
            &json!("Active"),
            "case {case:?}: expected a created destination to be Active, got {}",
            created.body
        );

        let updated = emulator
            .update_destination(
                &bundle,
                &ingestion,
                &arn,
                json!({"destinationConfiguration": updated_destination}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateIngestionDestination, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.member("destinationConfiguration"),
            &updated_destination,
            "case {case:?}: expected the updated destinationConfiguration, got {}",
            updated.body
        );
        // Everything the update did NOT name is left as it was.
        for untouched in ["arn", "ingestionArn", "processingConfiguration", "createdAt"] {
            assert_eq!(
                updated.member(untouched),
                created.member(untouched),
                "case {case:?}: expected {untouched} to survive the update unchanged, \
                 created {} vs updated {}",
                created.body,
                updated.body
            );
        }
        assert_eq!(
            updated.member("status"),
            &json!("Active"),
            "case {case:?}: expected an accepted update to leave the destination Active, got {}",
            updated.body
        );

        // What the update stored is what every later read returns, for either
        // identifier form of the destination.
        for identifier in [arn.clone(), id.clone()] {
            let read = emulator
                .get_destination(&bundle, &ingestion, &identifier)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {identifier:?}: expected HTTP 200, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.destination(),
                updated.destination(),
                "case {case:?} read by {identifier:?}: expected the stored record {}, got {}",
                updated.body,
                read.body
            );
        }
    }
}

/// RULE: every level of the chain resolves by either modelled `Identifier` form
/// — an ARN or the bare UUID — so the same update applies whichever form names
/// the bundle, the ingestion and the destination.
#[tokio::test]
async fn an_update_resolves_every_identifier_in_either_form() {
    let emulator = Emulator::new();

    // (case, how to name the bundle, the ingestion, the destination)
    let forms: Vec<(&str, bool, bool, bool)> = vec![
        ("all ARNs", true, true, true),
        ("all bare UUIDs", false, false, false),
        ("bundle by UUID, rest by ARN", false, true, true),
        ("destination by UUID, rest by ARN", true, true, false),
        ("ingestion by UUID, rest by ARN", true, false, true),
    ];

    for (case, bundle_as_arn, ingestion_as_arn, destination_as_arn) in forms {
        let (bundle_arn, ingestion_arn) = emulator.bundle_with_ingestion().await;
        let created = emulator
            .create_destination(
                &bundle_arn,
                &ingestion_arn,
                json!({
                    "processingConfiguration": processing("ocsf", "json"),
                    "destinationConfiguration": s3_destination("audit-logs-bucket", None),
                }),
            )
            .await;
        let destination_arn = arn_of(&created.body, "ingestionDestination");

        let pick = |arn: &str, as_arn: bool| {
            if as_arn {
                arn.to_string()
            } else {
                uuid_of(arn)
            }
        };
        let updated = emulator
            .update_destination(
                &pick(&bundle_arn, bundle_as_arn),
                &pick(&ingestion_arn, ingestion_as_arn),
                &pick(&destination_arn, destination_as_arn),
                json!({"destinationConfiguration":
                    s3_destination("audit-logs-bucket", Some("slack/"))}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.member("destinationConfiguration"),
            &s3_destination("audit-logs-bucket", Some("slack/")),
            "case {case:?}: expected the updated configuration, got {}",
            updated.body
        );
        assert_eq!(
            updated.member("arn"),
            &json!(destination_arn),
            "case {case:?}: expected the update to hit the created destination, got {}",
            updated.body
        );
    }
}

/// RULE: a WRITE on the ingestion-destination chain that names an ingestion or
/// an ingestion destination which does not exist is refused with HTTP 400
/// `ValidationException`, while the REPORTING path answers HTTP 404
/// `ResourceNotFoundException` for the same identifier. An app bundle that does
/// not exist is HTTP 404 on either path.
#[tokio::test]
async fn a_missing_chain_resource_is_a_validation_failure_on_writes_and_not_found_on_reads() {
    let emulator = Emulator::new();
    let (bundle, ingestion) = emulator.bundle_with_ingestion().await;
    let destination = arn_of(
        &emulator
            .create_destination(
                &bundle,
                &ingestion,
                json!({
                    "processingConfiguration": processing("ocsf", "json"),
                    "destinationConfiguration": s3_destination("audit-logs-bucket", None),
                }),
            )
            .await
            .body,
        "ingestionDestination",
    );
    let unknown = unknown_uuid();
    let unknown_arn = format!("arn:aws:appfabric:{REGION}:{ACCOUNT}:appbundle/{unknown}");
    let configuration = json!({"destinationConfiguration": s3_destination("audit-logs-bucket", None)});
    let create_body = json!({
        "processingConfiguration": processing("ocsf", "json"),
        "destinationConfiguration": s3_destination("audit-logs-bucket", None),
    });

    // (case, expected status, expected error code, answer)
    let cases = vec![
        (
            "update an unknown destination by uuid",
            400,
            "ValidationException",
            emulator
                .update_destination(&bundle, &ingestion, &unknown, configuration.clone())
                .await,
        ),
        (
            "update a destination under an unknown ingestion",
            400,
            "ValidationException",
            emulator
                .update_destination(&bundle, &unknown, &destination, configuration.clone())
                .await,
        ),
        (
            "create a destination under an unknown ingestion",
            400,
            "ValidationException",
            emulator
                .create_destination(&bundle, &unknown, create_body.clone())
                .await,
        ),
        (
            "update a destination under an unknown bundle",
            404,
            "ResourceNotFoundException",
            emulator
                .update_destination(&unknown, &ingestion, &destination, configuration.clone())
                .await,
        ),
        (
            "create a destination under an unknown bundle arn",
            404,
            "ResourceNotFoundException",
            emulator
                .create_destination(&unknown_arn, &ingestion, create_body.clone())
                .await,
        ),
        (
            "read an unknown destination",
            404,
            "ResourceNotFoundException",
            emulator.get_destination(&bundle, &ingestion, &unknown).await,
        ),
        (
            "read a destination under an unknown ingestion",
            404,
            "ResourceNotFoundException",
            emulator
                .get_destination(&bundle, &unknown, &destination)
                .await,
        ),
        (
            "read a destination under an unknown bundle",
            404,
            "ResourceNotFoundException",
            emulator
                .get_destination(&unknown, &ingestion, &destination)
                .await,
        ),
    ];

    for (case, status, code, answer) in cases {
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {}",
            answer.body
        );
    }

    // A refused write changes nothing: the destination still carries what the
    // create stored.
    let read = emulator
        .get_destination(&bundle, &ingestion, &destination)
        .await;
    assert_eq!(
        read.member("destinationConfiguration"),
        &s3_destination("audit-logs-bucket", None),
        "a refused write must leave the stored configuration alone, got {}",
        read.body
    );
}

/// RULE: a value outside a modelled bound is refused with HTTP 400 and nothing
/// is stored. The bounds are the model's own: `S3Bucket$bucketName` is
/// `@length(min: 3, max: 63)`, `S3Bucket$prefix` is `@length(min: 1, max: 120)`
/// and `FirehoseStream$streamName` is `@length(min: 3, max: 64)` — each tested
/// one byte past both of its published bounds.
#[tokio::test]
async fn a_configuration_outside_a_modelled_bound_is_refused() {
    let emulator = Emulator::new();
    let (bundle, ingestion) = emulator.bundle_with_ingestion().await;
    let created = emulator
        .create_destination(
            &bundle,
            &ingestion,
            json!({
                "processingConfiguration": processing("ocsf", "json"),
                "destinationConfiguration": s3_destination("audit-logs-bucket", None),
            }),
        )
        .await;
    let destination = arn_of(&created.body, "ingestionDestination");

    // (case, destinationConfiguration outside a bound)
    let cases: Vec<(&str, Value)> = vec![
        ("bucket name one byte under the minimum of 3", s3_destination("ab", None)),
        (
            "bucket name one byte over the maximum of 63",
            s3_destination(&"b".repeat(64), None),
        ),
        (
            "prefix one byte under the minimum of 1",
            s3_destination("audit-logs-bucket", Some("")),
        ),
        (
            "prefix one byte over the maximum of 120",
            s3_destination("audit-logs-bucket", Some(&"p".repeat(121))),
        ),
        ("stream name one byte under the minimum of 3", firehose_destination("ab")),
        (
            "stream name one byte over the maximum of 64",
            firehose_destination(&"s".repeat(65)),
        ),
    ];

    for (case, configuration) in cases {
        let refused = emulator
            .update_destination(
                &bundle,
                &ingestion,
                &destination,
                json!({"destinationConfiguration": configuration}),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        let read = emulator
            .get_destination(&bundle, &ingestion, &destination)
            .await;
        assert_eq!(
            read.destination(),
            created.destination(),
            "case {case:?}: a refused update must store nothing, expected {}, got {}",
            created.body,
            read.body
        );
    }
}

/// RULE: `clientToken` is an `@idempotencyToken`, so replaying it replays the
/// destination the first call created instead of minting a second one, and
/// replaying it with DIFFERENT parameters is refused with HTTP 409
/// `ConflictException` (the `IdempotentParameterMismatch` case). The table spans
/// each parameter the fingerprint covers.
#[tokio::test]
async fn a_replayed_client_token_replays_the_created_destination() {
    let emulator = Emulator::new();
    let (bundle, ingestion) = emulator.bundle_with_ingestion().await;
    let token = client_token(1);
    let first = emulator
        .create_destination(
            &bundle,
            &ingestion,
            json!({
                "clientToken": token,
                "processingConfiguration": processing("ocsf", "json"),
                "destinationConfiguration": s3_destination("audit-logs-bucket", None),
                "tags": [{"key": "env", "value": "prod"}],
            }),
        )
        .await;
    assert_eq!(
        first.status, 201,
        "expected HTTP 201, got {} with body {}",
        first.status, first.body
    );

    let replay = emulator
        .create_destination(
            &bundle,
            &ingestion,
            json!({
                "clientToken": token,
                "processingConfiguration": processing("ocsf", "json"),
                "destinationConfiguration": s3_destination("audit-logs-bucket", None),
                "tags": [{"key": "env", "value": "prod"}],
            }),
        )
        .await;
    assert_eq!(
        replay.destination(),
        first.destination(),
        "a replayed token must replay the first destination, expected {}, got {}",
        first.body,
        replay.body
    );

    // (case, the one parameter that differs from the first call)
    let mismatches: Vec<(&str, Value)> = vec![
        (
            "a different destination",
            json!({
                "clientToken": token,
                "processingConfiguration": processing("ocsf", "json"),
                "destinationConfiguration": s3_destination("other-audit-bucket", None),
                "tags": [{"key": "env", "value": "prod"}],
            }),
        ),
        (
            "a different processing configuration",
            json!({
                "clientToken": token,
                "processingConfiguration": processing("raw", "parquet"),
                "destinationConfiguration": s3_destination("audit-logs-bucket", None),
                "tags": [{"key": "env", "value": "prod"}],
            }),
        ),
        (
            "different tags",
            json!({
                "clientToken": token,
                "processingConfiguration": processing("ocsf", "json"),
                "destinationConfiguration": s3_destination("audit-logs-bucket", None),
                "tags": [{"key": "env", "value": "dev"}],
            }),
        ),
    ];

    for (case, body) in mismatches {
        let refused = emulator.create_destination(&bundle, &ingestion, body).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            refused.body
        );
    }
}
