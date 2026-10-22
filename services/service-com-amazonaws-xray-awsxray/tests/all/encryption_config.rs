// SPDX-License-Identifier: CC0-1.0

//! The X-Ray encryption configuration, end to end over the wire.
//!
//! Every case drives the SAME front door the production router does: a
//! `MultiServiceRouter` with this service mounted, addressed by the `xray`
//! endpoint host, speaking restJson1. So the assertions below are about HTTP
//! responses, not about internal state.
//!
//! The rules under test, each checked over a SET of inputs rather than one:
//!
//! * `GetEncryptionConfig` reports the configuration the service holds — X-Ray's
//!   default (`Type` `NONE`, `Status` `ACTIVE`, no `KeyId`) until one is put —
//!   and reading it never changes what a later caller sees;
//! * `PutEncryptionConfig` replaces the whole document, so the next read answers
//!   with the `Type` and `KeyId` it stored, whatever form the key selector took;
//! * a stored configuration reports `Status` `UPDATING` until the clock makes it
//!   `ACTIVE` ([`state::ENCRYPTION_SETTLE_AFTER`]), and that instant does not
//!   depend on whether anybody read;
//! * a request whose `KeyId` does not select a KMS key in one of the three forms
//!   `PutEncryptionConfig$KeyId` documents, one that sets `Type` `KMS` without a
//!   key, and one that sets `Type` `NONE` with a key, are each refused with
//!   `InvalidRequestException` and store nothing.

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_xray_awsxray::state;
use smithy_server_runtime::MultiServiceRouter;

/// A margin on top of the settle span, for the clock the handler reads.
const SETTLE_MARGIN: Duration = Duration::from_millis(150);

/// Long enough that the `UPDATING` -> `ACTIVE` transition is due.
fn past_settle() -> Duration {
    state::ENCRYPTION_SETTLE_AFTER + SETTLE_MARGIN
}

/// A key id in the form KMS mints: 8-4-4-4-12 hexadecimal digits.
const KEY_ID: &str = "ae4aa6d4-9a4d-489d-9a47-54ff6d789845";

/// One emulator with an empty store.
struct Xray {
    router: MultiServiceRouter,
}

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// restJson1 resolves an error shape from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    /// A member of the `EncryptionConfig` document, or `Value::Null`.
    fn config(&self, member: &str) -> &Value {
        self.body
            .get("EncryptionConfig")
            .and_then(|config| config.get(member))
            .unwrap_or(&Value::Null)
    }
}

impl Xray {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
            engine,
        ));
        // These tests exercise handlers, not SigV4; the signing name still has to
        // route, which the `Host` header below does.
        router.set_request_authentication(false);
        Self { router }
    }

    async fn call(&self, path: &str, body: &Value) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://xray.us-east-1.amazonaws.com{path}"))
            .header("host", "xray.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// `PutEncryptionConfig`, with `KeyId` present only when `key_id` is.
    async fn put(&self, encryption_type: &str, key_id: Option<&str>) -> Answer {
        let mut body = json!({ "Type": encryption_type });
        if let Some(key_id) = key_id {
            body["KeyId"] = json!(key_id);
        }
        self.call("/PutEncryptionConfig", &body).await
    }

    /// `GetEncryptionConfig`, whose request models no members.
    async fn get(&self) -> Answer {
        self.call("/EncryptionConfig", &json!({})).await
    }
}

/// Assert one answer carries exactly the `EncryptionConfig` document expected.
fn assert_config(
    case: &str,
    answer: &Answer,
    encryption_type: &str,
    status: &str,
    key_id: Option<&str>,
) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.config("Type"),
        &json!(encryption_type),
        "case {case:?}: expected Type {encryption_type:?}, got {:?}",
        answer.config("Type")
    );
    assert_eq!(
        answer.config("Status"),
        &json!(status),
        "case {case:?}: expected Status {status:?}, got {:?}",
        answer.config("Status")
    );
    let expected_key = key_id.map_or(Value::Null, |key_id| json!(key_id));
    assert_eq!(
        answer.config("KeyId"),
        &expected_key,
        "case {case:?}: expected KeyId {expected_key:?}, got {:?}",
        answer.config("KeyId")
    );
}

