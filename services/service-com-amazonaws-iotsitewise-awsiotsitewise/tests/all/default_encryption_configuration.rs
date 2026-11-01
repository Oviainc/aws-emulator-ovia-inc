// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise default encryption configuration
//! (`PutDefaultEncryptionConfiguration` / `DescribeDefaultEncryptionConfiguration`),
//! driven over HTTP through the same router the emulator boots with.
//!
//! The rule these tests pin: the default encryption configuration is one
//! account-wide SETTING, so a put replaces it whole and a describe reports
//! exactly what the last put stored — the encryption type it named and, under
//! customer-managed-key encryption, the ARN of the key its identifier selected,
//! in whichever of the identifier forms AWS KMS accepts it was given; an account
//! no put has configured reports the service-owned-key default without storing
//! anything; the configuration a put accepts is in force with the call, so the
//! put's own answer and every read after it report it active; and a request that
//! names an encryption type the model does not define, or an encryption type and
//! a key identifier that do not go together, is refused rather than stored.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    CONFIGURATION_STATE_VALUES, ENCRYPTION_TYPE_VALUES,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ARN_MAX_LENGTH, CONFIGURATION_STATE_ACTIVE, CONFIGURATION_STATE_UPDATE_IN_PROGRESS,
    ENCRYPTION_TYPE_KMS_BASED, ENCRYPTION_TYPE_SITEWISE_DEFAULT,
};

use crate::asset_lifecycle::{call, error_code, router, REGION};

/// The URI both operations are bound to, from
/// `smithy_query(query="operation", operation="PutDefaultEncryptionConfiguration")`:
/// `POST /configuration/account/encryption` and the `GET` of the same path.
const URI: &str = "/configuration/account/encryption";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// Every value the modelled `EncryptionType` enum accepts, written out here as an
/// oracle independent of the crate under test.
///
/// Source: `smithy_query(query="shape", shape="EncryptionType")` on this
/// service's model, which reports
/// `values: ["SITEWISE_DEFAULT_ENCRYPTION", "KMS_BASED_ENCRYPTION"]`.
const MODELLED_ENCRYPTION_TYPES: [&str; 2] = ["KMS_BASED_ENCRYPTION", "SITEWISE_DEFAULT_ENCRYPTION"];

/// Every value the modelled `ConfigurationState` enum accepts, likewise an
/// independent oracle.
///
/// Source: `smithy_query(query="shape", shape="ConfigurationState")`, which
/// reports `values: ["ACTIVE", "UPDATE_IN_PROGRESS", "UPDATE_FAILED"]`.
const MODELLED_CONFIGURATION_STATES: [&str; 3] = ["ACTIVE", "UPDATE_FAILED", "UPDATE_IN_PROGRESS"];

/// The longest a modelled `ARN` may be, as the `kmsKeyArn` member reports it
/// (`len 1..1600`).
const MODELLED_ARN_MAX_LENGTH: usize = 1600;

/// A single-Region KMS key id, in the form KMS mints one
/// (`1234abcd-12ab-34cd-56ef-1234567890ab`, from the `kmsKeyId` documentation's
/// own key-identifier forms).
const KEY_ID: &str = "1234abcd-12ab-34cd-56ef-1234567890ab";

/// Put a configuration and return `(status, body)`.
async fn put(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", URI, Some(body)).await
}

/// Describe the account's configuration and return `(status, body)`.
async fn describe(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", URI, None).await
}

/// The three members both operations answer with: `(encryptionType, kmsKeyArn,
/// configurationStatus.state)`.
fn reported(body: &Value) -> (&str, Option<&str>, &str) {
    (
        body["encryptionType"].as_str().unwrap_or(""),
        body["kmsKeyArn"].as_str(),
        body["configurationStatus"]["state"].as_str().unwrap_or(""),
    )
}

/// `true` when `value` satisfies the modelled `ARN` type the `kmsKeyArn` member
/// targets: `len 1..1600` and pattern
/// `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`.
///
/// Hand-rolled on purpose, as an INDEPENDENT oracle for what the emulator may
/// report in that member.
fn is_modelled_arn(value: &str) -> bool {
    let Some(rest) = ["arn:aws:", "arn:aws-cn:", "arn:aws-us-gov:"]
        .into_iter()
        .find_map(|prefix| value.strip_prefix(prefix))
    else {
        return false;
    };
    !rest.is_empty()
        && (1..=MODELLED_ARN_MAX_LENGTH).contains(&value.chars().count())
        && rest
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-:/_.".contains(ch))
}

