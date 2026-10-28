// SPDX-License-Identifier: CC0-1.0

//! The CMK an encrypted stream names, resolved against KMS.
//!
//! The rule under test: Firehose does not take a `KeyARN` on trust. A stream it reports as
//! encrypted is one whose records it can actually wrap, so the key is resolved before the
//! configuration is recorded — and a key KMS does not serve, because it holds no such key
//! or because the key's state does not admit a cryptographic operation, is refused with the
//! modelled `InvalidKMSResourceException` (HTTP 400) whose `Code` relays the KMS exception
//! type. A key KMS does serve is accepted, whichever call named it.
//!
//! Firehose and KMS are mounted on ONE shared engine, exactly as the router mounts them, so
//! the keys these cases name are real keys of the KMS service beside it: the enabled one is
//! minted with `CreateKey` and turned off with `DisableKey`, and the absent one is an ARN of
//! the same account and Region that KMS holds nothing for.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{s3_configuration, PAST_THE_TRANSITION, REGION};

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// Firehose and KMS on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One `awsJson1_1` call: `target` is the `X-Amz-Target` value and `service` the
    /// signing name the router claims the request by.
    async fn call(&self, service: &str, target: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn firehose(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call(
            "firehose",
            &format!("Firehose_20150804.{operation}"),
            body,
        )
        .await
    }

    async fn kms(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call("kms", &format!("TrentService.{operation}"), body)
            .await
    }

    /// Mint a customer managed key and answer its ARN.
    async fn create_key(&self, description: &str) -> String {
        let (status, created) = self
            .kms("CreateKey", json!({ "Description": description }))
            .await;
        assert_eq!(status, 200, "create key {description:?}: got {created:?}");
        let arn = created["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with("arn:"),
            "create key {description:?}: expected a key ARN, got {created:?}"
        );
        arn
    }
}

/// The wire error code of an error response: `__type` without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("no __type in {body:?}"))
}

/// A `CUSTOMER_MANAGED_CMK` encryption input naming `key_arn`.
fn encryption_input(key_arn: &str) -> Value {
    json!({ "KeyType": "CUSTOMER_MANAGED_CMK", "KeyARN": key_arn })
}

#[tokio::test]
async fn a_cmk_kms_cannot_serve_is_refused_and_one_it_serves_is_accepted() {
    let emulator = Emulator::new();

    let usable = emulator.create_key("a key Firehose may use").await;
    let turned_off = emulator.create_key("a key its owner turned off").await;
    let (status, disabled) = emulator
        .kms("DisableKey", json!({ "KeyId": turned_off.clone() }))
        .await;
    assert_eq!(status, 200, "disable the key: got {disabled:?}");
    // An ARN of this account and Region that KMS holds no key for. It is well-formed, so
    // the refusal is KMS's answer about the key and not the model's about the value.
    let absent = format!("arn:aws:kms:{REGION}:000000000000:key/{}", "0".repeat(32));

    // (case, the key the create names, the KMS exception type the refusal must relay)
    let refused: Vec<(&str, &str, &str)> = vec![
        (
            "a key KMS holds nothing for",
            absent.as_str(),
            "NotFoundException",
        ),
        (
            "a key its owner turned off",
            turned_off.as_str(),
            "DisabledException",
        ),
    ];

    for (case, key_arn, relayed) in &refused {
        let (status, answered) = emulator
            .firehose(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": format!("kms-create-{}", key_arn.len()),
                    "S3DestinationConfiguration": s3_configuration(),
                    "DeliveryStreamEncryptionConfigurationInput": encryption_input(key_arn),
                }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {answered:?}");
        assert_eq!(
            error_code(&answered),
            "InvalidKMSResourceException",
            "case {case:?}: got {answered:?}"
        );
        assert_eq!(
            answered["code"],
            json!(*relayed),
            "case {case:?}: expected the relayed KMS exception type {relayed:?}, got \
             {answered:?}"
        );
    }

    // The key KMS serves is accepted, and the stream reports it.
    let (status, created) = emulator
        .firehose(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "kms-create-usable",
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": encryption_input(&usable),
            }),
        )
        .await;
    assert_eq!(status, 200, "a usable key: got {created:?}");
    let (status, described) = emulator
        .firehose(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "kms-create-usable" }),
        )
        .await;
    assert_eq!(status, 200, "describe: got {described:?}");
    assert_eq!(
        described["DeliveryStreamDescription"]["DeliveryStreamEncryptionConfiguration"]
            ["KeyARN"],
        json!(usable),
        "the stream reports the key it was created under, got {described:?}"
    );
}