#[tokio::test]
async fn an_unconfigured_service_reports_default_encryption_and_reading_changes_nothing() {
    let xray = Xray::new();

    // Read it three times: a reporting operation answers the same document every
    // time, and leaves the next caller the same answer.
    for read in 1..=3 {
        let answer = xray.get().await;
        assert_config(
            &format!("read {read} of an unconfigured service"),
            &answer,
            "NONE",
            "ACTIVE",
            None,
        );
    }

    // And the clock owes an unconfigured service nothing: no span is open, so
    // waiting past one changes no member.
    sleep(past_settle());
    let answer = xray.get().await;
    assert_config(
        "after a span's worth of waiting",
        &answer,
        "NONE",
        "ACTIVE",
        None,
    );
}

#[tokio::test]
async fn a_stored_configuration_round_trips_whatever_form_its_key_selector_takes() {
    // The three selector forms come from `PutEncryptionConfig$KeyId`
    // (`smithy_query(query="operation",operation="PutEncryptionConfig")`): an
    // alias (`alias/MyKey`), a key id, or the full ARN of either. The rows differ
    // along the form, the characters in an alias name, the digit case of a key
    // id, the partition and region of an ARN, and the absence of a key under
    // `NONE`.
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("default encryption", "NONE", None),
        ("an alias", "KMS", Some("alias/MyKey")),
        (
            "an alias using every character an alias name allows",
            "KMS",
            Some("alias/team:x-ray_key/9"),
        ),
        ("a key id", "KMS", Some(KEY_ID)),
        (
            "the key id the model's own `KeyId` documentation gives as its example",
            "KMS",
            Some("ae4aa6d49-a4d8-9df9-a475-4ff6d7898456"),
        ),
        (
            "the id of a multi-region key",
            "KMS",
            Some("mrk-1234abcd12ab34cd56ef1234567890ab"),
        ),
        (
            "a key id in upper-case hex",
            "KMS",
            Some("AE4AA6D4-9A4D-489D-9A47-54FF6D789845"),
        ),
        (
            "the full ARN of a key",
            "KMS",
            Some("arn:aws:kms:us-east-2:123456789012:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        (
            "the full ARN of an alias in another partition and region",
            "KMS",
            Some("arn:aws-us-gov:kms:us-gov-west-1:210987654321:alias/MyKey"),
        ),
    ];

    for (case, encryption_type, key_id) in cases {
        let xray = Xray::new();

        // The put answers the document it stored, still being applied.
        let put = xray.put(encryption_type, *key_id).await;
        assert_config(case, &put, encryption_type, "UPDATING", *key_id);

        // A read taken straight after it sees the same transient status: the span
        // is never zero.
        let during = xray.get().await;
        assert_config(
            &format!("{case}, read while it is applied"),
            &during,
            encryption_type,
            "UPDATING",
            *key_id,
        );

        // Once the clock owes the transition, the SAME stored document is active —
        // the earlier read neither accelerated nor froze it.
        sleep(past_settle());
        let settled = xray.get().await;
        assert_config(
            &format!("{case}, read once the transition is due"),
            &settled,
            encryption_type,
            "ACTIVE",
            *key_id,
        );
    }
}