/// The generated value tables the handlers answer from carry exactly the modelled
/// enums, and the values this service names are drawn from them.
#[test]
fn the_generated_value_tables_are_the_modelled_enums() {
    let mut types = ENCRYPTION_TYPE_VALUES.to_vec();
    types.sort_unstable();
    assert_eq!(
        types, MODELLED_ENCRYPTION_TYPES,
        "the generated EncryptionType table drifted from the model: expected \
         {MODELLED_ENCRYPTION_TYPES:?}, got {ENCRYPTION_TYPE_VALUES:?}"
    );

    let mut states = CONFIGURATION_STATE_VALUES.to_vec();
    states.sort_unstable();
    assert_eq!(
        states, MODELLED_CONFIGURATION_STATES,
        "the generated ConfigurationState table drifted from the model: expected \
         {MODELLED_CONFIGURATION_STATES:?}, got {CONFIGURATION_STATE_VALUES:?}"
    );

    for value in [ENCRYPTION_TYPE_SITEWISE_DEFAULT, ENCRYPTION_TYPE_KMS_BASED] {
        assert!(
            MODELLED_ENCRYPTION_TYPES.contains(&value),
            "case {value:?}: the service names an encryption type the model does not define \
             ({MODELLED_ENCRYPTION_TYPES:?})"
        );
    }
    for value in [
        CONFIGURATION_STATE_ACTIVE,
        CONFIGURATION_STATE_UPDATE_IN_PROGRESS,
    ] {
        assert!(
            MODELLED_CONFIGURATION_STATES.contains(&value),
            "case {value:?}: the service names a configuration state the model does not define \
             ({MODELLED_CONFIGURATION_STATES:?})"
        );
    }
    assert_eq!(
        ARN_MAX_LENGTH, MODELLED_ARN_MAX_LENGTH,
        "the ARN bound the service reads drifted from the modelled one: expected \
         {MODELLED_ARN_MAX_LENGTH}, got {ARN_MAX_LENGTH}"
    );
}

/// An account no put has configured reports the service-owned-key default, and
/// reading it leaves it that way — a describe is a read, so a later caller sees
/// what the first one saw.
#[tokio::test]
async fn an_unconfigured_account_reports_the_service_owned_default_and_stays_that_way() {
    let router = router();

    for attempt in 1..=3 {
        let (status, body) = describe(&router).await;
        assert_eq!(
            status, 200,
            "describe attempt {attempt} with nothing configured: got {status} {body}"
        );
        let (encryption_type, kms_key_arn, state) = reported(&body);
        assert_eq!(
            encryption_type, ENCRYPTION_TYPE_SITEWISE_DEFAULT,
            "describe attempt {attempt} with nothing configured: got {body}"
        );
        assert_eq!(
            kms_key_arn, None,
            "describe attempt {attempt} with nothing configured reported a key: got {body}"
        );
        assert_eq!(
            state, CONFIGURATION_STATE_ACTIVE,
            "describe attempt {attempt} with nothing configured: got {body}"
        );
    }
}

/// One row of the round-trip table: an encryption type, the key identifier the
/// put names (if any), and the `kmsKeyArn` the service must report for it.
struct Case {
    label: &'static str,
    encryption_type: &'static str,
    kms_key_id: Option<String>,
    expected_arn: Option<String>,
}

