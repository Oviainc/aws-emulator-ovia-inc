// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `GetTableBucketEncryption`.
//!
//! The rule under test: a table bucket is ALWAYS encrypted, so a read of its
//! encryption configuration reports the algorithm (and, for SSE-KMS, the key)
//! persisted for that bucket, and the service default — S3-managed keys, no KMS
//! key — for a bucket no configuration was ever set on; a read of a bucket the
//! store does not hold is refused with the modeled not-found, and a read
//! changes nothing a later caller sees.
//!
//! Every case drives the real wire path through the harness, so what is
//! asserted is what an AWS SDK client would see. The table spans what the rule
//! generalises over: which algorithm the bucket carries (both modeled values,
//! checked against the model's own value list below), whether a KMS key is
//! carried and which one, whether the create named a configuration at all,
//! which other optional members the create carried, and the region the bucket
//! lives in.

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::ops::types::S_S_E_ALGORITHM_VALUES;
use s3tables::state::{ComAmazonawsS3tablesS3tablebucketsState as S3TablesState, SseAlgorithm};

use super::harness::{expected_bucket_arn, Emulator};

/// Two KMS key ARNs of the form the modeled `kmsKeyArn` pattern accepts
/// (`^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$`), differing in
/// region and key id so a reported key cannot be a constant.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";
const OTHER_KMS_KEY_ARN: &str =
    "arn:aws:kms:eu-west-1:000000000000:key/fedcba98-7654-3210-fedc-ba9876543210";

/// One row: a table bucket created the way the row says, and the encryption
/// configuration a read of it must report.
#[derive(Debug)]
struct Case {
    label: &'static str,
    region: &'static str,
    name: &'static str,
    /// The create's `encryptionConfiguration`, as `(sseAlgorithm, kmsKeyArn)`,
    /// or `None` when the create named none.
    created_with: Option<(&'static str, Option<&'static str>)>,
    /// Whichever other optional members the create carries, so the reported
    /// configuration is shown not to depend on them.
    extra_members: &'static [(&'static str, &'static str)],
    expected_algorithm: &'static str,
    expected_kms_key_arn: Option<&'static str>,
}

impl Case {
    fn create_request(&self) -> Value {
        let mut request = Map::new();
        request.insert("name".to_string(), json!(self.name));
        if let Some((algorithm, key)) = self.created_with {
            let mut configuration = Map::new();
            configuration.insert("sseAlgorithm".to_string(), json!(algorithm));
            if let Some(key) = key {
                configuration.insert("kmsKeyArn".to_string(), json!(key));
            }
            request.insert(
                "encryptionConfiguration".to_string(),
                Value::Object(configuration),
            );
        }
        for (member, value) in self.extra_members {
            let value: Value = serde_json::from_str(value).expect("a well-formed member value");
            request.insert((*member).to_string(), value);
        }
        Value::Object(request)
    }
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no configuration named at create: the service default",
            region: "us-east-1",
            name: "encryption-default-east",
            created_with: None,
            extra_members: &[],
            expected_algorithm: "AES256",
            expected_kms_key_arn: None,
        },
        Case {
            label: "no configuration named, in another region",
            region: "eu-west-1",
            name: "encryption-default-west",
            created_with: None,
            extra_members: &[],
            expected_algorithm: "AES256",
            expected_kms_key_arn: None,
        },
        Case {
            label: "no configuration named, but a storage class and tags were",
            region: "us-east-1",
            name: "encryption-default-with-others",
            created_with: None,
            extra_members: &[
                (
                    "storageClassConfiguration",
                    r#"{"storageClass":"INTELLIGENT_TIERING"}"#,
                ),
                ("tags", r#"{"team":"data","tier":"gold"}"#),
            ],
            expected_algorithm: "AES256",
            expected_kms_key_arn: None,
        },
        Case {
            label: "S3-managed keys named explicitly",
            region: "us-east-1",
            name: "encryption-aes256",
            created_with: Some(("AES256", None)),
            extra_members: &[],
            expected_algorithm: "AES256",
            expected_kms_key_arn: None,
        },
        Case {
            label: "KMS-managed keys with a key",
            region: "us-east-1",
            name: "encryption-kms",
            created_with: Some(("aws:kms", Some(KMS_KEY_ARN))),
            extra_members: &[],
            expected_algorithm: "aws:kms",
            expected_kms_key_arn: Some(KMS_KEY_ARN),
        },
        Case {
            label: "KMS-managed keys with a different key, in another region",
            region: "eu-west-1",
            name: "encryption-kms-other-key",
            created_with: Some(("aws:kms", Some(OTHER_KMS_KEY_ARN))),
            extra_members: &[("tags", r#"{"team":"platform"}"#)],
            expected_algorithm: "aws:kms",
            expected_kms_key_arn: Some(OTHER_KMS_KEY_ARN),
        },
    ]
}

/// The reported `encryptionConfiguration`, as `(sseAlgorithm, kmsKeyArn)`.
fn reported(body: &Value, case: &str) -> (String, Option<String>) {
    let configuration = body
        .get("encryptionConfiguration")
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no encryptionConfiguration: {body}"));
    let algorithm = configuration
        .get("sseAlgorithm")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the configuration carries no sseAlgorithm: {configuration}")
        })
        .to_string();
    let key = configuration
        .get("kmsKeyArn")
        .and_then(Value::as_str)
        .map(str::to_string);
    (algorithm, key)
}