#[tokio::test]
async fn each_put_supersedes_the_whole_stored_document() {
    let xray = Xray::new();

    // A sequence of puts, each replacing the last: the reader always reports the
    // newest document, and a key is dropped when the type that needs it is.
    let sequence: &[(&str, &str, Option<&str>)] = &[
        ("to a key id", "KMS", Some(KEY_ID)),
        ("to an alias", "KMS", Some("alias/MyKey")),
        ("back to default encryption", "NONE", None),
        (
            "to a key ARN",
            "KMS",
            Some("arn:aws:kms:eu-west-1:123456789012:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
    ];

    for (case, encryption_type, key_id) in sequence {
        let put = xray.put(encryption_type, *key_id).await;
        assert_config(case, &put, encryption_type, "UPDATING", *key_id);
        let read = xray.get().await;
        assert_config(
            &format!("{case}, read while it is applied"),
            &read,
            encryption_type,
            "UPDATING",
            *key_id,
        );
    }

    // The last put is the one that settles, and it opened a fresh span: the
    // earlier puts' spans do not carry over.
    sleep(past_settle());
    let settled = xray.get().await;
    assert_config(
        "the last of the sequence, once its transition is due",
        &settled,
        "KMS",
        "ACTIVE",
        Some("arn:aws:kms:eu-west-1:123456789012:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
    );

    // A put taken on an ALREADY active configuration opens a new span again.
    let put = xray.put("NONE", None).await;
    assert_config(
        "a put on an active configuration",
        &put,
        "NONE",
        "UPDATING",
        None,
    );
}

#[tokio::test]
async fn a_request_that_names_no_usable_kms_key_is_refused_and_stores_nothing() {
    // Rows differ along WHICH part of the selector contract fails: the member
    // combination (`PutEncryptionConfig$KeyId`: "Omit this key if you set Type to
    // NONE"), the three documented forms, the grouping and digits of a key id
    // (single-region and multi-region, one group or one digit either side of each
    // bound), and the ARN fields — including the published 12-digit length of an
    // account id, tested one digit either side of the bound.
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("KMS with no key at all", "KMS", None),
        ("a bare word", "KMS", Some("my-key")),
        ("an alias prefix with no name", "KMS", Some("alias/")),
        (
            "an alias name with a disallowed character",
            "KMS",
            Some("alias/My Key"),
        ),
        (
            "a key id with one group too few",
            "KMS",
            Some("ae4aa6d4-9a4d-489d-54ff6d789845"),
        ),
        (
            "a key id with one group too many",
            "KMS",
            Some("ae4aa6d4-9a4d-489d-9a47-54ff-6d789845"),
        ),
        (
            "a key id with an empty group",
            "KMS",
            Some("ae4aa6d4--489d-9a47-54ff6d789845"),
        ),
        (
            "a multi-region key id one digit short",
            "KMS",
            Some("mrk-1234abcd12ab34cd56ef1234567890a"),
        ),
        (
            "a multi-region key id one digit long",
            "KMS",
            Some("mrk-1234abcd12ab34cd56ef1234567890abc"),
        ),
        (
            "a key id with a non-hex digit",
            "KMS",
            Some("ze4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        (
            "an ARN of another service",
            "KMS",
            Some("arn:aws:s3:us-east-1:123456789012:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        (
            "an ARN with no region",
            "KMS",
            Some("arn:aws:kms::123456789012:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        (
            "an ARN whose account id is 11 digits",
            "KMS",
            Some("arn:aws:kms:us-east-1:12345678901:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        (
            "an ARN whose account id is 13 digits",
            "KMS",
            Some("arn:aws:kms:us-east-1:1234567890123:key/ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        (
            "an ARN whose resource is neither a key nor an alias",
            "KMS",
            Some("arn:aws:kms:us-east-1:123456789012:ae4aa6d4-9a4d-489d-9a47-54ff6d789845"),
        ),
        ("a key beside default encryption", "NONE", Some("alias/MyKey")),
    ];

    for (case, encryption_type, key_id) in cases {
        let xray = Xray::new();

        // Seed a configuration the refusal must not disturb.
        let seeded = xray.put("KMS", Some("alias/Seed")).await;
        assert_config(
            "the seeded configuration",
            &seeded,
            "KMS",
            "UPDATING",
            Some("alias/Seed"),
        );

        let refused = xray.put(encryption_type, *key_id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status, refused.body
        );
        let error_type = refused.error_type.clone().unwrap_or_default();
        assert!(
            error_type.contains("InvalidRequestException"),
            "case {case:?}: expected an InvalidRequestException, got {error_type:?} with body {:?}",
            refused.body
        );

        // Nothing was stored: the reader still reports the seeded document.
        let read = xray.get().await;
        assert_config(
            &format!("{case}, after the refusal"),
            &read,
            "KMS",
            "UPDATING",
            Some("alias/Seed"),
        );
    }
}
