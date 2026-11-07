// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members a Glue answer reports where the value they carry is a
//! default, a sentinel or an empty collection.
//!
//! The rule under test: a response carries every member of a reported structure the service
//! holds a value for, and a value that happens to be a default, a sentinel or an empty
//! collection is STILL a value — an empty list reported as `[]`, a kind's details structure
//! with nothing inside it as `{}`. Absence is reserved for a member the service holds no
//! value for, so a client can tell "empty" from "not reported" and SDK code that
//! dereferences the member works. [`crate::response_members`] asserts the same rule for the
//! one case of a listing's collection member; this suite is that rule over the members of
//! the records those listings and reads report.
//!
//! Every call goes over the wire the way a client makes it, through the shared
//! [`crate::wire`] harness: `POST /` with `X-Amz-Target: AWSGlue.<Operation>` and an
//! `awsJson1_1` body, dispatched by the same `MultiServiceRouter` the production front door
//! uses — so what is asserted here is HTTP behaviour and not a direct call into a handler.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which value
//! the member carries, how many items its collection holds, which members the write stated
//! and which it left to the service, and which read reports them — a single read, a listing
//! and a batch read all project the same stored record.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::partitions::seed_table;
use crate::wire::{call, emulator, ok};

/// The values `CsvSerdeOption` admits, the published list of
/// `smithy_query shape=CsvSerdeOption`.
const CSV_SERDE_OPTIONS: [&str; 3] = ["OpenCSVSerDe", "LazySimpleSerDe", "None"];

/// The `CsvSerdeOption` a CSV classifier carries when its write named none: "you can specify
/// the `None` value when you want the crawler to do the detection" (`CsvClassifier$Serde`),
/// which is the choice a classifier that states no SerDe is left at.
const SERDE_WHEN_UNSTATED: &str = "None";

/// The state of encrypted catalog search the Data Catalog reports beside its encryption
/// settings. No operation of this service turns it on, so every catalog reports this one.
const ENCRYPTED_CATALOG_SEARCH_STATUS: &str = "NOT_ENABLED";

/// A KMS key ARN of this emulator's account, the form a client names a key by.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The `Classifier` member `kind` of the classifier `name`, as `GetClassifier` reports it.
async fn classifier(router: &MultiServiceRouter, name: &str, kind: &str) -> Value {
    let body = ok(router, "GetClassifier", &json!({ "Name": name })).await;
    body["Classifier"][kind].clone()
}

