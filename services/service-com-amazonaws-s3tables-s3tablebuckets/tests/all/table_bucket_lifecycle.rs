// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the S3 Tables table-bucket control plane:
//! `CreateTableBucket` -> `GetTableBucket`.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` over a
//! restJson1 request whose SigV4 credential scope names the `s3tables` signing
//! name), so what is asserted is what an AWS SDK client would see rather than a
//! function's return value.
//!
//! The rule under test: a table bucket is MINTED from the request and PERSISTED,
//! and every later answer about it is reconstructed from that record — its ARN
//! is derived from the request's region and the requested name, its name is
//! unique per account and Region, and a name or a configuration outside the
//! service's rules is refused rather than stored. Each table spans the class the
//! rule generalises over (name form and length, which optional members the
//! request carries, which encryption algorithm, which storage class, how many
//! tags, which region), not one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;
use smithy_server_runtime::MultiServiceRouter;

use s3tables::ops::types::{
    S_S_E_ALGORITHM_VALUES, STORAGE_CLASS_VALUES, TABLE_BUCKET_TYPE_VALUES,
};
use s3tables::state::{
    ComAmazonawsS3tablesS3tablebucketsState as S3TablesState, SseAlgorithm, StorageClass,
    TableBucket, TableBucketType,
};

/// The account id every emulated ARN carries (`engine::emulator_account_id()`),
/// restated so an ARN assertion reads whole.
const ACCOUNT: &str = "000000000000";

/// A KMS key ARN of the form the modeled `kmsKeyArn` pattern accepts
/// (`^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$`).
const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A member of the body, as a string.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "response has no string `{member}` member: {body}",
                    body = self.body
                )
            })
            .to_string()
    }

    /// The `message` member of an error body, when it carries one.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the S3 Tables service
/// mounted on it, reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(s3tables::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation over the wire. `region` is the region the request's
    /// SigV4 credential scope names, which is the region the handlers scope an
    /// ARN to.
    async fn call(&self, method: &str, path: &str, region: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTS3TABLES/20260101/{region}/s3tables/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3tables.{region}.amazonaws.com{path}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .rsplit('#')
                    .next()
                    .unwrap_or(value)
                    .split(':')
                    .next()
                    .unwrap_or(value)
                    .to_string()
            });
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_table_bucket(&self, region: &str, request: &Value) -> Reply {
        self.call("PUT", "/buckets", region, Some(request)).await
    }

    async fn get_table_bucket(&self, region: &str, arn: &str) -> Reply {
        self.call("GET", &format!("/buckets/{}", encode(arn)), region, None)
            .await
    }

    /// The stored record, straight out of the shared engine. Used for the
    /// members `CreateTableBucket` persists that no implemented reader exposes
    /// yet (the encryption configuration, the storage class and the tags).
    fn stored(&self, arn: &str) -> Option<TableBucket> {
        self.engine
            .state::<S3TablesState>()
            .lock()
            .table_buckets
            .get(arn)
            .cloned()
    }

    fn stored_count(&self) -> usize {
        self.engine
            .state::<S3TablesState>()
            .lock()
            .table_buckets
            .len()
    }
}

/// An `@httpLabel` is ONE path segment, so an ARN's `:` and `/` have to travel
/// as `%3A` and `%2F` or the request addresses a different route.
fn encode(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The ARN `CreateTableBucket` must mint for `name` in `region`.
fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:s3tables:{region}:{ACCOUNT}:bucket/{name}")
}

