// SPDX-License-Identifier: CC0-1.0

//! Marketplace Commerce Analytics' two publication requests, end to end over
//! `awsJson1_1`.
//!
//! Every case here goes through the router, so it exercises routing, the
//! `X-Amz-Target` dispatch, deserialization, the handler, the persisted state
//! and serialization — the same path an SDK client takes.
//!
//! The rules under test, stated once:
//!
//! 1. Each accepted publication request is persisted under a freshly minted,
//!    unique `dataSetRequestId`, which is the value returned to the caller, and
//!    the record holds every input member exactly as it arrived — including the
//!    optional `destinationS3Prefix` and `customerDefinedValues`.
//! 2. The artifact a request would publish is recorded for the destination
//!    object its data set type, its date and its destination derive, under the
//!    file-name format that operation documents; requesting the same object
//!    again overwrites that artifact while still minting a new request id.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use aws_smithy_types::date_time::DateTime;
use serde_json::{json, Map, Value};
use service_com_amazonaws_marketplacecommerceanalytics_marketplacecommerceanalytics20150701::ops::types::{
    DATA_SET_TYPE_VALUES, SUPPORT_DATA_SET_TYPE_VALUES,
};
use service_com_amazonaws_marketplacecommerceanalytics_marketplacecommerceanalytics20150701::state::{
    ComAmazonawsMarketplacecommerceanalyticsMarketplacecommerceanalytics20150701State as McaState,
    DataSetRequest, PublishedArtifact, RequestKind, MAX_RECORDS_PER_STORE, MAX_S3_KEY_BYTES,
};
use service_com_amazonaws_marketplacecommerceanalytics_marketplacecommerceanalytics20150701::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service's
/// `awsJson1_1` dispatch.
const TARGET_PREFIX: &str = "MarketplaceCommerceAnalytics20150701";

/// A SigV4 credential scope naming this service's signing name
/// (`marketplacecommerceanalytics`); it is what routes the request here.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/marketplacecommerceanalytics/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// A role ARN with a permissions policy, as `roleNameArn` takes.
const ROLE: &str = "arn:aws:iam::123456789012:role/MarketplaceAnalyticsRole";

/// The topic `snsTopicArn` names.
const TOPIC: &str = "arn:aws:sns:us-east-1:123456789012:mca";

/// One emulator: a router carrying only this service over one shared engine,
/// plus a handle on that engine so a case can read what the handler persisted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { engine, router }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `dataSetRequestId` the answer carries, or the empty string.
    fn request_id(&self) -> String {
        self.body["dataSetRequestId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

impl Emulator {
    /// Call `operation` with `body` the way an SDK client does: `POST /` with
    /// the operation named in `X-Amz-Target`.
    async fn call(&self, operation: &str, body: &Value) -> Answer {
        let payload = body.to_string();
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "marketplacecommerceanalytics.us-east-1.amazonaws.com")
            .header("authorization", SIGNED)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(Bytes::from(payload))
            .expect("the test request is well formed");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }
}

/// True when `value` is a UUID in its canonical 8-4-4-4-12 hyphenated form,
/// carrying RFC 9562's version 8 (a custom, implementation-defined layout) and
/// its two-bit variant: the shape a `dataSetRequestId` carries. Spelled out here
/// rather than delegating to the crate, so a wrongly shaped id cannot pass.
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    if groups.len() != 5 {
        return false;
    }
    if [8, 4, 4, 4, 12]
        .iter()
        .zip(&groups)
        .any(|(want, got)| got.len() != *want)
    {
        return false;
    }
    if !value
        .chars()
        .all(|c| c == '-' || c.is_ascii_digit() || matches!(c, 'a'..='f'))
    {
        return false;
    }
    // Version nibble, then the two-bit RFC 9562 variant.
    groups[2].starts_with('8') && matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b')
}

/// `customerDefinedValues` as the request body carries it, and as the record is
/// expected to hold it.
fn values_body(pairs: &[(&str, &str)]) -> (Option<Value>, BTreeMap<String, String>) {
    let expected: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let wire = if pairs.is_empty() {
        None
    } else {
        let mut map = Map::new();
        for (k, v) in pairs {
            map.insert((*k).to_string(), Value::String((*v).to_string()));
        }
        Some(Value::Object(map))
    };
    (wire, expected)
}