#[tokio::test]
async fn starting_encryption_resolves_the_cmk_the_request_names() {
    let emulator = Emulator::new();

    let usable = emulator.create_key("a key Firehose may use").await;
    let turned_off = emulator.create_key("a key its owner turned off").await;
    let (status, disabled) = emulator
        .kms("DisableKey", json!({ "KeyId": turned_off.clone() }))
        .await;
    assert_eq!(status, 200, "disable the key: got {disabled:?}");
    let absent = format!("arn:aws:kms:{REGION}:000000000000:key/{}", "1".repeat(32));

    for (index, name) in ["sse-absent-key", "sse-disabled-key", "sse-usable-key"]
        .iter()
        .enumerate()
    {
        let (status, created) = emulator
            .firehose(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": name,
                    "S3DestinationConfiguration": s3_configuration(),
                }),
            )
            .await;
        assert_eq!(status, 200, "create {name} (row {index}): got {created:?}");
    }
    // Server-side encryption can only be changed on an `ACTIVE` stream.
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // (case, stream, the key the call names, the KMS exception type relayed, or none when
    // the call must succeed)
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "a key KMS holds nothing for",
            "sse-absent-key",
            absent.as_str(),
            Some("NotFoundException"),
        ),
        (
            "a key its owner turned off",
            "sse-disabled-key",
            turned_off.as_str(),
            Some("DisabledException"),
        ),
        (
            "a key KMS serves",
            "sse-usable-key",
            usable.as_str(),
            None,
        ),
    ];

    for (case, stream, key_arn, relayed) in rows {
        let (status, answered) = emulator
            .firehose(
                "StartDeliveryStreamEncryption",
                json!({
                    "DeliveryStreamName": stream,
                    "DeliveryStreamEncryptionConfigurationInput": encryption_input(key_arn),
                }),
            )
            .await;
        match relayed {
            Some(relayed) => {
                assert_eq!(status, 400, "case {case:?}: got {answered:?}");
                assert_eq!(
                    error_code(&answered),
                    "InvalidKMSResourceException",
                    "case {case:?}: got {answered:?}"
                );
                assert_eq!(
                    answered["code"],
                    json!(relayed),
                    "case {case:?}: expected the relayed KMS exception type \
                     {relayed:?}, got {answered:?}"
                );
                // A refused change leaves the stream's SSE state alone.
                let (status, described) = emulator
                    .firehose(
                        "DescribeDeliveryStream",
                        json!({ "DeliveryStreamName": stream }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: describe got {described:?}");
                assert_eq!(
                    described["DeliveryStreamDescription"]
                        ["DeliveryStreamEncryptionConfiguration"]["Status"],
                    json!("DISABLED"),
                    "case {case:?}: the refused change left SSE off, got {described:?}"
                );
            }
            None => {
                assert_eq!(status, 200, "case {case:?}: got {answered:?}");
                let (status, described) = emulator
                    .firehose(
                        "DescribeDeliveryStream",
                        json!({ "DeliveryStreamName": stream }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: describe got {described:?}");
                assert_eq!(
                    described["DeliveryStreamDescription"]
                        ["DeliveryStreamEncryptionConfiguration"]["KeyARN"],
                    json!(key_arn),
                    "case {case:?}: the stream records the key, got {described:?}"
                );
            }
        }
    }
}