/// `createdAt` as epoch seconds, read from the ONE wire form the model declares
/// for it: `@timestampFormat("date-time")`, so an RFC 3339 string. The
/// epoch-seconds number the protocol defaults to is the defect, not an accepted
/// alternative.
fn created_at_seconds(body: &Value, case: &str) -> f64 {
    match body.get("createdAt") {
        Some(Value::String(text)) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: createdAt {text:?} is not a date-time: {error}")
            })
            .as_secs_f64(),
        actual => panic!(
            "case {case:?}: expected createdAt to be an RFC 3339 date-time string, got {actual:?}"
        ),
    }
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One `CreateTableBucket` request, as a row of the table below.
#[derive(Debug, Clone)]
struct CreateCase {
    label: &'static str,
    region: &'static str,
    name: String,
    /// `encryptionConfiguration`, as `(sseAlgorithm, kmsKeyArn)`.
    encryption: Option<(&'static str, Option<&'static str>)>,
    /// `storageClassConfiguration.storageClass`.
    storage_class: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

impl CreateCase {
    fn request(&self) -> Value {
        let mut request = Map::new();
        request.insert("name".to_string(), json!(self.name));
        if let Some((algorithm, key)) = self.encryption {
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
        if let Some(storage_class) = self.storage_class {
            request.insert(
                "storageClassConfiguration".to_string(),
                json!({ "storageClass": storage_class }),
            );
        }
        if !self.tags.is_empty() {
            let tags: Map<String, Value> = self
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            request.insert("tags".to_string(), Value::Object(tags));
        }
        Value::Object(request)
    }
}

/// The table: it spans name length (the modeled minimum, a middling name, the
/// modeled maximum), name form (letters, digits, interior hyphens), which
/// optional members the request carries, both encryption algorithms, both
/// storage classes, tag counts from none to several, and two regions.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the shortest name the model allows, no optional members",
            region: "us-east-1",
            name: "ab1".to_string(),
            encryption: None,
            storage_class: None,
            tags: &[],
        },
        CreateCase {
            label: "interior hyphens, SSE-S3, the standard storage class, one tag",
            region: "us-east-1",
            name: "my-table-bucket".to_string(),
            encryption: Some(("AES256", None)),
            storage_class: Some("STANDARD"),
            tags: &[("team", "analytics")],
        },
        CreateCase {
            label: "SSE-KMS with a key, intelligent tiering, several tags",
            region: "eu-west-1",
            name: "finance-lakehouse".to_string(),
            encryption: Some(("aws:kms", Some(KMS_KEY_ARN))),
            storage_class: Some("INTELLIGENT_TIERING"),
            tags: &[("env", "prod"), ("owner", "finance"), ("cost-center", "4711")],
        },
        CreateCase {
            label: "digits only, tags but no configuration",
            region: "us-west-2",
            name: "20260101".to_string(),
            encryption: None,
            storage_class: None,
            tags: &[("snapshot", "")],
        },
        CreateCase {
            label: "a name at the modeled maximum of 63 characters",
            region: "us-west-2",
            // 63 characters, the modeled maximum: `a` + 30 `b-` pairs + `b9`.
            name: format!("a{}b9", "b-".repeat(30)),
            encryption: Some(("AES256", None)),
            storage_class: Some("INTELLIGENT_TIERING"),
            tags: &[],
        },
    ]
}