/// One publication request, and the destination object it is expected to write.
struct Case {
    /// How a failing assertion names this row.
    name: &'static str,
    /// A `DataSetType` value, or a `SupportDataSetType` value for the support
    /// export table.
    data_set_type: &'static str,
    /// The epoch-second value sent as `dataSetPublicationDate` / `fromDate`,
    /// and the UTC instant it is: both are spelled out so the expected object
    /// name below can be read against them.
    date_epoch_seconds: i64,
    bucket: &'static str,
    prefix: Option<&'static str>,
    values: &'static [(&'static str, &'static str)],
    /// The bucket-relative key the artifact is expected under, written out in
    /// full rather than recomputed, so a wrong derivation fails.
    expected_key: &'static str,
}

/// `GenerateDataSet` publishes `{data_set_type}_YYYY-MM-DD.csv` (the operation's
/// own documentation), joined under `destinationS3Prefix` when one is given.
///
/// The rows differ along everything the rule generalises over: the data set
/// type (including types the published catalog lists beyond the cited one), the
/// date (epoch zero, a leap day, a year boundary, a mid-month instant), the
/// prefix (absent, nested, surrounded by slashes, empty) and the number of
/// `customerDefinedValues` (none, one, the modelled maximum of five).
const GENERATE_CASES: &[Case] = &[
    Case {
        name: "no prefix, one tracking value",
        data_set_type: "customer_profile_by_geography",
        // 2014-05-13T16:53:20Z
        date_epoch_seconds: 1_400_000_000,
        bucket: "my-bucket",
        prefix: None,
        values: &[("trackingId", "abc123")],
        expected_key: "customer_profile_by_geography_2014-05-13.csv",
    },
    Case {
        name: "nested prefix, no values",
        data_set_type: "monthly_revenue_billing_and_revenue_data",
        // 2016-01-01T00:00:00Z
        date_epoch_seconds: 1_451_606_400,
        bucket: "reports",
        prefix: Some("myprefix/mydatasets"),
        values: &[],
        expected_key: "myprefix/mydatasets/monthly_revenue_billing_and_revenue_data_2016-01-01.csv",
    },
    Case {
        name: "surrounding slashes are not part of the path",
        data_set_type: "us_sales_and_use_tax_records",
        // 2024-02-29T00:00:00Z — a leap day
        date_epoch_seconds: 1_709_164_800,
        bucket: "tax",
        prefix: Some("/quarterly/"),
        values: &[("quarter", "Q1"), ("owner", "finance")],
        expected_key: "quarterly/us_sales_and_use_tax_records_2024-02-29.csv",
    },
    Case {
        name: "empty prefix publishes to the bucket root",
        data_set_type: "disbursed_amount_by_product",
        // 2025-12-31T23:59:59Z — the last second of a year
        date_epoch_seconds: 1_767_225_599,
        bucket: "d",
        prefix: Some(""),
        values: &[
            ("a", "1"),
            ("b", "2"),
            ("c", "3"),
            ("d", "4"),
            ("e", "5"),
        ],
        expected_key: "disbursed_amount_by_product_2025-12-31.csv",
    },
    Case {
        name: "the epoch itself",
        data_set_type: "daily_business_fees",
        // 1970-01-01T00:00:00Z
        date_epoch_seconds: 0,
        bucket: "epoch-bucket",
        prefix: Some("a/b/c"),
        values: &[],
        expected_key: "a/b/c/daily_business_fees_1970-01-01.csv",
    },
];