/// The entry for `name` in the `GetClassifiers` listing, under its kind's member.
async fn listed_classifier(router: &MultiServiceRouter, name: &str, kind: &str) -> Value {
    let body = ok(router, "GetClassifiers", &json!({})).await;
    let listed = body["Classifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("GetClassifiers must report a Classifiers list, got {body}"));
    listed
        .iter()
        .map(|entry| entry[kind].clone())
        .find(|entry| entry.get("Name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("the listing must report the classifier {name:?}, got {body}"))
}

/// A CSV classifier reports the SerDe it processes CSV with, whether the write stated one or
/// left it at the detection option — and the listing reports the same value the single read
/// does.
///
/// The rows differ along the value the member carries: each published `CsvSerdeOption`, a
/// write that states none at all, and a write that states one beside every other CSV member.
#[tokio::test]
async fn a_csv_classifier_reports_the_serde_it_processes_csv_with() {
    let router = emulator();

    // (case, the CsvClassifier the create carries, the Serde the reads must report)
    let mut rows: Vec<(String, Value, &str)> = vec![
        (
            // The recorded shape: a CSV classifier that states no SerDe.
            "no SerDe stated".to_string(),
            json!({ "Name": "csv-unstated" }),
            SERDE_WHEN_UNSTATED,
        ),
        (
            "no SerDe stated beside other members".to_string(),
            json!({
                "Name": "csv-unstated-with-members",
                "Delimiter": "|",
                "ContainsHeader": "PRESENT",
                "Header": ["id", "amount"],
            }),
            SERDE_WHEN_UNSTATED,
        ),
        (
            "every other CSV member stated beside a SerDe".to_string(),
            json!({
                "Name": "csv-every-member",
                "Delimiter": "\t",
                "QuoteSymbol": "'",
                "ContainsHeader": "ABSENT",
                "Header": ["id", "region", "amount"],
                "DisableValueTrimming": true,
                "AllowSingleColumn": false,
                "CustomDatatypeConfigured": true,
                "CustomDatatypes": ["BINARY", "DECIMAL"],
                "Serde": "OpenCSVSerDe",
            }),
            "OpenCSVSerDe",
        ),
    ];
    // Every published value of the enum, including the one the service also reports when the
    // write states none: stating it explicitly reads back the same way.
    for option in CSV_SERDE_OPTIONS {
        rows.push((
            format!("the SerDe stated as {option}"),
            json!({ "Name": format!("csv-{option}"), "Serde": option }),
            option,
        ));
    }

    for (case, csv, expected) in &rows {
        let name = csv["Name"].as_str().expect("the row names its classifier");
        ok(&router, "CreateClassifier", &json!({ "CsvClassifier": csv })).await;

        let read = classifier(&router, name, "CsvClassifier").await;
        assert_eq!(
            read.get("Serde").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected Serde {expected:?}, got {read}"
        );

        let listed = listed_classifier(&router, name, "CsvClassifier").await;
        assert_eq!(
            listed, read,
            "case {case:?}: the listing must report the entry the single read does, got \
             {listed}"
        );
    }
}

/// `Serde` belongs to the CSV classifier alone: a classifier of another kind holds no value
/// for it, so the kind's structure carries no such member.
///
/// The rows are the other three published kinds, each addressed through the member of
/// `Classifier` its create filled.
#[tokio::test]
async fn a_classifier_of_another_kind_reports_no_serde() {
    let router = emulator();

    // (case, the kind's member of Classifier, the create body)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a grok classifier",
            "GrokClassifier",
            json!({
                "GrokClassifier": {
                    "Name": "kind-grok",
                    "Classification": "logs",
                    "GrokPattern": "%{GREEDYDATA:line}",
                },
            }),
        ),
        (
            "an XML classifier",
            "XMLClassifier",
            json!({
                "XMLClassifier": {
                    "Name": "kind-xml",
                    "Classification": "xml",
                    "RowTag": "record",
                },
            }),
        ),
        (
            "a JSON classifier",
            "JsonClassifier",
            json!({
                "JsonClassifier": { "Name": "kind-json", "JsonPath": "$.records[*]" },
            }),
        ),
    ];

    for (case, kind, request) in &rows {
        let name = request[kind]["Name"]
            .as_str()
            .expect("the row names its classifier");
        ok(&router, "CreateClassifier", request).await;

        let read = classifier(&router, name, kind).await;
        assert!(
            read.get("Serde").is_none(),
            "case {case:?}: {kind} holds no SerDe, so none may be reported, got {read}"
        );
    }
}

/// One row of the storage-descriptor table: what the write carried as its
/// `StorageDescriptor`, and the column list the reads must report for it.
type DescriptorRow = (&'static str, Value, Value);

/// The rows both storage-descriptor suites drive. They differ along how many columns the
/// descriptor lists — none at all, an empty list, one and several — and along which other
/// members the write stated beside them.
fn descriptor_rows() -> Vec<DescriptorRow> {
    vec![
        (
            // The recorded shape: a descriptor stating a location and no column.
            "a descriptor carrying no Columns member",
            json!({ "Location": "s3://sim2real-glue/no-columns/" }),
            json!([]),
        ),
        (
            "a descriptor carrying an empty Columns list",
            json!({ "Columns": [], "Location": "s3://sim2real-glue/empty-columns/" }),
            json!([]),
        ),
        (
            "a descriptor carrying one column",
            json!({ "Columns": [{ "Name": "id", "Type": "string" }] }),
            json!([{ "Name": "id", "Type": "string" }]),
        ),
        (
            "a descriptor carrying three columns",
            json!({
                "Columns": [
                    { "Name": "id", "Type": "string" },
                    { "Name": "amount", "Type": "double" },
                    { "Name": "recorded_at", "Type": "timestamp" },
                ],
                "Location": "s3://sim2real-glue/three-columns/",
                "InputFormat": "org.apache.hadoop.mapred.TextInputFormat",
            }),
            json!([
                { "Name": "id", "Type": "string" },
                { "Name": "amount", "Type": "double" },
                { "Name": "recorded_at", "Type": "timestamp" },
            ]),
        ),
    ]
}

/// The `Columns` member of `descriptor`, which has to be there.
fn reported_columns(case: &str, descriptor: &Value) -> Value {
    descriptor.get("Columns").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the descriptor must report Columns, got {descriptor}")
    })
}