/// The identifier forms a `kmsKeyId` may take, each paired with the ARN the
/// service must report for it, plus the type that names no key at all.
///
/// The forms are the key-identifier forms AWS KMS accepts and the `kmsKeyId`
/// documentation points at — a key id (single-Region and multi-Region), an
/// `alias/<name>` path, and the full ARN of either. An identifier that is
/// already an ARN names its own partition, Region and account, so it is reported
/// as given; a bare id or alias names a key in the Region the request addresses
/// and in this emulator's account.
fn round_trip_cases() -> Vec<Case> {
    let multi_region = format!("mrk-{}", "a1b2c3d4".repeat(4));
    vec![
        Case {
            label: "the service-owned key, which names no customer managed key",
            encryption_type: ENCRYPTION_TYPE_SITEWISE_DEFAULT,
            kms_key_id: None,
            expected_arn: None,
        },
        Case {
            label: "a single-Region key id",
            encryption_type: ENCRYPTION_TYPE_KMS_BASED,
            kms_key_id: Some(KEY_ID.to_string()),
            expected_arn: Some(format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{KEY_ID}")),
        },
        Case {
            label: "a multi-Region key id",
            encryption_type: ENCRYPTION_TYPE_KMS_BASED,
            kms_key_id: Some(multi_region.clone()),
            expected_arn: Some(format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{multi_region}")),
        },
        Case {
            label: "an alias path",
            encryption_type: ENCRYPTION_TYPE_KMS_BASED,
            kms_key_id: Some("alias/Plant_1-east".to_string()),
            expected_arn: Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:alias/Plant_1-east"
            )),
        },
        Case {
            label: "the full ARN of a key in another partition, Region and account",
            encryption_type: ENCRYPTION_TYPE_KMS_BASED,
            kms_key_id: Some(
                "arn:aws-us-gov:kms:us-gov-west-1:210987654321:key/\
                 ae4aa6d4-9a4d-489d-9a47-54ff6d789845"
                    .to_string(),
            ),
            expected_arn: Some(
                "arn:aws-us-gov:kms:us-gov-west-1:210987654321:key/\
                 ae4aa6d4-9a4d-489d-9a47-54ff6d789845"
                    .to_string(),
            ),
        },
        Case {
            label: "the full ARN of an alias",
            encryption_type: ENCRYPTION_TYPE_KMS_BASED,
            kms_key_id: Some("arn:aws:kms:eu-west-1:123456789012:alias/MyKey".to_string()),
            expected_arn: Some("arn:aws:kms:eu-west-1:123456789012:alias/MyKey".to_string()),
        },
    ]
}

/// The body a case puts.
fn case_body(case: &Case) -> Value {
    let mut body = json!({ "encryptionType": case.encryption_type });
    if let Some(key_id) = case.kms_key_id.as_deref() {
        body["kmsKeyId"] = Value::String(key_id.to_string());
    }
    body
}

/// Every encryption type, and every key-identifier form a KMS-based one accepts,
/// reads back through the describe: the setting the put stored is the one the
/// describe reports, and the key is reported as the ARN of the key the identifier
/// selected. Putting the rows in sequence also pins that a put REPLACES the
/// setting whole rather than merging into it.
#[tokio::test]
async fn every_encryption_type_and_key_identifier_form_reads_back() {
    let router = router();

    for case in round_trip_cases() {
        let label = case.label;
        let (status, body) = put(&router, case_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: put got {status} {body}"
        );
        let (encryption_type, kms_key_arn, state) = reported(&body);
        assert_eq!(
            encryption_type, case.encryption_type,
            "case {label:?}: the put reported encryption type {encryption_type}, expected {}",
            case.encryption_type
        );
        assert_eq!(
            kms_key_arn,
            case.expected_arn.as_deref(),
            "case {label:?}: the put reported key {kms_key_arn:?}, expected {:?}",
            case.expected_arn
        );
        assert!(
            CONFIGURATION_STATE_VALUES.contains(&state),
            "case {label:?}: the put reported the unmodelled state {state:?}"
        );
        assert_eq!(
            state, CONFIGURATION_STATE_ACTIVE,
            "case {label:?}: the put reported state {state:?}, expected the configuration it \
             applied to be in force"
        );
        if let Some(arn) = kms_key_arn {
            assert!(
                is_modelled_arn(arn),
                "case {label:?}: the put reported {arn:?}, which is not a modelled ARN"
            );
        }

        let (status, body) = describe(&router).await;
        assert_eq!(status, 200, "case {label:?}: describe got {status} {body}");
        let (encryption_type, kms_key_arn, state) = reported(&body);
        assert_eq!(
            encryption_type, case.encryption_type,
            "case {label:?}: the describe reported encryption type {encryption_type}, expected {}",
            case.encryption_type
        );
        assert_eq!(
            kms_key_arn,
            case.expected_arn.as_deref(),
            "case {label:?}: the describe reported key {kms_key_arn:?}, expected {:?}",
            case.expected_arn
        );
        assert!(
            CONFIGURATION_STATE_VALUES.contains(&state),
            "case {label:?}: the describe reported the unmodelled state {state:?}"
        );
        assert_eq!(
            state, CONFIGURATION_STATE_ACTIVE,
            "case {label:?}: the describe reported state {state:?}, expected the configuration \
             the put applied to be in force"
        );
    }
}