#[tokio::test]
async fn a_created_table_bucket_is_readable_field_for_field() {
    let emulator = Emulator::new();
    let cases = create_cases();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for case in &cases {
        assert!(
            case.name.chars().count() >= 3 && case.name.chars().count() <= 63,
            "case {label:?}: the row's own name must satisfy the modeled length, got {len}",
            label = case.label,
            len = case.name.chars().count()
        );

        let before = now_seconds();
        let created = emulator
            .create_table_bucket(case.region, &case.request())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateTableBucket must succeed, got {status} {body}",
            label = case.label,
            status = created.status,
            body = created.body
        );
        let arn = created.text("arn");
        assert_eq!(
            arn,
            expected_arn(case.region, &case.name),
            "case {label:?}: the minted ARN must carry the request's region and name, got {arn:?}",
            label = case.label
        );

        // The round trip: everything the reader answers comes from the record
        // the create persisted.
        let read = emulator.get_table_bucket(case.region, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetTableBucket must succeed, got {status} {body}",
            label = case.label,
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.text("arn"),
            arn,
            "case {label:?}: the reader must answer with the bucket's own ARN",
            label = case.label
        );
        assert_eq!(
            read.text("name"),
            case.name,
            "case {label:?}: the reader must answer with the requested name, got {actual:?}",
            label = case.label,
            actual = read.text("name")
        );
        assert_eq!(
            read.text("ownerAccountId"),
            ACCOUNT,
            "case {label:?}: the owner must be the emulator's account, got {actual:?}",
            label = case.label,
            actual = read.text("ownerAccountId")
        );
        assert_eq!(
            read.text("type"),
            TableBucketType::Customer.as_wire(),
            "case {label:?}: a bucket this operation creates is customer-owned, got {actual:?}",
            label = case.label,
            actual = read.text("type")
        );

        let bucket_id = read.text("tableBucketId");
        assert!(
            !bucket_id.is_empty(),
            "case {label:?}: tableBucketId must be a server-generated id, got {bucket_id:?}",
            label = case.label
        );
        assert!(
            !bucket_id.contains(&case.name),
            "case {label:?}: tableBucketId must not be derived from the request, got {bucket_id:?}",
            label = case.label
        );
        assert!(
            seen_ids.insert(bucket_id.clone()),
            "case {label:?}: tableBucketId {bucket_id:?} was already minted for another bucket",
            label = case.label
        );

        let created_at = created_at_seconds(&read.body, case.label);
        let after = now_seconds();
        assert!(
            created_at >= before - 1.0 && created_at <= after + 1.0,
            "case {label:?}: createdAt {created_at} must lie in the window the create ran in \
             ({before}..={after})",
            label = case.label
        );

        // Reading twice must answer identically: a read changes nothing.
        let again = emulator.get_table_bucket(case.region, &arn).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: two reads of one bucket must answer identically",
            label = case.label
        );

        // What the create persisted beyond what this reader exposes.
        let stored = emulator.stored(&arn).unwrap_or_else(|| {
            panic!(
                "case {label:?}: the bucket must be persisted under its ARN {arn:?}",
                label = case.label
            )
        });
        let expected_encryption = case.encryption.map(|(algorithm, key)| {
            (
                SseAlgorithm::from_wire(algorithm)
                    .unwrap_or_else(|| panic!("the row names a modeled algorithm: {algorithm:?}")),
                key.map(str::to_string),
            )
        });
        let actual_encryption = stored
            .encryption
            .as_ref()
            .map(|encryption| (encryption.sse_algorithm, encryption.kms_key_arn.clone()));
        assert_eq!(
            actual_encryption, expected_encryption,
            "case {label:?}: the stored encryption configuration must be the requested one, got \
             {actual_encryption:?}",
            label = case.label
        );
        assert_eq!(
            stored.storage_class,
            case.storage_class.map(|value| {
                StorageClass::from_wire(value)
                    .unwrap_or_else(|| panic!("the row names a modeled storage class: {value:?}"))
            }),
            "case {label:?}: the stored storage class must be the requested one, got {actual:?}",
            label = case.label,
            actual = stored.storage_class
        );
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            // The store keeps tags in key order, so compare in that order.
            .collect::<std::collections::BTreeMap<String, String>>()
            .into_iter()
            .collect();
        let actual_tags: Vec<(String, String)> = stored.tags.clone().into_iter().collect();
        assert_eq!(
            actual_tags, expected_tags,
            "case {label:?}: the stored tags must be the requested ones, got {actual_tags:?}",
            label = case.label
        );
    }

    assert_eq!(
        emulator.stored_count(),
        cases.len(),
        "every row must have left exactly one bucket in the store"
    );
}