/// `StartSupportDataExport` documents a DIFFERENT file name from
/// `GenerateDataSet`'s: `{data_set_type}_YYYY-MM-DD'T'HH-mm-ss'Z'.csv`, down to
/// the second of the requested `fromDate`.
const SUPPORT_CASES: &[Case] = &[
    Case {
        name: "test data set, no prefix, no values",
        data_set_type: "test_customer_support_contacts_data",
        // 2014-05-13T16:53:20Z
        date_epoch_seconds: 1_400_000_000,
        bucket: "my-bucket",
        prefix: None,
        values: &[],
        expected_key: "test_customer_support_contacts_data_2014-05-13T16-53-20Z.csv",
    },
    Case {
        name: "live data set, nested prefix, values echoed",
        data_set_type: "customer_support_contacts_data",
        // 2025-12-31T23:59:59Z
        date_epoch_seconds: 1_767_225_599,
        bucket: "support-exports",
        prefix: Some("contacts/2025"),
        values: &[("runBy", "nightly"), ("attempt", "2")],
        expected_key: "contacts/2025/customer_support_contacts_data_2025-12-31T23-59-59Z.csv",
    },
    Case {
        name: "the epoch itself, slash-wrapped prefix",
        data_set_type: "customer_support_contacts_data",
        // 1970-01-01T00:00:00Z
        date_epoch_seconds: 0,
        bucket: "epoch-bucket",
        prefix: Some("/from-the-start/"),
        values: &[("why", "backfill")],
        expected_key: "from-the-start/customer_support_contacts_data_1970-01-01T00-00-00Z.csv",
    },
];

/// Build the request body for `case`, dating it with `member`.
fn body_for(case: &Case, member: &str, values: Option<Value>) -> Value {
    let mut body = json!({
        "dataSetType": case.data_set_type,
        "roleNameArn": ROLE,
        "destinationS3BucketName": case.bucket,
        "snsTopicArn": TOPIC,
    });
    body[member] = json!(case.date_epoch_seconds);
    if let Some(prefix) = case.prefix {
        body["destinationS3Prefix"] = json!(prefix);
    }
    if let Some(values) = values {
        body["customerDefinedValues"] = values;
    }
    body
}