/// A partition's storage descriptor reports its columns as a LIST, empty when the descriptor
/// lists none — and both the single read and the batch read report it, because the two
/// project the same stored partition.
#[tokio::test]
async fn a_partition_storage_descriptor_reports_its_columns_as_a_list() {
    const DATABASE: &str = "reported_members_db";
    const TABLE: &str = "reported_members_tbl";

    for (case, descriptor, expected) in descriptor_rows() {
        let router = emulator();
        seed_table(&router, DATABASE, TABLE, &["day"]).await;
        ok(
            &router,
            "CreatePartition",
            &json!({
                "DatabaseName": DATABASE,
                "TableName": TABLE,
                "PartitionInput": { "Values": ["2026-01-01"], "StorageDescriptor": descriptor },
            }),
        )
        .await;

        let read = ok(
            &router,
            "GetPartition",
            &json!({
                "DatabaseName": DATABASE,
                "TableName": TABLE,
                "PartitionValues": ["2026-01-01"],
            }),
        )
        .await;
        assert_eq!(
            reported_columns(case, &read["Partition"]["StorageDescriptor"]),
            expected,
            "case {case:?}: GetPartition must report {expected}, got {read}"
        );

        let batch = ok(
            &router,
            "BatchGetPartition",
            &json!({
                "DatabaseName": DATABASE,
                "TableName": TABLE,
                "PartitionsToGet": [{ "Values": ["2026-01-01"] }],
            }),
        )
        .await;
        assert_eq!(
            reported_columns(case, &batch["Partitions"][0]["StorageDescriptor"]),
            expected,
            "case {case:?}: BatchGetPartition must report {expected}, got {batch}"
        );
    }
}

/// A table's storage descriptor is the same structure, projected the same way: its columns
/// are reported as a list, empty when the descriptor lists none.
#[tokio::test]
async fn a_table_storage_descriptor_reports_its_columns_as_a_list() {
    const DATABASE: &str = "reported_members_table_db";

    for (case, descriptor, expected) in descriptor_rows() {
        let router = emulator();
        ok(
            &router,
            "CreateDatabase",
            &json!({ "DatabaseInput": { "Name": DATABASE } }),
        )
        .await;
        ok(
            &router,
            "CreateTable",
            &json!({
                "DatabaseName": DATABASE,
                "TableInput": { "Name": "described", "StorageDescriptor": descriptor },
            }),
        )
        .await;

        let read = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": DATABASE, "Name": "described" }),
        )
        .await;
        assert_eq!(
            reported_columns(case, &read["Table"]["StorageDescriptor"]),
            expected,
            "case {case:?}: GetTable must report {expected}, got {read}"
        );
    }
}

/// A batch partition read reports the keys it left unprocessed as a LIST, which is empty
/// whenever the read answered every key it was given — so a batch caller's retry loop reads
/// zero keys instead of having to special-case an absent member.
///
/// The rows differ along how many keys the request carries, how many of them the table holds
/// a partition for, and whether a key is repeated.
#[tokio::test]
async fn a_batch_partition_read_reports_the_keys_it_left_unprocessed() {
    const DATABASE: &str = "unprocessed_keys_db";
    const TABLE: &str = "unprocessed_keys_tbl";

    let router = emulator();
    seed_table(&router, DATABASE, TABLE, &["day"]).await;
    for day in ["2026-01-01", "2026-01-02", "2026-01-03"] {
        ok(
            &router,
            "CreatePartition",
            &json!({
                "DatabaseName": DATABASE,
                "TableName": TABLE,
                "PartitionInput": { "Values": [day] },
            }),
        )
        .await;
    }

    // (case, the requested keys, how many partitions the answer holds)
    let rows: Vec<(&str, Vec<Vec<&str>>, usize)> = vec![
        ("one key the table holds", vec![vec!["2026-01-01"]], 1),
        (
            // The recorded shape: every requested key resolves.
            "every key the table holds",
            vec![vec!["2026-01-01"], vec!["2026-01-02"], vec!["2026-01-03"]],
            3,
        ),
        (
            "one key the table holds and one it does not",
            vec![vec!["2026-01-01"], vec!["2026-12-31"]],
            1,
        ),
        ("no key the table holds", vec![vec!["2025-01-01"]], 0),
        (
            "the same key twice",
            vec![vec!["2026-01-02"], vec!["2026-01-02"]],
            2,
        ),
    ];

    for (case, keys, found) in &rows {
        let to_get: Vec<Value> = keys
            .iter()
            .map(|values| json!({ "Values": values }))
            .collect();
        let body = ok(
            &router,
            "BatchGetPartition",
            &json!({
                "DatabaseName": DATABASE,
                "TableName": TABLE,
                "PartitionsToGet": to_get,
            }),
        )
        .await;
        assert_eq!(
            body.get("UnprocessedKeys"),
            Some(&json!([])),
            "case {case:?}: every key was answered, so UnprocessedKeys must be the empty \
             list, got {body}"
        );
        assert_eq!(
            body["Partitions"].as_array().map(Vec::len),
            Some(*found),
            "case {case:?}: expected {found} partition(s) in the answer, got {body}"
        );
    }
}

