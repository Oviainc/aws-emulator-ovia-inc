// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `CreateActivity` ADMITS, driven over this service's own
//! wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! `tests/all/activity_lifecycle.rs` pins the create -> describe round trip, the
//! name rule and the idempotency check. The rule under test HERE is the other half
//! of the same operation: a create is admitted only when the request satisfies every
//! condition the operation's own members carry, and a create that is refused
//! persists NOTHING — so the refusal is the operation's own modelled error and a
//! later read of the ARN that name determines reports no activity at all.
//!
//! The two conditions the model does not carry, each with its own modelled error:
//!
//! 1. which members an `encryptionConfiguration` of each type may hold
//!    (`InvalidEncryptionConfiguration`, 400) — the same rule
//!    `CreateStateMachine` reads, so the rows mirror
//!    `tests/all/state_machine_create_options.rs`;
//! 2. how many `tags` one resource may carry (`TooManyTags`, 400).
//!
//! The harness (`Emulator`, `discriminated`, `error_code`, `string`) is the one
//! `state_machine_lifecycle` builds; `tests/all.rs` is this binary's crate root, so
//! it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{
    ENCRYPTION_TYPE_AWS_OWNED_KEY, ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
    MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS, MAX_TAGS, MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
};

use crate::state_machine_lifecycle::{discriminated, error_code, string, Emulator, ACCOUNT, REGION};

/// The text the service's `TooManyTags` refusal carries, spelled out here rather
/// than imported so this assertion cannot follow the implementation if it drifts.
const TOO_MANY_TAGS_TEXT: &str = "Too many tags provided.";

/// A KMS key ARN of one of the forms `kmsKeyId` documents ("an alias, alias ARN, key
/// ID, or key ARN of a symmetric encryption KMS key").
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The ARN an activity of `name` carries in the region every request here signs for.
fn activity_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:{name}")
}

/// Assert that the store holds no activity under the ARN `name` determines, which is
/// what every refused create above must leave behind.
async fn assert_nothing_stored(emulator: &Emulator, case: &str, name: &str) {
    let arn = activity_arn(name);
    let (status, body) = emulator
        .call("DescribeActivity", &json!({ "activityArn": arn }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ActivityDoesNotExist"),
        "case {case:?}: a refused create must store nothing under {arn}, got {status} with body \
         {body:?}"
    );
}

/// `count` distinct tag pairs.
fn tags(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({ "key": format!("k{index}"), "value": "v" }))
        .collect()
}

// ---------------------------------------------------------------------------
// encryptionConfiguration
// ---------------------------------------------------------------------------

