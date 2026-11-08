// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the export surface: `ExportTableToPointInTime` and the reader it
//! round-trips through, `DescribeExport` (with `ListExports` as the second reader).
//!
//! The rules pinned here:
//!
//! 1. an export is served only from a table that HAS a point in time to export from: a
//!    table whose point-in-time recovery is off is refused with
//!    `PointInTimeRecoveryUnavailableException` ("Point in time recovery is not enabled
//!    for table '<name>'"), whatever else the request asks for, and a table whose
//!    recovery was turned on and then off again is in exactly that state;
//! 2. an `ExportTime` must lie inside the table's recovery window, and one outside it is
//!    refused with `InvalidExportTimeException`; a request that names none exports the
//!    newest point that window reaches, which is the `LatestRestorableDateTime` the
//!    continuous-backups reader reports for the same table;
//! 3. an accepted export is RECORDED, and every member of the description is read from
//!    that record: the export reports `IN_PROGRESS` for the transition span and
//!    `COMPLETED` after it, and only once it has finished does it report an end instant,
//!    a manifest, the items it exported and their billable size — the items the table
//!    actually held;
//! 4. a `ClientToken` makes the call idempotent: the same token with the same parameters
//!    answers the same export rather than starting a second one, and the same token with
//!    different parameters is refused with `ExportConflictException`;
//! 5. the readers answer from that one record: `DescribeExport` reports it by its
//!    `ExportArn` and refuses an ARN no export of this endpoint is held under with
//!    `ExportNotFoundException`, and `ListExports` lists it;
//! 6. the export WRITES the items it read into the destination bucket — the data file,
//!    the manifests that say what is in it and the digests that cover them — so a caller
//!    lists and reads those objects through Amazon S3's own operations; a destination the
//!    object store holds no bucket for leaves the task `FAILED`, with the reason it
//!    failed.
//!
//! Every DynamoDB call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, with Amazon S3 mounted beside it
//! on ONE engine and read through its OWN client, because the objects an export writes
//! exist only if that service serves them. Each rule is asserted over a SET of inputs —
//! the members the request carries, the export format, the encryption it asks for, the
//! point in time it names, the items the table holds — rather than one example.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::S3_SSE_ALGORITHM_VALUES;
use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::emulator::{call, call_in, describe_when_active, error_code, message, ok, REGION};

/// A second region, for the rule that an export record is addressable only from the
/// endpoint whose region it was taken in.
const OTHER_REGION: &str = "eu-west-1";

/// The bucket a case that is not about the bucket exports to.
const BUCKET: &str = "table-exports";

/// A router carrying this service and the one an export writes its objects into — Amazon
/// S3 — over ONE shared engine, which is how the assembled emulator mounts them.
fn emulator_with_s3() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These suites sign nothing, so signature verification is off; the scope header is
    // read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam the export
/// writes its objects through, so a read here sees what the export really stored.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// The emulator with the destination bucket already created through Amazon S3's own API,
/// which is what an export needs to have somewhere to write.
async fn emulator_with_destination() -> MultiServiceRouter {
    let router = emulator_with_s3();
    s3(&router)
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("the destination bucket is created");
    router
}

/// The keys the destination bucket holds, in the order S3 lists them.
async fn listed_keys(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let listed = s3(router)
        .list_objects_v2()
        .bucket(bucket)
        .send()
        .await
        .expect("the destination bucket is listed");
    listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        .collect()
}

/// The bytes Amazon S3 serves for one key of the destination bucket.
async fn object_bytes(router: &MultiServiceRouter, bucket: &str, key: &str) -> Vec<u8> {
    let read = s3(router)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|error| panic!("S3 serves {key:?}: {error:?}"));
    read.body
        .collect()
        .await
        .expect("the object's bytes are read")
        .to_vec()
}

/// Wait out one transition span, so the `IN_PROGRESS` -> `COMPLETED` transition the clock
/// owes a started export is due.
async fn settle() {
    tokio::time::sleep(state::STATUS_TRANSITION + Duration::from_millis(250)).await;
}