#[tokio::test]
async fn a_name_is_unique_per_region_and_a_repeat_conflicts() {
    let emulator = Emulator::new();
    const NAME: &str = "shared-bucket-name";

    // The same name in different regions names different buckets: a table
    // bucket name is unique within an account and Region.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let created = emulator
            .create_table_bucket(region, &json!({ "name": NAME }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the first create in a region must succeed, got {status} {body}",
            status = created.status,
            body = created.body
        );
        assert_eq!(
            created.text("arn"),
            expected_arn(region, NAME),
            "case {region:?}: the ARN must be scoped to the request's region, got {actual:?}",
            actual = created.text("arn")
        );
    }
    assert_eq!(
        emulator.stored_count(),
        3,
        "one name in three regions is three buckets, got {actual}",
        actual = emulator.stored_count()
    );

    // A repeat in the same region is the conflict with a previous write.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let repeat = emulator
            .create_table_bucket(region, &json!({ "name": NAME }))
            .await;
        assert_eq!(
            repeat.status, 409,
            "case {region:?}: a repeated name must conflict, got {status} {body}",
            status = repeat.status,
            body = repeat.body
        );
        assert_eq!(
            repeat.error_code.as_deref(),
            Some("ConflictException"),
            "case {region:?}: the refusal must be the modeled conflict, got {actual:?}",
            actual = repeat.error_code
        );
        assert_eq!(
            repeat.message(),
            // The one sentence this service states a taken table bucket name
            // with; the suite that owns the refusal wording is
            // `fixed_error_sentences.rs`.
            "The bucket that you tried to create already exists, and you own it.",
            "case {region:?}: the conflict must carry the service's fixed sentence, got \
             {message:?}",
            message = repeat.message()
        );
    }
    assert_eq!(
        emulator.stored_count(),
        3,
        "a refused create must persist nothing, got {actual} buckets",
        actual = emulator.stored_count()
    );

    // Each region's bucket is still readable, and only through its own ARN.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let read = emulator
            .get_table_bucket(region, &expected_arn(region, NAME))
            .await;
        assert_eq!(
            read.status, 200,
            "case {region:?}: the bucket must still be readable, got {status} {body}",
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.text("arn"),
            expected_arn(region, NAME),
            "case {region:?}: a read must answer about the bucket its ARN names",
        );
    }
}

/// One refused `CreateTableBucket` request.
struct RefusalCase {
    label: &'static str,
    request: Value,
    /// The error code, where the rule being broken is the service's own one.
    /// `None` where the modeled constraint is refused at the boundary, whose
    /// code is the framework's to choose.
    expected_code: Option<&'static str>,
}