/// An `encryptionConfiguration` is accepted only when its members agree with its
/// `type`, and what is accepted is PERSISTED: the describe reads the record back.
#[tokio::test]
async fn create_activity_accepts_every_encryption_configuration_it_can_apply() {
    let emulator = Emulator::new();

    // Rows vary along the FORM of the key the configuration names and along the
    // reuse period, taken at each published bound of the modelled 60..900 range.
    let accepted: Vec<(&str, Value)> = vec![
        ("no encryption configuration at all", json!({})),
        (
            "the AWS owned key spelled out",
            json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY }),
        ),
        (
            "a customer-managed key named by key ARN",
            json!({ "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY, "kmsKeyId": KEY_ARN }),
        ),
        (
            "a customer-managed key named by key id",
            json!({
                "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                "kmsKeyId": "12345678-1234-1234-1234-123456789012",
            }),
        ),
        (
            "a customer-managed key named by alias",
            json!({ "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY, "kmsKeyId": "alias/sfn" }),
        ),
        (
            "a customer-managed key named by alias ARN",
            json!({
                "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                "kmsKeyId": "arn:aws:kms:us-west-2:000000000000:alias/sfn",
            }),
        ),
        (
            "a customer-managed key at the lowest reuse period the range allows",
            json!({
                "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                "kmsKeyId": KEY_ARN,
                "kmsDataKeyReusePeriodSeconds": MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
            }),
        ),
        (
            "a customer-managed key at the highest reuse period the range allows",
            json!({
                "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                "kmsKeyId": KEY_ARN,
                "kmsDataKeyReusePeriodSeconds": MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
            }),
        ),
    ];

    for (index, (case, configuration)) in accepted.into_iter().enumerate() {
        let name = format!("encryption-ok-{index}");
        let mut request = json!({ "name": name });
        let configured = configuration.as_object().is_some_and(|it| !it.is_empty());
        if configured {
            request["encryptionConfiguration"] = configuration.clone();
        }

        let (status, created) = emulator.call("CreateActivity", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a valid encryption configuration must be accepted, got {status} with \
             body {created:?}"
        );
        let arn = string(&created, "activityArn");
        assert_eq!(
            arn,
            activity_arn(&name),
            "case {case:?}: expected the ARN {} the region and the name determine, got {arn:?}",
            activity_arn(&name)
        );

        // The EFFECTIVE configuration: a create that named none reads back the type
        // its data is encrypted under.
        let expected = discriminated(
            "EncryptionConfiguration",
            if configured {
                configuration
            } else {
                json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY })
            },
        );
        let described = emulator
            .ok("DescribeActivity", &json!({ "activityArn": arn }))
            .await;
        assert_eq!(
            described.get("encryptionConfiguration"),
            Some(&expected),
            "case {case:?}: expected the stored encryption configuration {expected}, got \
             {described:?}"
        );
    }
}