/// Run one table and assert both rules over every row.
async fn run_table(cases: &'static [Case], operation: &str, date_member: &str, kind: RequestKind) {
    let emulator = emulator();
    let mut seen: HashSet<String> = HashSet::new();

    for case in cases {
        let (wire_values, expected_values) = values_body(case.values);
        let body = body_for(case, date_member, wire_values);

        let before = SystemTime::now();
        let answer = emulator.call(operation, &body).await;
        let after = SystemTime::now();

        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with body {}",
            case.name, answer.status, answer.body
        );
        let id = answer.request_id();
        assert!(
            is_uuid_shaped(&id),
            "case {:?}: expected a UUID-shaped dataSetRequestId, got {id:?}",
            case.name
        );
        assert!(
            seen.insert(id.clone()),
            "case {:?}: dataSetRequestId {id:?} was already minted for an earlier request",
            case.name
        );

        let store = emulator.engine.state::<McaState>();
        let store = store.lock();

        let record = store.requests.get(&id).unwrap_or_else(|| {
            panic!(
                "case {:?}: dataSetRequestId {id:?} is not findable in state (ids held: {:?})",
                case.name, store.request_order
            )
        });
        assert_eq!(
            record.kind, kind,
            "case {:?}: expected the record to name {kind:?}, got {:?}",
            case.name, record.kind
        );
        assert_eq!(
            record.data_set_type, case.data_set_type,
            "case {:?}: expected dataSetType {:?}, got {:?}",
            case.name, case.data_set_type, record.data_set_type
        );
        assert_eq!(
            record.role_name_arn, ROLE,
            "case {:?}: expected roleNameArn {ROLE:?}, got {:?}",
            case.name, record.role_name_arn
        );
        assert_eq!(
            record.destination_s3_bucket_name, case.bucket,
            "case {:?}: expected destinationS3BucketName {:?}, got {:?}",
            case.name, case.bucket, record.destination_s3_bucket_name
        );
        assert_eq!(
            record.destination_s3_prefix.as_deref(),
            case.prefix,
            "case {:?}: expected destinationS3Prefix stored verbatim as {:?}, got {:?}",
            case.name,
            case.prefix,
            record.destination_s3_prefix
        );
        assert_eq!(
            record.sns_topic_arn, TOPIC,
            "case {:?}: expected snsTopicArn {TOPIC:?}, got {:?}",
            case.name, record.sns_topic_arn
        );
        assert_eq!(
            record.customer_defined_values, expected_values,
            "case {:?}: expected customerDefinedValues {expected_values:?}, got {:?}",
            case.name, record.customer_defined_values
        );

        // The dated member the operation takes, and only that one.
        let (dated, undated) = match kind {
            RequestKind::GenerateDataSet => (&record.publication_date, &record.from_date),
            RequestKind::StartSupportDataExport => (&record.from_date, &record.publication_date),
        };
        assert_eq!(
            dated.map(|d| d.secs()),
            Some(case.date_epoch_seconds),
            "case {:?}: expected {date_member} {} to be stored, got {dated:?}",
            case.name,
            case.date_epoch_seconds
        );
        assert!(
            undated.is_none(),
            "case {:?}: the other operation's date member must stay unset, got {undated:?}",
            case.name
        );
        assert!(
            record.created_at >= before && record.created_at <= after,
            "case {:?}: created_at must be the instant the call was accepted, got {:?}",
            case.name,
            record.created_at
        );

        assert_eq!(
            store.newest_request_id(),
            Some(id.as_str()),
            "case {:?}: the id returned must be the newest recorded, got {:?}",
            case.name,
            store.newest_request_id()
        );

        let artifact_key = format!("{}/{}", case.bucket, case.expected_key);
        let artifact = store.published.get(&artifact_key).unwrap_or_else(|| {
            let mut keys: Vec<&String> = store.published.keys().collect();
            keys.sort();
            panic!(
                "case {:?}: expected a published artifact at {artifact_key:?}, holds {keys:?}",
                case.name
            )
        });
        assert_eq!(
            artifact.s3_key, case.expected_key,
            "case {:?}: expected object key {:?}, got {:?}",
            case.name, case.expected_key, artifact.s3_key
        );
        assert_eq!(
            artifact.bucket, case.bucket,
            "case {:?}: expected bucket {:?}, got {:?}",
            case.name, case.bucket, artifact.bucket
        );
        assert_eq!(
            artifact.sns_topic_arn, TOPIC,
            "case {:?}: expected the artifact to notify {TOPIC:?}, got {:?}",
            case.name, artifact.sns_topic_arn
        );
        assert_eq!(
            artifact.customer_defined_values, expected_values,
            "case {:?}: expected the artifact to echo {expected_values:?}, got {:?}",
            case.name, artifact.customer_defined_values
        );
        assert_eq!(
            artifact.last_request_id, id,
            "case {:?}: expected the artifact to name request {id:?}, got {:?}",
            case.name, artifact.last_request_id
        );
    }

    let store = emulator.engine.state::<McaState>();
    let store = store.lock();
    assert_eq!(
        store.requests.len(),
        cases.len(),
        "expected one record per request, got {} for {} requests",
        store.requests.len(),
        cases.len()
    );
}

#[tokio::test]
async fn generate_data_set_records_every_request_and_its_published_object() {
    run_table(
        GENERATE_CASES,
        "GenerateDataSet",
        "dataSetPublicationDate",
        RequestKind::GenerateDataSet,
    )
    .await;
}

#[tokio::test]
async fn start_support_data_export_records_every_request_and_its_published_object() {
    run_table(
        SUPPORT_CASES,
        "StartSupportDataExport",
        "fromDate",
        RequestKind::StartSupportDataExport,
    )
    .await;
}