/// Every security configuration a read reports states whether it enforces SSL on the JDBC
/// connections made under it, and the listing reports the same configuration the single read
/// does.
///
/// The rows differ along which encryption properties the configuration carries: none at all,
/// one of each kind, and every kind at once.
#[tokio::test]
async fn a_security_configuration_reports_whether_it_enforces_jdbc_ssl() {
    let router = emulator();

    // (case, the create body)
    let rows: Vec<(&str, Value)> = vec![
        (
            "an encryption configuration carrying no property",
            json!({ "Name": "jdbc-none", "EncryptionConfiguration": {} }),
        ),
        (
            // The recorded shape: S3 encryption under a KMS key.
            "S3 encryption under a KMS key",
            json!({
                "Name": "jdbc-s3",
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-KMS", "KmsKeyArn": KMS_KEY_ARN }],
                },
            }),
        ),
        (
            "CloudWatch encryption only",
            json!({
                "Name": "jdbc-cloudwatch",
                "EncryptionConfiguration": {
                    "CloudWatchEncryption": { "CloudWatchEncryptionMode": "DISABLED" },
                },
            }),
        ),
        (
            "every encryption property at once",
            json!({
                "Name": "jdbc-all",
                "EncryptionConfiguration": {
                    "S3Encryption": [{ "S3EncryptionMode": "SSE-S3" }],
                    "CloudWatchEncryption": { "CloudWatchEncryptionMode": "DISABLED" },
                    "JobBookmarksEncryption": { "JobBookmarksEncryptionMode": "CSE-KMS" },
                    "DataQualityEncryption": { "DataQualityEncryptionMode": "DISABLED" },
                },
            }),
        ),
    ];

    for (case, request) in &rows {
        let name = request["Name"]
            .as_str()
            .expect("the row names its configuration");
        ok(&router, "CreateSecurityConfiguration", request).await;

        let read = ok(&router, "GetSecurityConfiguration", &json!({ "Name": name })).await;
        let reported = read["SecurityConfiguration"].clone();
        assert_eq!(
            reported["EncryptionConfiguration"].get("EnforceJdbcSsl"),
            Some(&json!(false)),
            "case {case:?}: no stored configuration enforces JDBC SSL, so the read must \
             report it false, got {read}"
        );

        let listing = ok(&router, "GetSecurityConfigurations", &json!({})).await;
        let listed = listing["SecurityConfigurations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a listing, got {listing}"))
            .iter()
            .find(|entry| entry.get("Name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("case {case:?}: the listing must report {name}"))
            .clone();
        assert_eq!(
            listed, reported,
            "case {case:?}: the listing must report the configuration the single read does, \
             got {listed}"
        );
    }
}

/// The Data Catalog reports the state of encrypted catalog search beside its encryption
/// settings, for a catalog nothing has configured as much as for one a write has.
///
/// The rows differ along what the write stated: nothing at all, each setting alone and both
/// together.
#[tokio::test]
async fn the_data_catalog_reports_the_state_of_encrypted_catalog_search() {
    // (case, the settings to write, if any)
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("a catalog nothing has configured", None),
        ("an empty settings structure", Some(json!({}))),
        (
            // The recorded shape: encryption at rest enforced under a key.
            "encryption at rest under a KMS key",
            Some(json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS",
                    "SseAwsKmsKeyId": KMS_KEY_ARN,
                },
            })),
        ),
        (
            "connection password encryption only",
            Some(json!({
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": true },
            })),
        ),
        (
            "both settings at once",
            Some(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false },
            })),
        ),
    ];

    for (case, settings) in &rows {
        let router = emulator();
        if let Some(settings) = settings {
            ok(
                &router,
                "PutDataCatalogEncryptionSettings",
                &json!({ "DataCatalogEncryptionSettings": settings }),
            )
            .await;
        }

        let (status, body) = call(&router, "GetDataCatalogEncryptionSettings", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the settings read must answer 200, got {status} with {body}"
        );
        assert_eq!(
            body["DataCatalogEncryptionSettings"].get("EncryptedCatalogSearchStatus"),
            Some(&json!(ENCRYPTED_CATALOG_SEARCH_STATUS)),
            "case {case:?}: expected the catalog-search state \
             {ENCRYPTED_CATALOG_SEARCH_STATUS:?}, got {body}"
        );
    }
}