#[tokio::test]
async fn a_name_or_configuration_outside_the_rules_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    let too_short = "ab";
    let too_long = "a".repeat(64);

    let cases = vec![
        RefusalCase {
            label: "a leading hyphen",
            request: json!({ "name": "-leading-hyphen" }),
            expected_code: Some("BadRequestException"),
        },
        RefusalCase {
            label: "a trailing hyphen",
            request: json!({ "name": "trailing-hyphen-" }),
            expected_code: Some("BadRequestException"),
        },
        RefusalCase {
            label: "a hyphen at both ends",
            request: json!({ "name": "-both-" }),
            expected_code: Some("BadRequestException"),
        },
        RefusalCase {
            label: "nothing but hyphens",
            request: json!({ "name": "---" }),
            expected_code: Some("BadRequestException"),
        },
        RefusalCase {
            label: "SSE-KMS without a key",
            request: json!({
                "name": "kms-without-a-key",
                "encryptionConfiguration": { "sseAlgorithm": "aws:kms" },
            }),
            expected_code: Some("BadRequestException"),
        },
        RefusalCase {
            label: "SSE-S3 with a key",
            request: json!({
                "name": "aes-with-a-key",
                "encryptionConfiguration": {
                    "sseAlgorithm": "AES256",
                    "kmsKeyArn": KMS_KEY_ARN,
                },
            }),
            expected_code: Some("BadRequestException"),
        },
        RefusalCase {
            label: "one character under the modeled minimum length",
            request: json!({ "name": too_short }),
            expected_code: None,
        },
        RefusalCase {
            label: "one character over the modeled maximum length",
            request: json!({ "name": too_long }),
            expected_code: None,
        },
        RefusalCase {
            label: "a character the modeled pattern excludes",
            request: json!({ "name": "Not_Lowercase" }),
            expected_code: None,
        },
        RefusalCase {
            label: "a storage class this service has no storage for",
            request: json!({
                "name": "unknown-storage-class",
                "storageClassConfiguration": { "storageClass": "GLACIER" },
            }),
            expected_code: None,
        },
        RefusalCase {
            label: "an encryption algorithm the model does not declare",
            request: json!({
                "name": "unknown-algorithm",
                "encryptionConfiguration": { "sseAlgorithm": "ROT13" },
            }),
            expected_code: None,
        },
    ];

    for case in &cases {
        let refused = emulator.create_table_bucket("us-east-1", &case.request).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: the request must be refused as a bad request, got {status} {body}",
            label = case.label,
            status = refused.status,
            body = refused.body
        );
        if let Some(code) = case.expected_code {
            assert_eq!(
                refused.error_code.as_deref(),
                Some(code),
                "case {label:?}: the refusal must carry the modeled error code, got {actual:?}",
                label = case.label,
                actual = refused.error_code
            );
        }
        assert_eq!(
            emulator.stored_count(),
            0,
            "case {label:?}: a refused create must persist nothing",
            label = case.label
        );
    }

    // The names at the published bounds themselves are accepted: 3 and 63
    // characters, beginning and ending with a letter or a digit.
    for name in ["abc", &"a".repeat(63)] {
        let created = emulator
            .create_table_bucket("us-east-1", &json!({ "name": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name at a published bound must be accepted, got {status} {body}",
            status = created.status,
            body = created.body
        );
    }
    assert_eq!(
        emulator.stored_count(),
        2,
        "only the two accepted names may be resident, got {actual}",
        actual = emulator.stored_count()
    );
}

#[tokio::test]
async fn an_arn_no_create_minted_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_table_bucket("us-east-1", &json!({ "name": "the-one-that-exists" }))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture bucket must be created, got {status} {body}",
        status = created.status,
        body = created.body
    );

    let cases = [
        (
            "a name nothing created",
            expected_arn("us-east-1", "never-created"),
        ),
        (
            "the right name in another region",
            expected_arn("eu-west-1", "the-one-that-exists"),
        ),
        (
            "the right name under another account",
            "arn:aws:s3tables:us-east-1:123456789012:bucket/the-one-that-exists".to_string(),
        ),
    ];

    for (label, arn) in &cases {
        let read = emulator.get_table_bucket("us-east-1", arn).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: an unknown bucket must not be found, got {status} {body}",
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: the refusal must be the modeled not-found, got {actual:?}",
            actual = read.error_code
        );
    }
}

#[tokio::test]
async fn the_stored_enums_carry_exactly_the_modeled_values() {
    // The catalogs this service stores are the model's own, as the generated
    // value lists in `ops::types` state them.
    let wires = |values: &[&'static str]| -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    };

    let bucket_types: BTreeSet<String> = TableBucketType::ALL
        .iter()
        .map(|value| value.as_wire().to_string())
        .collect();
    assert_eq!(
        bucket_types,
        wires(TABLE_BUCKET_TYPE_VALUES),
        "case \"TableBucketType\": the stored values must be the modeled ones, got {bucket_types:?}"
    );

    let algorithms: BTreeSet<String> = SseAlgorithm::ALL
        .iter()
        .map(|value| value.as_wire().to_string())
        .collect();
    assert_eq!(
        algorithms,
        wires(S_S_E_ALGORITHM_VALUES),
        "case \"SSEAlgorithm\": the stored values must be the modeled ones, got {algorithms:?}"
    );

    let storage_classes: BTreeSet<String> = StorageClass::ALL
        .iter()
        .map(|value| value.as_wire().to_string())
        .collect();
    assert_eq!(
        storage_classes,
        wires(STORAGE_CLASS_VALUES),
        "case \"StorageClass\": the stored values must be the modeled ones, got {storage_classes:?}"
    );

    // Only `aws:kms` takes a key, which is the rule the handler enforces.
    for algorithm in SseAlgorithm::ALL {
        assert_eq!(
            algorithm.takes_kms_key(),
            *algorithm == SseAlgorithm::AwsKms,
            "case {wire:?}: only the KMS algorithm takes a key",
            wire = algorithm.as_wire()
        );
    }
}