#[tokio::test]
async fn a_read_reports_the_persisted_configuration_or_the_service_default() {
    let emulator = Emulator::new();

    for case in cases() {
        let created = emulator
            .create_table_bucket(case.region, &case.create_request())
            .await;
        assert_eq!(
            created.status,
            200,
            "case {label:?}: the create must succeed, got {status} {body}",
            label = case.label,
            status = created.status,
            body = created.body
        );
        let arn = created.text("arn");
        assert_eq!(
            arn,
            expected_bucket_arn(case.region, case.name),
            "case {label:?}: the create must mint the modeled ARN, got {arn:?}",
            label = case.label
        );

        let read = emulator.get_table_bucket_encryption(case.region, &arn).await;
        assert_eq!(
            read.status,
            200,
            "case {label:?}: the read must succeed, got {status} {body}",
            label = case.label,
            status = read.status,
            body = read.body
        );
        let (algorithm, key) = reported(&read.body, case.label);
        assert_eq!(
            algorithm, case.expected_algorithm,
            "case {label:?}: expected sseAlgorithm {expected:?}, got {algorithm:?}",
            label = case.label,
            expected = case.expected_algorithm
        );
        assert_eq!(
            key.as_deref(),
            case.expected_kms_key_arn,
            "case {label:?}: expected kmsKeyArn {expected:?}, got {key:?}",
            label = case.label,
            expected = case.expected_kms_key_arn
        );

        // A read reports, it does not write: the record keeps whatever the
        // create persisted (nothing, where the create named no configuration),
        // and a second read answers the same.
        {
            let store = emulator.engine.state::<S3TablesState>();
            let store = store.lock();
            let bucket = store
                .table_buckets
                .get(&arn)
                .unwrap_or_else(|| panic!("case {label:?}: the bucket is resident", label = case.label));
            let persisted = bucket
                .encryption
                .as_ref()
                .map(|encryption| encryption.sse_algorithm.as_wire().to_string());
            assert_eq!(
                persisted,
                case.created_with.map(|(algorithm, _)| algorithm.to_string()),
                "case {label:?}: the read must not change the stored record, got {persisted:?}",
                label = case.label
            );
        }
        let again = emulator.get_table_bucket_encryption(case.region, &arn).await;
        assert_eq!(
            reported(&again.body, case.label),
            (algorithm, key),
            "case {label:?}: two reads must report the same configuration",
            label = case.label
        );
    }
}

#[tokio::test]
async fn the_table_spans_every_modeled_algorithm() {
    // The catalog is the model's own value list for `SSEAlgorithm`
    // (`s3tables::ops::types::S_S_E_ALGORITHM_VALUES`), so a value added to the
    // model fails this until the table above covers it.
    for modeled in S_S_E_ALGORITHM_VALUES {
        assert!(
            cases()
                .iter()
                .any(|case| case.expected_algorithm == *modeled),
            "case {modeled:?}: no row reports this modeled algorithm"
        );
        assert!(
            SseAlgorithm::from_wire(modeled).is_some(),
            "case {modeled:?}: the stored form must parse every modeled algorithm"
        );
    }
}

#[tokio::test]
async fn a_read_of_a_bucket_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // One resident bucket, so the refusals below are about the ARN they name
    // and not about an empty store.
    let created = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "encryption-resident" }))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"the resident bucket\": the create must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let resident = created.text("arn");

    // A bucket that was created and then deleted: the read must stop answering
    // for it.
    let doomed = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "encryption-deleted" }))
        .await
        .text("arn");
    let deleted = emulator.delete_table_bucket("us-east-1", &doomed).await;
    assert_eq!(
        deleted.status, 204,
        "case \"a deleted bucket\": the delete must succeed, got {status} {body}",
        status = deleted.status,
        body = deleted.body
    );

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a bucket that was never created",
            expected_bucket_arn("us-east-1", "encryption-never-created"),
            "us-east-1",
        ),
        ("a bucket that was deleted", doomed.clone(), "us-east-1"),
        (
            "the resident bucket's name in another region",
            expected_bucket_arn("eu-west-1", "encryption-resident"),
            "eu-west-1",
        ),
        (
            "another account's bucket of the same name",
            resident.replace("000000000000", "123456789012"),
            "us-east-1",
        ),
    ];

    for (label, arn, region) in &cases {
        let read = emulator.get_table_bucket_encryption(region, arn).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: expected 404, got {status} {body}",
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected NotFoundException, got {actual:?}",
            actual = read.error_code
        );
        assert_eq!(
            read.message(),
            "The specified bucket does not exist.",
            "case {label:?}: expected the service's fixed not-found sentence, got {actual:?}",
            actual = read.message()
        );
    }

    // The resident bucket still reads, so the refusals above are about the ARN
    // each named.
    let read = emulator
        .get_table_bucket_encryption("us-east-1", &resident)
        .await;
    assert_eq!(
        read.status, 200,
        "case \"the resident bucket\": the read must still succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
}