/// Create one on-demand table, wait out the span the clock owes a new table, and put
/// `items` items in it, answering its `TableId`.
async fn table_with_items(router: &MultiServiceRouter, name: &str, items: usize) -> String {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let described = describe_when_active(router, name, name).await;
    for index in 0..items {
        ok(
            router,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {"Part": {"S": format!("item-{index}")}, "Payload": {"S": "x".repeat(16)}},
            }),
        )
        .await;
    }
    described["TableId"].as_str().unwrap_or_default().to_string()
}

/// Turn point-in-time recovery on (or off) for `name`.
async fn set_recovery(router: &MultiServiceRouter, name: &str, enabled: bool) -> Value {
    ok(
        router,
        "UpdateContinuousBackups",
        json!({
            "TableName": name,
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": enabled},
        }),
    )
    .await
}

/// The restorable window `name` reports right now: `(earliest, latest)` as the
/// continuous-backups reader states them, which is the window an export is held to.
async fn restorable_window(router: &MultiServiceRouter, name: &str) -> (f64, f64) {
    let described = ok(
        router,
        "DescribeContinuousBackups",
        json!({"TableName": name}),
    )
    .await;
    let recovery = &described["ContinuousBackupsDescription"]["PointInTimeRecoveryDescription"];
    (
        recovery["EarliestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default(),
        recovery["LatestRestorableDateTime"]
            .as_f64()
            .unwrap_or_default(),
    )
}

/// The `ExportDescription` of either operation's response.
fn description(body: &Value) -> Value {
    body["ExportDescription"].clone()
}

/// The ARN a response's export is recorded under.
fn export_arn(body: &Value) -> String {
    description(body)["ExportArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// An `ExportTableToPointInTime` body for `table` with no member but the two required
/// ones.
fn export_body(table_arn: &str) -> Value {
    json!({"TableArn": table_arn, "S3Bucket": BUCKET})
}

#[tokio::test]
async fn an_export_of_a_table_with_recovery_off_is_refused() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "NoRecovery", 1).await;
    table_with_items(&router, "RecoveryTurnedOff", 1).await;
    // A table whose recovery was turned on and then off again has forgotten where its
    // window started, so it is in the same state as one that never had recovery at all.
    set_recovery(&router, "RecoveryTurnedOff", true).await;
    set_recovery(&router, "RecoveryTurnedOff", false).await;

    // Rows differ in the table addressed and in what else the request asks for: the
    // refusal is about the table's recovery, so no other member can turn it into a
    // success.
    let cases: [(&str, &str, Value); 6] = [
        (
            "a table whose recovery was never enabled",
            "NoRecovery",
            json!({}),
        ),
        (
            "a table whose recovery was enabled and then disabled",
            "RecoveryTurnedOff",
            json!({}),
        ),
        (
            "an export naming a format",
            "NoRecovery",
            json!({"ExportFormat": "ION"}),
        ),
        (
            "an export naming a prefix and an owner",
            "NoRecovery",
            json!({"S3Prefix": "snapshots/nightly", "S3BucketOwner": engine::emulator_account_id()}),
        ),
        (
            "an export naming a point in time",
            "NoRecovery",
            json!({"ExportTime": 1_700_000_000}),
        ),
        (
            "an export naming a client token",
            "NoRecovery",
            json!({"ClientToken": "a-token-for-a-table-without-recovery"}),
        ),
    ];

    for (case, table, extra) in cases {
        let arn = state::table_arn(REGION, table);
        let mut body = export_body(&arn);
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let (status, answered) = call(&router, "ExportTableToPointInTime", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "PointInTimeRecoveryUnavailableException",
            "case {case:?}: a table with no point in time to export from is refused with the \
             modelled error, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Point in time recovery is not enabled for table '{table}'"),
            "case {case:?}: the refusal names the table the request asked for"
        );
        // Nothing was recorded, so the readers hold no export of that table.
        let listed = ok(&router, "ListExports", json!({"TableArn": arn})).await;
        assert_eq!(
            listed["ExportSummaries"],
            json!([]),
            "case {case:?}: a refused export records nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_accepted_export_is_recorded_and_read_back() {
    let router = emulator_with_destination().await;
    let items = 3;
    let table_id = table_with_items(&router, "Exported", items).await;
    set_recovery(&router, "Exported", true).await;
    let table_arn = state::table_arn(REGION, "Exported");
    let (_, latest) = restorable_window(&router, "Exported").await;

    // Rows differ in the members the request carries: the format the items are written in
    // (`DYNAMODB_JSON`, the one of the two values the member declares whose item encoding
    // this emulator writes), each encryption the bucket may use
    // (`S3_SSE_ALGORITHM_VALUES`, the `S3SseAlgorithm` enum), a prefix and none, an owner
    // and none, and the point in time named or left out.
    let owner = engine::emulator_account_id();
    let mut cases: Vec<(String, Value)> = vec![
        ("only the required members".to_string(), json!({})),
        (
            "a prefix and a bucket owner".to_string(),
            json!({"S3Prefix": "snapshots/nightly/", "S3BucketOwner": owner}),
        ),
        (
            "the newest point the window reaches".to_string(),
            json!({"ExportTime": latest}),
        ),
        (
            "the format named".to_string(),
            json!({"ExportFormat": "DYNAMODB_JSON"}),
        ),
    ];
    for algorithm in S3_SSE_ALGORITHM_VALUES {
        let mut asked = json!({"S3SseAlgorithm": algorithm});
        if *algorithm == "KMS" {
            asked["S3SseKmsKeyId"] = json!("1234abcd-12ab-34cd-56ef-1234567890ab");
        }
        cases.push((format!("{algorithm} encryption"), asked));
    }

    let mut started_exports: Vec<(String, String, Value)> = Vec::new();
    for (case, extra) in cases {
        let mut body = export_body(&table_arn);
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let started = ok(&router, "ExportTableToPointInTime", body.clone()).await;
        let reported = description(&started);
        let arn = export_arn(&started);

        assert!(
            arn.starts_with(&format!("{table_arn}/export/")),
            "case {case:?}: the ExportArn names an export of the table it was taken from, got \
             {reported}"
        );
        assert_eq!(
            reported["ExportStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: an export task runs for a span before it completes, got {reported}"
        );
        assert_eq!(
            reported["TableArn"],
            json!(table_arn),
            "case {case:?}: the description names the exported table, got {reported}"
        );
        assert_eq!(
            reported["TableId"],
            json!(table_id),
            "case {case:?}: the description carries the exported table's own id, got {reported}"
        );
        assert_eq!(
            reported["ExportType"],
            json!("FULL_EXPORT"),
            "case {case:?}: an export that names no type is a full export, got {reported}"
        );
        assert_eq!(
            reported["S3Bucket"],
            json!(BUCKET),
            "case {case:?}: the description echoes the bucket asked for, got {reported}"
        );
        // Every member the request named is reported back as it was asked for, and one it
        // did not name is absent — except `ExportFormat`, which has a documented default.
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            if member == "ExportTime" {
                let asked = value.as_f64().unwrap_or_default();
                let answered = reported["ExportTime"].as_f64().unwrap_or_default();
                assert!(
                    (asked - answered).abs() < 0.002,
                    "case {case:?}: the point in time exported is the one asked for \
                     ({asked}), got {answered}"
                );
                continue;
            }
            assert_eq!(
                reported[&member], value,
                "case {case:?}: the description echoes {member}, got {reported}"
            );
        }
        if extra.get("ExportFormat").is_none() {
            assert_eq!(
                reported["ExportFormat"],
                json!("DYNAMODB_JSON"),
                "case {case:?}: an export that names no format takes the documented default, \
                 got {reported}"
            );
        }
        if extra.get("S3Prefix").is_none() {
            assert!(
                reported["S3Prefix"].is_null(),
                "case {case:?}: a request that named no prefix reports none, got {reported}"
            );
        }
        if extra.get("ExportTime").is_none() {
            let answered = reported["ExportTime"].as_f64().unwrap_or_default();
            assert!(
                (latest - answered).abs() < 0.002,
                "case {case:?}: an export that names no point in time exports the newest the \
                 window reaches ({latest}), got {answered}"
            );
        }
        // While the task is running it reports nothing of what it will have written.
        for member in ["EndTime", "ExportManifest", "ItemCount", "BilledSizeBytes"] {
            assert!(
                reported[member].is_null(),
                "case {case:?}: a running export reports no {member}, got {reported}"
            );
        }
        assert!(
            reported["ClientToken"].is_null(),
            "case {case:?}: a request that carried no client token reports none, got {reported}"
        );

        // The reader answers from that one record, and the description it gives while the
        // task runs is the one the write path gave.
        let read = ok(&router, "DescribeExport", json!({"ExportArn": arn.clone()})).await;
        assert_eq!(
            description(&read),
            reported,
            "case {case:?}: DescribeExport reports the record the export wrote"
        );
        started_exports.push((case, arn, extra));
    }

    // One wait for the span every started export owes, then each is read back finished.
    settle().await;
    for (case, arn, extra) in started_exports {
        let finished =
            description(&ok(&router, "DescribeExport", json!({"ExportArn": arn.clone()})).await);
        assert_eq!(
            finished["ExportStatus"],
            json!("COMPLETED"),
            "case {case:?}: the span the clock owed has elapsed, got {finished}"
        );
        assert!(
            finished["EndTime"].as_f64().unwrap_or_default()
                >= finished["StartTime"].as_f64().unwrap_or_default(),
            "case {case:?}: a finished export ends no earlier than it began, got {finished}"
        );
        assert_eq!(
            finished["ItemCount"],
            json!(items),
            "case {case:?}: the export reports the items the table held, got {finished}"
        );
        assert!(
            finished["BilledSizeBytes"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: an export of a table holding items has a billable size, got \
             {finished}"
        );
        let manifest = finished["ExportManifest"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let id = arn.rsplit('/').next().unwrap_or_default();
        assert!(
            manifest.ends_with("manifest-summary.json") && manifest.contains(id),
            "case {case:?}: a finished export names the manifest it wrote, under its own id \
             {id:?}, got {manifest:?}"
        );
        if let Some(prefix) = extra.get("S3Prefix").and_then(Value::as_str) {
            assert!(
                manifest.starts_with(prefix.trim_end_matches('/')),
                "case {case:?}: the manifest is written under the prefix asked for, got \
                 {manifest:?}"
            );
        }

        // And the listing reports the same record, in the status it now stands in.
        let listed = ok(&router, "ListExports", json!({"TableArn": table_arn.clone()})).await;
        let summary = listed["ExportSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["ExportArn"] == json!(arn))
            .unwrap_or(Value::Null);
        assert_eq!(
            summary["ExportStatus"],
            json!("COMPLETED"),
            "case {case:?}: the listing reports the export this started, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_export_of_an_empty_table_reports_no_items() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Empty", 0).await;
    set_recovery(&router, "Empty", true).await;
    let arn = export_arn(
        &ok(
            &router,
            "ExportTableToPointInTime",
            export_body(&state::table_arn(REGION, "Empty")),
        )
        .await,
    );

    settle().await;
    let finished = description(&ok(&router, "DescribeExport", json!({"ExportArn": arn})).await);
    assert_eq!(
        finished["ItemCount"],
        json!(0),
        "an export of a table holding no items exports none, got {finished}"
    );
    assert_eq!(
        finished["BilledSizeBytes"],
        json!(0),
        "an export of a table holding no items has no billable size, got {finished}"
    );
}

#[tokio::test]
async fn a_point_in_time_outside_the_recovery_window_is_refused() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Windowed", 1).await;
    set_recovery(&router, "Windowed", true).await;
    let table_arn = state::table_arn(REGION, "Windowed");
    let (earliest, latest) = restorable_window(&router, "Windowed").await;

    // Rows differ in where the named instant falls relative to the window the table
    // reports: each published bound, inside it, and one step past each end.
    let cases: [(&str, f64, bool); 5] = [
        ("the oldest restorable point", earliest, true),
        ("the newest restorable point", latest, true),
        ("a point between the bounds", (earliest + latest) / 2.0, true),
        ("a point before recovery began", earliest - 86_400.0, false),
        ("a point in the future", latest + 3_600.0, false),
    ];

    for (case, point, accepted) in cases {
        let mut body = export_body(&table_arn);
        body["ExportTime"] = json!(point);
        let (status, answered) = call(&router, "ExportTableToPointInTime", body.clone()).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a point inside the window is exported, got {status} with \
                 {answered}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidExportTimeException",
            "case {case:?}: a point outside the window is refused with the modelled error, got \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            "The specified ExportTime is outside of the point in time recovery window",
            "case {case:?}: the refusal states what the model states for this error"
        );
    }
}

#[tokio::test]
async fn a_client_token_makes_the_call_idempotent() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Idempotent", 2).await;
    set_recovery(&router, "Idempotent", true).await;
    let table_arn = state::table_arn(REGION, "Idempotent");

    // Rows differ in the request the token is first used for, and so in what a repeat has
    // to match: the required members alone, a prefix, a format, and a named point in time.
    let (_, latest) = restorable_window(&router, "Idempotent").await;
    let cases: [(&str, Value); 4] = [
        ("only the required members", json!({})),
        ("a prefix", json!({"S3Prefix": "nightly"})),
        ("a format", json!({"ExportFormat": "DYNAMODB_JSON"})),
        ("a named point in time", json!({"ExportTime": latest})),
    ];

    for (case, extra) in cases {
        let token = format!("token-for-{}", case.replace(' ', "-"));
        let mut body = export_body(&table_arn);
        body["ClientToken"] = json!(token);
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }

        let first = ok(&router, "ExportTableToPointInTime", body.clone()).await;
        assert_eq!(
            description(&first)["ClientToken"],
            json!(token),
            "case {case:?}: the description reports the token the request carried, got {first}"
        );
        let repeated = ok(&router, "ExportTableToPointInTime", body.clone()).await;
        assert_eq!(
            export_arn(&repeated),
            export_arn(&first),
            "case {case:?}: an identical call under the same token answers the same export"
        );

        // The same token with any other parameters names a different export, which is the
        // conflict the operation models.
        let mut changed = body.clone();
        changed["S3Bucket"] = json!("another-bucket");
        let (status, answered) = call(&router, "ExportTableToPointInTime", changed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ExportConflictException",
            "case {case:?}: the same token with changed parameters is refused, got {answered}"
        );
    }

    // One record per token, and not one per call: the repeats above started nothing new.
    let listed = ok(&router, "ListExports", json!({"TableArn": table_arn})).await;
    assert_eq!(
        listed["ExportSummaries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        4,
        "an idempotent repeat records no second export, got {listed}"
    );
}

#[tokio::test]
async fn a_table_arn_that_addresses_no_table_of_this_endpoint_is_refused() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Present", 1).await;
    set_recovery(&router, "Present", true).await;

    // A well-formed ARN of this endpoint naming a table that does not exist is the miss
    // the operation models; a value that is no table ARN of this endpoint at all is an
    // input failure.
    let (status, answered) = call(
        &router,
        "ExportTableToPointInTime",
        export_body(&state::table_arn(REGION, "Absent")),
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with {answered}");
    assert_eq!(
        error_code(&answered),
        "TableNotFoundException",
        "a table this endpoint holds none of is the modelled miss, got {answered}"
    );
    assert_eq!(
        message(&answered),
        "Table not found: Absent",
        "the refusal names the table the request asked for"
    );

    // Rows differ in HOW the value fails to be an ARN of this service naming a table of
    // the endpoint the request reached.
    let cases: [(&str, String); 4] = [
        ("not an ARN at all", "Present".to_string()),
        (
            "an ARN of another service",
            engine::emulator_arn("s3", REGION, "Present"),
        ),
        (
            "an index of the table",
            format!("{}/index/ByPart", state::table_arn(REGION, "Present")),
        ),
        (
            "a table of another region",
            state::table_arn(OTHER_REGION, "Present"),
        ),
    ];
    for (case, value) in cases {
        let (status, answered) = call(
            &router,
            "ExportTableToPointInTime",
            export_body(&value.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: a value that addresses no table is an input failure, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Invalid TableArn: Invalid TableArn provided as input {value}"),
            "case {case:?}: the refusal states the member and echoes the value sent"
        );
    }
}

#[tokio::test]
async fn a_value_outside_an_enumerated_members_set_is_refused() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Enumerated", 1).await;
    set_recovery(&router, "Enumerated", true).await;
    let table_arn = state::table_arn(REGION, "Enumerated");

    // Rows differ in which enumerated member carries a value outside its set, and the
    // set it is held to is the model's own (the `ExportFormat`, `ExportType` and
    // `S3SseAlgorithm` value tables this service publishes).
    let cases: [(&str, &str, &str, &[&str]); 3] = [
        (
            "a format no export takes",
            "ExportFormat",
            "PARQUET",
            service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::EXPORT_FORMAT_VALUES,
        ),
        (
            "an export type that is neither",
            "ExportType",
            "PARTIAL_EXPORT",
            service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::EXPORT_TYPE_VALUES,
        ),
        (
            "an encryption the bucket cannot use",
            "S3SseAlgorithm",
            "ROT13",
            S3_SSE_ALGORITHM_VALUES,
        ),
    ];

    for (case, member, value, values) in cases {
        let mut body = export_body(&table_arn);
        body[member] = json!(value);
        let (status, answered) = call(&router, "ExportTableToPointInTime", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: a value outside an enumerated member's set is refused, got {answered}"
        );
        assert!(
            message(&answered).contains(&values.join(", ")),
            "case {case:?}: the refusal states the value set the member is held to, got {}",
            message(&answered)
        );
    }
}

#[tokio::test]
async fn an_export_arn_no_export_is_held_under_is_refused() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Readable", 1).await;
    set_recovery(&router, "Readable", true).await;
    let arn = export_arn(
        &ok(
            &router,
            "ExportTableToPointInTime",
            export_body(&state::table_arn(REGION, "Readable")),
        )
        .await,
    );

    // A record is addressable only from the region it was taken in: an export of this
    // store read from another endpoint is the same miss as one that was never started.
    let (status, answered) = call_in(
        &router,
        OTHER_REGION,
        "DescribeExport",
        json!({"ExportArn": arn.clone()}),
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} with {answered}");
    assert_eq!(
        error_code(&answered),
        "ValidationException",
        "an ARN of another region's endpoint addresses no export here, got {answered}"
    );

    // Rows differ in HOW the ARN fails to name an export this endpoint holds: a
    // well-formed ARN of an export never started, a backup's ARN, a table's own ARN, and
    // a value that is no ARN at all.
    let table_arn = state::table_arn(REGION, "Readable");
    let cases: [(&str, String, &str); 4] = [
        (
            "an export that was never started",
            format!("{table_arn}/export/01700000000000-deadbeef"),
            "ExportNotFoundException",
        ),
        (
            "a backup of the same table",
            format!("{table_arn}/backup/01700000000000-deadbeef"),
            "ValidationException",
        ),
        ("the table's own ARN", table_arn.clone(), "ValidationException"),
        (
            "a value that is no ARN at all",
            "Readable/export/1".to_string(),
            "ValidationException",
        ),
    ];

    for (case, value, expected) in cases {
        let (status, answered) =
            call(&router, "DescribeExport", json!({"ExportArn": value.clone()})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            expected,
            "case {case:?}: expected {expected}, got {answered}"
        );
    }

    // And the ARN that WAS minted still reads back, so the refusals above are about the
    // values they were given and not about the reader.
    let read = ok(&router, "DescribeExport", json!({"ExportArn": arn.clone()})).await;
    assert_eq!(
        description(&read)["ExportArn"],
        json!(arn),
        "the export this started reads back by its own ARN, got {read}"
    );
}

#[tokio::test]
async fn a_capability_no_record_carries_yet_is_refused_as_unimplemented() {
    let router = emulator_with_destination().await;
    table_with_items(&router, "Incremental", 1).await;
    set_recovery(&router, "Incremental", true).await;
    let table_arn = state::table_arn(REGION, "Incremental");
    let (_, latest) = restorable_window(&router, "Incremental").await;

    // An incremental export reports what CHANGED between two points in time, a filter
    // only the items a condition selects, and an Ion export the items in an encoding this
    // emulator does not write: none of the three is something it can write real data for,
    // so each is refused as not written yet rather than answered with a record of the
    // full, unfiltered, DynamoDB-JSON export the caller did not ask for.
    let cases: [(&str, Value); 4] = [
        (
            "an incremental export",
            json!({"ExportType": "INCREMENTAL_EXPORT", "IncrementalExportSpecification": {
                "ExportFromTime": latest - 3_600.0,
                "ExportToTime": latest,
            }}),
        ),
        (
            "an incremental specification on a full export",
            json!({"IncrementalExportSpecification": {"ExportToTime": latest}}),
        ),
        (
            "a filtered export",
            json!({"FilterSpecification": {"FilterExpression": "attribute_exists(Payload)"}}),
        ),
        ("an Ion export", json!({"ExportFormat": "ION"})),
    ];

    for (case, extra) in cases {
        let mut body = export_body(&table_arn);
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let (status, answered) = call(&router, "ExportTableToPointInTime", body).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected HTTP 501, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "NotImplemented",
            "case {case:?}: a capability nothing can report is refused as unwritten, got \
             {answered}"
        );
    }

    // Nothing was recorded for any of them.
    let listed = ok(&router, "ListExports", json!({"TableArn": table_arn})).await;
    assert_eq!(
        listed["ExportSummaries"],
        json!([]),
        "a refused export records nothing, got {listed}"
    );
}

#[tokio::test]
async fn an_export_writes_the_items_it_read_into_the_destination_bucket() {
    let router = emulator_with_destination().await;
    let table_id = table_with_items(&router, "Written", 2).await;
    set_recovery(&router, "Written", true).await;
    let table_arn = state::table_arn(REGION, "Written");

    // Rows differ in the prefix the objects are written under, which is the one member
    // that moves their keys: none, a plain one, and one a caller spelled with slashes
    // around it.
    let cases: [(&str, Option<&str>); 3] = [
        ("no prefix", None),
        ("a plain prefix", Some("nightly")),
        ("a prefix spelled with slashes", Some("/snapshots/nightly/")),
    ];

    for (case, prefix) in cases {
        let mut body = export_body(&table_arn);
        if let Some(prefix) = prefix {
            body["S3Prefix"] = json!(prefix);
        }
        let started = ok(&router, "ExportTableToPointInTime", body).await;
        let arn = export_arn(&started);
        let id = arn.rsplit('/').next().unwrap_or_default().to_string();
        let under = match prefix.map(|prefix| prefix.trim_matches('/')) {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}/AWSDynamoDB/{id}/"),
            _ => format!("AWSDynamoDB/{id}/"),
        };

        // The objects are the ones a DynamoDB export leaves behind, and they exist
        // because Amazon S3 itself lists them.
        let keys = listed_keys(&router, BUCKET).await;
        for suffix in [
            "_started",
            "manifest-files.json",
            "manifest-files.json.md5",
            "manifest-summary.json",
            "manifest-summary.json.md5",
            &format!("data/{id}.json.gz"),
        ] {
            let key = format!("{under}{suffix}");
            assert!(
                keys.contains(&key),
                "case {case:?}: S3 must hold {key:?} after the export, got {keys:?}"
            );
        }

        // The summary manifest says what was exported, read from the record: the export
        // and table it belongs to, the items it wrote, and where the data files are
        // listed.
        let summary_key = format!("{under}manifest-summary.json");
        let summary: Value = serde_json::from_slice(&object_bytes(&router, BUCKET, &summary_key).await)
            .unwrap_or(Value::Null);
        assert_eq!(
            summary["exportArn"],
            json!(arn),
            "case {case:?}: the manifest names the export that wrote it, got {summary}"
        );
        assert_eq!(
            summary["tableArn"],
            json!(table_arn),
            "case {case:?}: the manifest names the exported table, got {summary}"
        );
        assert_eq!(
            summary["tableId"],
            json!(table_id),
            "case {case:?}: the manifest carries the table's own id, got {summary}"
        );
        assert_eq!(
            summary["itemCount"],
            json!(2),
            "case {case:?}: the manifest counts the items the table held, got {summary}"
        );
        assert_eq!(
            summary["outputFormat"],
            json!("DYNAMODB_JSON"),
            "case {case:?}: the manifest names the format the data was written in, got {summary}"
        );
        assert_eq!(
            summary["manifestFilesS3Key"],
            json!(format!("{under}manifest-files.json")),
            "case {case:?}: the manifest points at the list of data files, got {summary}"
        );

        // Which in turn points at the data file, with the digests of the bytes S3 holds.
        let files = String::from_utf8(
            object_bytes(&router, BUCKET, &format!("{under}manifest-files.json")).await,
        )
        .unwrap_or_default();
        let listed_file: Value = serde_json::from_str(files.trim()).unwrap_or(Value::Null);
        let data_key = listed_file["dataFileS3Key"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            data_key,
            format!("{under}data/{id}.json.gz"),
            "case {case:?}: the file list names the data file, got {listed_file}"
        );
        assert_eq!(
            listed_file["itemCount"],
            json!(2),
            "case {case:?}: the file list counts the items in that file, got {listed_file}"
        );
        let data = object_bytes(&router, BUCKET, &data_key).await;
        assert!(
            !data.is_empty() && data.starts_with(&[0x1f, 0x8b]),
            "case {case:?}: the data file is the GZIP object its key names, got {} bytes",
            data.len()
        );
        assert_eq!(
            listed_file["etag"],
            json!(service_com_amazonaws_s3_amazons3::state::unquoted_etag(
                &service_com_amazonaws_s3_amazons3::state::object_etag(&data)
            )),
            "case {case:?}: the file list carries the entity tag of the bytes stored, got \
             {listed_file}"
        );

        // And the reader's own `ExportManifest` names that very object, so a caller that
        // reads the description can read the manifest.
        settle().await;
        let finished = description(&ok(&router, "DescribeExport", json!({"ExportArn": arn})).await);
        assert_eq!(
            finished["ExportManifest"],
            json!(summary_key),
            "case {case:?}: the description names the manifest in the bucket, got {finished}"
        );
        assert_eq!(
            finished["ExportStatus"],
            json!("COMPLETED"),
            "case {case:?}: an export whose objects landed completes, got {finished}"
        );
    }
}