/// The configuration a put accepts is in force with the call: the put's own
/// answer reports it active, a describe taken straight after reports the same
/// thing, and the describe repeated reports it again — there is no state a
/// client has to wait out, and no later call that moves it.
#[tokio::test]
async fn an_accepted_configuration_is_in_force_on_the_put_that_stored_it() {
    let router = router();

    let expected_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{KEY_ID}");
    let (status, body) = put(
        &router,
        json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": KEY_ID }),
    )
    .await;
    assert_eq!(status, 200, "put a KMS-based configuration: {status} {body}");
    let (_, _, state) = reported(&body);
    assert_eq!(
        state, CONFIGURATION_STATE_ACTIVE,
        "the put of a fresh configuration reported state {state:?}, expected the configuration \
         it applied to be in force"
    );

    for attempt in 1..=3 {
        let (status, body) = describe(&router).await;
        assert_eq!(status, 200, "describe {attempt}: {status} {body}");
        let (encryption_type, kms_key_arn, state) = reported(&body);
        assert_eq!(
            state, CONFIGURATION_STATE_ACTIVE,
            "describe {attempt} reported state {state:?}: got {body}"
        );
        assert_eq!(
            encryption_type, ENCRYPTION_TYPE_KMS_BASED,
            "describe {attempt} changed the encryption type: got {body}"
        );
        assert_eq!(
            kms_key_arn,
            Some(expected_arn.as_str()),
            "describe {attempt} changed the key: got {kms_key_arn:?}, expected {expected_arn:?}"
        );
    }
}

/// A put is the last word on the account's encryption: one naming the
/// service-owned key drops the customer managed key an earlier put selected,
/// rather than leaving it to be reported beside a type that uses none.
#[tokio::test]
async fn a_put_naming_the_service_owned_key_drops_the_customer_managed_key() {
    let router = router();

    for key_id in [KEY_ID, "alias/MyKey"] {
        let (status, body) = put(
            &router,
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": key_id }),
        )
        .await;
        assert_eq!(status, 200, "case {key_id:?}: put got {status} {body}");
        let (_, kms_key_arn, _) = reported(&body);
        assert!(
            kms_key_arn.is_some(),
            "case {key_id:?}: the KMS-based put reported no key: got {body}"
        );

        let (status, body) = put(&router, json!({ "encryptionType": ENCRYPTION_TYPE_SITEWISE_DEFAULT })).await;
        assert_eq!(
            status, 200,
            "case {key_id:?}: put the service-owned key: got {status} {body}"
        );
        let (encryption_type, kms_key_arn, _) = reported(&body);
        assert_eq!(
            encryption_type, ENCRYPTION_TYPE_SITEWISE_DEFAULT,
            "case {key_id:?}: got {body}"
        );
        assert_eq!(
            kms_key_arn, None,
            "case {key_id:?}: the service-owned-key put kept the customer managed key \
             {kms_key_arn:?}"
        );

        let (_, body) = describe(&router).await;
        let (encryption_type, kms_key_arn, _) = reported(&body);
        assert_eq!(
            encryption_type, ENCRYPTION_TYPE_SITEWISE_DEFAULT,
            "case {key_id:?}: the describe after dropping the key: got {body}"
        );
        assert_eq!(
            kms_key_arn, None,
            "case {key_id:?}: the describe still reports the key {kms_key_arn:?}"
        );
    }
}