/// Every value the model's `DataSetType` / `SupportDataSetType` enums publish is
/// accepted, and each names its own object.
///
/// The catalogues come from the crate's model-derived constants
/// (`ops::types::DATA_SET_TYPE_VALUES` / `SUPPORT_DATA_SET_TYPE_VALUES`), so no
/// value can drift out of this case.
#[tokio::test]
async fn every_published_data_set_type_is_accepted_and_names_its_own_object() {
    let emulator = emulator();
    // 2014-05-13T16:53:20Z
    let epoch_seconds = 1_400_000_000_i64;
    let mut ids: HashSet<String> = HashSet::new();

    for data_set_type in DATA_SET_TYPE_VALUES {
        let body = json!({
            "dataSetType": data_set_type,
            "dataSetPublicationDate": epoch_seconds,
            "roleNameArn": ROLE,
            "destinationS3BucketName": "catalog",
            "snsTopicArn": TOPIC,
        });
        let answer = emulator.call("GenerateDataSet", &body).await;
        assert_eq!(
            answer.status, 200,
            "case {data_set_type:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            ids.insert(answer.request_id()),
            "case {data_set_type:?}: dataSetRequestId {:?} repeats an earlier one",
            answer.request_id()
        );

        let store = emulator.engine.state::<McaState>();
        let store = store.lock();
        let key = format!("catalog/{data_set_type}_2014-05-13.csv");
        assert!(
            store.published.contains_key(&key),
            "case {data_set_type:?}: expected an artifact at {key:?}, holds {} objects",
            store.published.len()
        );
    }

    for data_set_type in SUPPORT_DATA_SET_TYPE_VALUES {
        let body = json!({
            "dataSetType": data_set_type,
            "fromDate": epoch_seconds,
            "roleNameArn": ROLE,
            "destinationS3BucketName": "catalog",
            "snsTopicArn": TOPIC,
        });
        let answer = emulator.call("StartSupportDataExport", &body).await;
        assert_eq!(
            answer.status, 200,
            "case {data_set_type:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            ids.insert(answer.request_id()),
            "case {data_set_type:?}: dataSetRequestId {:?} repeats an earlier one",
            answer.request_id()
        );

        let store = emulator.engine.state::<McaState>();
        let store = store.lock();
        let key = format!("catalog/{data_set_type}_2014-05-13T16-53-20Z.csv");
        assert!(
            store.published.contains_key(&key),
            "case {data_set_type:?}: expected an artifact at {key:?}, holds {} objects",
            store.published.len()
        );
    }

    let expected = DATA_SET_TYPE_VALUES.len() + SUPPORT_DATA_SET_TYPE_VALUES.len();
    assert_eq!(
        ids.len(),
        expected,
        "expected one distinct id per published data set type, got {} for {expected} types",
        ids.len()
    );
}

/// Requesting the same object again mints a NEW request id, keeps both request
/// records, and overwrites the one artifact that object holds — "If a file with
/// the same name already exists (e.g. if the same data set is requested twice),
/// the original file will be overwritten by the new file."
#[tokio::test]
async fn re_requesting_the_same_object_overwrites_its_artifact_and_mints_a_new_id() {
    let emulator = emulator();

    // Same data set, same publication date, same destination — the second
    // request differs only in the values it asks to be echoed back.
    let first = json!({
        "dataSetType": "customer_profile_by_geography",
        "dataSetPublicationDate": 1_400_000_000_i64,
        "roleNameArn": ROLE,
        "destinationS3BucketName": "my-bucket",
        "destinationS3Prefix": "myprefix/mydatasets",
        "snsTopicArn": TOPIC,
        "customerDefinedValues": {"attempt": "1"},
    });
    let mut second = first.clone();
    second["customerDefinedValues"] = json!({"attempt": "2"});

    // A third request differs in the DAY, so it is a different object and must
    // not disturb the first one's artifact. 1_400_086_400 is 2014-05-14T16:53:20Z.
    let mut other_day = first.clone();
    other_day["dataSetPublicationDate"] = json!(1_400_086_400_i64);
    other_day["customerDefinedValues"] = json!({"attempt": "3"});

    let answers = [
        ("first", &first),
        ("second", &second),
        ("other day", &other_day),
    ];
    let mut ids = Vec::new();
    for (name, body) in answers {
        let answer = emulator.call("GenerateDataSet", body).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        ids.push(answer.request_id());
    }

    assert!(
        ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2],
        "expected three distinct dataSetRequestIds, got {ids:?}"
    );

    let store = emulator.engine.state::<McaState>();
    let store = store.lock();

    assert_eq!(
        store.request_order, ids,
        "expected the records in the order the requests arrived, got {:?}",
        store.request_order
    );

    let shared = "my-bucket/myprefix/mydatasets/customer_profile_by_geography_2014-05-13.csv";
    let artifact = store
        .published
        .get(shared)
        .unwrap_or_else(|| panic!("expected an artifact at {shared:?}"));
    assert_eq!(
        artifact.last_request_id, ids[1],
        "expected the re-request {:?} to own the object, got {:?}",
        ids[1], artifact.last_request_id
    );
    assert_eq!(
        artifact.customer_defined_values.get("attempt").map(String::as_str),
        Some("2"),
        "expected the newer request's values on the overwritten object, got {:?}",
        artifact.customer_defined_values
    );

    let other = "my-bucket/myprefix/mydatasets/customer_profile_by_geography_2014-05-14.csv";
    assert!(
        store.published.contains_key(other),
        "a different publication date is a different object; expected {other:?} too"
    );
    assert_eq!(
        store.published.len(),
        2,
        "expected two distinct objects for three requests, got {}",
        store.published.len()
    );
    assert_eq!(
        store.requests.len(),
        3,
        "an overwritten object must not drop a request record, got {}",
        store.requests.len()
    );
}