#[tokio::test]
async fn an_export_to_a_bucket_the_object_store_does_not_hold_fails() {
    // No bucket is created here, so the destination the export names holds nothing.
    let router = emulator_with_s3();
    table_with_items(&router, "Undeliverable", 1).await;
    set_recovery(&router, "Undeliverable", true).await;

    let started = ok(
        &router,
        "ExportTableToPointInTime",
        export_body(&state::table_arn(REGION, "Undeliverable")),
    )
    .await;
    let arn = export_arn(&started);
    assert_eq!(
        description(&started)["ExportStatus"],
        json!("IN_PROGRESS"),
        "the export is accepted and starts, got {started}"
    );

    settle().await;
    let finished = description(&ok(&router, "DescribeExport", json!({"ExportArn": arn})).await);
    assert_eq!(
        finished["ExportStatus"],
        json!("FAILED"),
        "an export with nowhere to write cannot complete, got {finished}"
    );
    assert_eq!(
        finished["FailureCode"],
        json!("S3NoSuchBucket"),
        "the failure says what was wrong with the destination, got {finished}"
    );
    assert_eq!(
        finished["FailureMessage"],
        json!("The specified bucket does not exist"),
        "the failure carries the reason S3 states for that destination, got {finished}"
    );
    // And it reports nothing it did not write.
    for member in ["ExportManifest", "ItemCount", "BilledSizeBytes"] {
        assert!(
            finished[member].is_null(),
            "a failed export reports no {member}, got {finished}"
        );
    }
}