/// A request whose encryption type the model does not define, or whose key
/// identifier does not go with the type it names, is refused — and the refusal
/// leaves the stored configuration exactly as it was.
#[tokio::test]
async fn a_request_outside_the_rule_is_refused_and_stores_nothing() {
    let router = router();

    // The configuration every refusal below has to leave alone.
    let seeded_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:alias/seeded");
    let (status, body) = put(
        &router,
        json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": "alias/seeded" }),
    )
    .await;
    assert_eq!(status, 200, "seed the configuration: {status} {body}");

    // Each row is a request the rule refuses: a key identifier missing where the
    // type requires one, identifiers in none of the forms KMS accepts (a bare
    // word, another vendor's ARN, a KMS ARN whose account is not 12 digits, an
    // empty alias name, an id with trailing whitespace), ARNs naming a partition
    // or a Region the modelled `ARN` pattern excludes, an identifier whose ARN
    // would be longer than that type admits, one past the published `kmsKeyId`
    // length bound (`len 1..2048`), a key named beside the type that uses none,
    // and encryption types outside the modelled enum.
    let long_alias = format!("alias/{}", "a".repeat(2_000));
    let past_the_length_bound = "a".repeat(2_049);
    let rows: Vec<(String, Value)> = vec![
        (
            "no key identifier under KMS-based encryption".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED }),
        ),
        (
            "an identifier that is not a key at all".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": "not-a-key" }),
        ),
        (
            "another vendor's ARN".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": "arn:aws:s3:::bucket/key" }),
        ),
        (
            "a KMS ARN whose account is 11 digits".to_string(),
            json!({
                "encryptionType": ENCRYPTION_TYPE_KMS_BASED,
                "kmsKeyId": format!("arn:aws:kms:{REGION}:12345678901:key/{KEY_ID}"),
            }),
        ),
        (
            "an alias with no name".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": "alias/" }),
        ),
        (
            "a partition the modelled ARN pattern excludes".to_string(),
            json!({
                "encryptionType": ENCRYPTION_TYPE_KMS_BASED,
                "kmsKeyId": format!("arn:aws-iso:kms:{REGION}:123456789012:key/{KEY_ID}"),
            }),
        ),
        (
            "a Region carrying a character the modelled ARN pattern excludes".to_string(),
            json!({
                "encryptionType": ENCRYPTION_TYPE_KMS_BASED,
                "kmsKeyId": format!("arn:aws:kms:us east 1:123456789012:key/{KEY_ID}"),
            }),
        ),
        (
            "a key id with trailing whitespace".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": format!("{KEY_ID} ") }),
        ),
        (
            "an identifier whose ARN is longer than a modelled ARN".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": long_alias }),
        ),
        (
            "an identifier past the modelled kmsKeyId length bound".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_KMS_BASED, "kmsKeyId": past_the_length_bound }),
        ),
        (
            "a key named beside the service-owned-key type".to_string(),
            json!({ "encryptionType": ENCRYPTION_TYPE_SITEWISE_DEFAULT, "kmsKeyId": KEY_ID }),
        ),
        (
            "a lowercase spelling of a modelled encryption type".to_string(),
            json!({ "encryptionType": "kms_based_encryption", "kmsKeyId": KEY_ID }),
        ),
        (
            "another service's encryption type".to_string(),
            json!({ "encryptionType": "AES256" }),
        ),
        (
            "the empty encryption type".to_string(),
            json!({ "encryptionType": "" }),
        ),
    ];

    for (label, body) in rows {
        let (status, answer) = put(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?} ({body}): got {status} {answer}"
        );
        let code = error_code(&answer);
        assert!(
            matches!(code, "InvalidRequestException" | "ValidationException"),
            "case {label:?}: refused with {code:?}, expected a request refusal: {answer}"
        );

        let (_, answer) = describe(&router).await;
        let (encryption_type, kms_key_arn, _) = reported(&answer);
        assert_eq!(
            encryption_type, ENCRYPTION_TYPE_KMS_BASED,
            "case {label:?}: the refusal changed the encryption type: got {answer}"
        );
        assert_eq!(
            kms_key_arn,
            Some(seeded_arn.as_str()),
            "case {label:?}: the refusal changed the key to {kms_key_arn:?}, expected \
             {seeded_arn:?}"
        );
    }
}