/// A request the model refuses is refused as a 400 `ValidationException`, never
/// as a 501: the operations are implemented, so "not written yet" is never the
/// right answer.
#[tokio::test]
async fn invalid_requests_are_refused_rather_than_answered_501() {
    let emulator = emulator();

    let valid = json!({
        "dataSetType": "customer_profile_by_geography",
        "dataSetPublicationDate": 1_400_000_000_i64,
        "roleNameArn": ROLE,
        "destinationS3BucketName": "my-bucket",
        "snsTopicArn": TOPIC,
    });

    // Each row drops or breaks ONE modelled constraint of the same valid body.
    let mut unknown_type = valid.clone();
    unknown_type["dataSetType"] = json!("not_a_data_set_type");

    let mut missing_topic = valid.clone();
    missing_topic
        .as_object_mut()
        .expect("the body is an object")
        .remove("snsTopicArn");

    let mut empty_bucket = valid.clone();
    empty_bucket["destinationS3BucketName"] = json!("");

    let mut too_many_values = valid.clone();
    too_many_values["customerDefinedValues"] =
        json!({"a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6"});

    // `DataSetType` is an enum, so a support-only value is not a data set type.
    let mut wrong_enum = valid.clone();
    wrong_enum["dataSetType"] = json!("customer_support_contacts_data");

    // The published object is named for the publication date's calendar day, so
    // an instant outside any calendar date names no data set file.
    let mut unrenderable_date = valid.clone();
    unrenderable_date["dataSetPublicationDate"] = json!(1_000_000_000_000_000_000_i64);

    for (name, body) in [
        ("unknown dataSetType", &unknown_type),
        ("absent required snsTopicArn", &missing_topic),
        ("destinationS3BucketName below its minimum length", &empty_bucket),
        ("customerDefinedValues above its maximum size", &too_many_values),
        ("a SupportDataSetType value as a DataSetType", &wrong_enum),
        ("a dataSetPublicationDate outside any calendar date", &unrenderable_date),
    ] {
        let answer = emulator.call("GenerateDataSet", body).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().contains("ValidationException"),
            "case {name:?}: expected a ValidationException, got {:?}",
            answer.error_code()
        );
    }

    let store = emulator.engine.state::<McaState>();
    let store = store.lock();
    assert!(
        store.requests.is_empty() && store.published.is_empty(),
        "a refused request must persist nothing, got {} records and {} objects",
        store.requests.len(),
        store.published.len()
    );
}