/// An `encryptionConfiguration` the service cannot apply is
/// `InvalidEncryptionConfiguration` (400) and stores nothing.
#[tokio::test]
async fn create_activity_refuses_an_encryption_configuration_it_cannot_apply() {
    let emulator = Emulator::new();

    // Rows vary along WHICH condition makes the configuration one the service cannot
    // apply: a customer-managed key with no usable key, or an owned key carrying a
    // member that "only applies to customer managed keys".
    let refused: Vec<(&str, Value)> = vec![
        (
            "a customer-managed key with no kmsKeyId",
            json!({ "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY }),
        ),
        (
            "a customer-managed key whose kmsKeyId names no KMS key",
            json!({ "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY, "kmsKeyId": "not-a-key" }),
        ),
        (
            "a customer-managed key named by an ARN of another service",
            json!({
                "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                "kmsKeyId": "arn:aws:lambda:us-west-2:000000000000:function:encrypt",
            }),
        ),
        (
            "a customer-managed key named by a blank kmsKeyId",
            json!({ "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY, "kmsKeyId": " " }),
        ),
        (
            "the AWS owned key with a kmsKeyId it cannot use",
            json!({ "type": ENCRYPTION_TYPE_AWS_OWNED_KEY, "kmsKeyId": KEY_ARN }),
        ),
        (
            "the AWS owned key with a data key reuse period it cannot use",
            json!({
                "type": ENCRYPTION_TYPE_AWS_OWNED_KEY,
                "kmsDataKeyReusePeriodSeconds": MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS,
            }),
        ),
    ];

    for (index, (case, configuration)) in refused.into_iter().enumerate() {
        let name = format!("encryption-bad-{index}");
        let (status, body) = emulator
            .call(
                "CreateActivity",
                &json!({ "name": name, "encryptionConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidEncryptionConfiguration"),
            "case {case:?}: expected 400 InvalidEncryptionConfiguration, got {status} with body \
             {body:?}"
        );
        assert!(
            body.get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.starts_with("Invalid Encryption Configuration:")),
            "case {case:?}: the refusal must say what is invalid, got {body:?}"
        );
        assert_nothing_stored(&emulator, case, &name).await;
    }

    // One past each published bound of the modelled 60..900 range. The range is
    // MODELLED, so the centralized request walk may refuse the value before the
    // handler runs — the requirement is the 400 and that nothing is created, not
    // which of the two 400s answers.
    let outside = [
        (
            "one second below the lowest reuse period",
            MIN_KMS_DATA_KEY_REUSE_PERIOD_SECONDS - 1,
        ),
        (
            "one second above the highest reuse period",
            MAX_KMS_DATA_KEY_REUSE_PERIOD_SECONDS + 1,
        ),
    ];
    for (index, (case, period)) in outside.into_iter().enumerate() {
        let name = format!("reuse-bad-{index}");
        let (status, body) = emulator
            .call(
                "CreateActivity",
                &json!({
                    "name": name,
                    "encryptionConfiguration": {
                        "type": ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY,
                        "kmsKeyId": KEY_ARN,
                        "kmsDataKeyReusePeriodSeconds": period,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a reuse period of {period} must be refused, got {status} with body \
             {body:?}"
        );
        let code = error_code(&body);
        assert!(
            ["InvalidEncryptionConfiguration", "ValidationException"].contains(&code.as_str()),
            "case {case:?}: expected the encryption or validation refusal, got {code} with body \
             {body:?}"
        );
        assert_nothing_stored(&emulator, case, &name).await;
    }

    // An `EncryptionType` outside the modelled enum is refused by the centralized
    // request walk before the handler runs.
    let (status, code) = emulator
        .err(
            "CreateActivity",
            &json!({
                "name": "encryption-unknown-type",
                "encryptionConfiguration": { "type": "CUSTOMER_MANAGED_HSM_KEY" },
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "an encryption type the model does not declare is refused, got {status} {code}"
    );
    assert_nothing_stored(&emulator, "an unmodelled encryption type", "encryption-unknown-type")
        .await;
}

// ---------------------------------------------------------------------------
// tags
// ---------------------------------------------------------------------------

/// The create stores the tags it carries up to the per-resource limit, and a create
/// past that limit is `TooManyTags` (400) carrying the service's fixed text, with
/// nothing stored.
#[tokio::test]
async fn create_activity_bounds_the_tags_it_stores() {
    let emulator = Emulator::new();

    // Rows differ along the COUNT of tags, at the limit and on either side of it.
    let accepted = [
        ("no tags member at all", None),
        ("an empty tag list", Some(0)),
        ("one tag", Some(1)),
        ("the most tags a resource may carry", Some(MAX_TAGS)),
    ];
    for (index, (case, count)) in accepted.into_iter().enumerate() {
        let name = format!("tags-ok-{index}");
        let mut request = json!({ "name": name });
        if let Some(count) = count {
            request["tags"] = Value::Array(tags(count));
        }
        let (status, created) = emulator.call("CreateActivity", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {count:?} tags, got {status} with body {created:?}"
        );

        let listed = emulator
            .ok(
                "ListTagsForResource",
                &json!({ "resourceArn": activity_arn(&name) }),
            )
            .await;
        let stored = listed
            .get("tags")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_else(|| panic!("case {case:?}: expected a 'tags' array, got {listed:?}"));
        assert_eq!(
            stored,
            count.unwrap_or(0),
            "case {case:?}: expected the {:?} tags the create carried, got {stored}",
            count.unwrap_or(0)
        );
    }

    // Rows differ along HOW FAR past the limit the request is; the refusal is the
    // same, and so is its FIXED text.
    let refused = [
        ("one tag past the limit", MAX_TAGS + 1),
        ("twice the limit", MAX_TAGS * 2),
        ("ten times the limit", MAX_TAGS * 10),
    ];
    for (index, (case, count)) in refused.into_iter().enumerate() {
        let name = format!("tags-bad-{index}");
        let (status, body) = emulator
            .call(
                "CreateActivity",
                &json!({ "name": name, "tags": tags(count) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "TooManyTags"),
            "case {case:?}: expected 400 TooManyTags for {count} tags, got {status} with body \
             {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(TOO_MANY_TAGS_TEXT),
            "case {case:?}: expected the fixed service text {TOO_MANY_TAGS_TEXT:?}, got {body:?}"
        );
        assert_nothing_stored(&emulator, case, &name).await;
    }
}