/// `destinationS3BucketName` carries only a length bound in the model, so which
/// names exist is S3's rule: a bucket name "can consist only of lowercase
/// letters, numbers, dots (.), and hyphens (-)", "must begin and end with a
/// letter or number", "must not contain two adjacent periods" and "must not be
/// formatted as an IP address" (Amazon S3 User Guide, bucket naming rules).
/// One row per published clause, plus names the rules allow — including a
/// one-character name, since the model's 1..63 length bound is the model's to
/// enforce and a handler must not refuse what it accepts.
#[tokio::test]
async fn s3_bucket_naming_rules_decide_which_destination_buckets_exist() {
    let emulator = emulator();

    for (name, bucket, expected_status) in [
        ("a hyphenated name", "my-bucket", 200_u16),
        ("dots separate labels", "logs.example.data", 200),
        ("digits are allowed", "bucket1", 200),
        ("one character, as the model's length bound allows", "d", 200),
        ("a slash is not in the character class", "my-bucket/nested", 400),
        ("an uppercase letter is not", "MyBucket", 400),
        ("an underscore is not", "my_bucket", 400),
        ("a leading hyphen does not begin alphanumeric", "-leading", 400),
        ("a trailing dot does not end alphanumeric", "trailing.", 400),
        ("two adjacent periods", "bucket..dots", 400),
        ("formatted as an IP address", "192.168.5.4", 400),
    ] {
        let body = json!({
            "dataSetType": "daily_business_fees",
            "dataSetPublicationDate": 0,
            "roleNameArn": ROLE,
            "destinationS3BucketName": bucket,
            "snsTopicArn": TOPIC,
        });
        let answer = emulator.call("GenerateDataSet", &body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {name:?} ({bucket:?}): expected HTTP {expected_status}, got {} with body {}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert!(
                answer.error_code().contains("ValidationException"),
                "case {name:?} ({bucket:?}): expected a ValidationException, got {:?}",
                answer.error_code()
            );
        } else {
            let store = emulator.engine.state::<McaState>();
            let store = store.lock();
            let key = format!("{bucket}/daily_business_fees_1970-01-01.csv");
            assert!(
                store.published.contains_key(&key),
                "case {name:?} ({bucket:?}): expected an artifact at {key:?}, holds {} objects",
                store.published.len()
            );
        }
    }

    let store = emulator.engine.state::<McaState>();
    let store = store.lock();
    assert_eq!(
        store.requests.len(),
        4,
        "only the four legal bucket names may have been recorded, got {} records",
        store.requests.len()
    );
}

/// `destinationS3Prefix` carries no modelled length bound, but S3 stores an
/// object key of at most 1,024 bytes (Amazon S3 object key naming guidelines,
/// reproduced on `state::MAX_S3_KEY_BYTES`), so a prefix that pushes the derived
/// key past that bound names an object no publication could write.
///
/// Tested on the published bound, one byte under it, and one byte and 500 bytes
/// past it.
#[tokio::test]
async fn a_prefix_that_overruns_the_s3_object_key_limit_is_refused() {
    let emulator = emulator();
    // `dataSetPublicationDate` 0 is 1970-01-01T00:00:00Z, so the file name is
    // this, and the key is `{prefix}/{object_name}`.
    let object_name = "daily_business_fees_1970-01-01.csv";
    let room = MAX_S3_KEY_BYTES - object_name.len() - "/".len();

    for (name, prefix_len, expected_status) in [
        ("one byte under the S3 object key limit", room - 1, 200_u16),
        ("exactly at the S3 object key limit", room, 200),
        ("one byte past the S3 object key limit", room + 1, 400),
        ("far past the S3 object key limit", room + 500, 400),
    ] {
        let prefix = "p".repeat(prefix_len);
        let body = json!({
            "dataSetType": "daily_business_fees",
            "dataSetPublicationDate": 0,
            "roleNameArn": ROLE,
            "destinationS3BucketName": "keys",
            "destinationS3Prefix": prefix,
            "snsTopicArn": TOPIC,
        });
        let answer = emulator.call("GenerateDataSet", &body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {name:?}: expected HTTP {expected_status}, got {} with body {}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert!(
                answer.error_code().contains("ValidationException"),
                "case {name:?}: expected a ValidationException, got {:?}",
                answer.error_code()
            );
        } else {
            let key = format!("{prefix}/{object_name}");
            let store = emulator.engine.state::<McaState>();
            let store = store.lock();
            assert!(
                store.published.contains_key(&format!("keys/{key}")),
                "case {name:?}: expected an artifact under a {}-byte key, holds {} objects",
                key.len(),
                store.published.len()
            );
        }
    }
}

/// Nothing deletes an accepted request — the service models no delete — so the
/// request store is bounded, and a request that would cross the ceiling is
/// refused with the service's only modelled fault
/// (`MarketplaceCommerceAnalyticsException`, HTTP 500) rather than growing the
/// process. Both operations share the one store, so both are refused.
#[tokio::test]
async fn the_request_store_is_bounded_and_refuses_with_the_modelled_fault() {
    let emulator = emulator();

    // Every request names the SAME destination object, so `published` holds one
    // entry throughout and the record ceiling is what refuses.
    let body = json!({
        "dataSetType": "customer_profile_by_geography",
        "dataSetPublicationDate": 1_400_000_000_i64,
        "roleNameArn": ROLE,
        "destinationS3BucketName": "my-bucket",
        "snsTopicArn": TOPIC,
    });
    for accepted in 0..MAX_RECORDS_PER_STORE {
        let answer = emulator.call("GenerateDataSet", &body).await;
        assert_eq!(
            answer.status, 200,
            "case {accepted:?} requests in: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    for (name, operation, body) in [
        ("GenerateDataSet at the ceiling", "GenerateDataSet", &body),
        (
            "StartSupportDataExport at the ceiling",
            "StartSupportDataExport",
            &json!({
                "dataSetType": "customer_support_contacts_data",
                "fromDate": 1_400_000_000_i64,
                "roleNameArn": ROLE,
                "destinationS3BucketName": "my-bucket",
                "snsTopicArn": TOPIC,
            }),
        ),
    ] {
        let answer = emulator.call(operation, body).await;
        assert_eq!(
            answer.status, 500,
            "case {name:?}: expected HTTP 500, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error_code()
                .contains("MarketplaceCommerceAnalyticsException"),
            "case {name:?}: expected the modelled fault, got {:?}",
            answer.error_code()
        );
    }

    let store = emulator.engine.state::<McaState>();
    let store = store.lock();
    assert_eq!(
        store.requests.len(),
        MAX_RECORDS_PER_STORE,
        "a refused request must not grow the store, got {} records",
        store.requests.len()
    );
    assert_eq!(
        store.published.len(),
        1,
        "one destination object was ever named, got {} objects",
        store.published.len()
    );
}

/// The published-object store is bounded on its own: its key embeds the
/// caller-supplied bucket and prefix, so how many distinct keys exist is the
/// caller's choice. At the ceiling a NEW destination object is refused, while
/// re-requesting an object the store already holds still overwrites it — the
/// bound is on growth, not on publication.
#[test]
fn the_published_store_is_bounded_independently_of_the_request_store() {
    let mut state = McaState::default();
    for seeded in 0..MAX_RECORDS_PER_STORE {
        let object = format!("seeded_{seeded}.csv");
        state.published.insert(
            format!("bucket/{object}"),
            PublishedArtifact {
                s3_key: object,
                bucket: "bucket".to_string(),
                sns_topic_arn: TOPIC.to_string(),
                customer_defined_values: BTreeMap::new(),
                last_request_id: format!("seed-{seeded}"),
            },
        );
    }

    let request = |id: &str, bucket: &str| DataSetRequest {
        data_set_request_id: id.to_string(),
        kind: RequestKind::GenerateDataSet,
        data_set_type: "daily_business_fees".to_string(),
        publication_date: Some(DateTime::from_secs(0)),
        from_date: None,
        role_name_arn: ROLE.to_string(),
        destination_s3_bucket_name: bucket.to_string(),
        destination_s3_prefix: None,
        sns_topic_arn: TOPIC.to_string(),
        customer_defined_values: BTreeMap::new(),
        created_at: SystemTime::now(),
    };

    let refused = state.accept(request("new", "another-bucket"), "new.csv".to_string());
    assert!(
        refused.is_err(),
        "case \"a new destination object at the ceiling\": expected a refusal, got {refused:?}"
    );
    assert!(
        state.requests.is_empty(),
        "case \"a new destination object at the ceiling\": a refusal must write nothing, got {} records",
        state.requests.len()
    );

    let accepted = state.accept(request("overwrite", "bucket"), "seeded_0.csv".to_string());
    assert_eq!(
        accepted.as_deref(),
        Ok("overwrite"),
        "case \"an object the store already holds\": expected the request id back, got {accepted:?}"
    );
    assert_eq!(
        state.published.len(),
        MAX_RECORDS_PER_STORE,
        "case \"an object the store already holds\": an overwrite must not grow the store, got {} objects",
        state.published.len()
    );
    assert_eq!(
        state.published["bucket/seeded_0.csv"].last_request_id,
        "overwrite",
        "case \"an object the store already holds\": expected the newer request to own it, got {:?}",
        state.published["bucket/seeded_0.csv"].last_request_id
    );
}
