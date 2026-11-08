// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListExports`.
//!
//! The rule pinned here: a listing reports one `ExportSummary` per export record the
//! store holds that the request SELECTS — the region the request reached, the table its
//! `TableArn` names, and the documented window "within the past 90 days" — in "reverse
//! alphanumeric order of ExportArn" (the operation's own doc), `MaxResults` entries at a
//! time, with `NextToken` reported exactly when more remain and resuming strictly after
//! the entry the page stopped on in that order. Every member of a summary is read from
//! the record, so nothing is canned.
//!
//! Each rule is asserted over a SET of inputs: the records held, the table named, the
//! region, the page size (on each bound the member publishes and one past it), the age of
//! a record, the status and type of a record, and the cursor.
//!
//! The records are seeded through the same per-service store `ExportTableToPointInTime`
//! writes and this handler reads (the shared engine `emulator_and_engine` hands back),
//! because a listing's rules are about records of a given AGE, status and region that no
//! single request can produce; every read is driven over the wire through the router the
//! emulator's front doors use. The write path has its own suite (`table_exports`).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::{
    EXPORT_STATUS_VALUES, EXPORT_TYPE_VALUES,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    self, Export, EXPORT_HISTORY_WINDOW,
};

use crate::emulator::{call, call_in, emulator_and_engine, error_code, message, ok, REGION};

/// A second region, for the rule that a listing answers over the region its request
/// reached.
const OTHER_REGION: &str = "eu-west-1";

/// One day, the unit the documented 90-day window is stated in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A whole-second instant `ago` before now: a record stored at whole seconds reports a
/// timestamp a test can compare exactly, and every instant here stays inside the
/// documented window unless a case puts it outside on purpose.
fn seconds_ago(ago: Duration) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch");
    UNIX_EPOCH + Duration::from_secs(now.as_secs().saturating_sub(ago.as_secs()))
}

/// Record one export in the store the handler reads, answering the `ExportArn` it was
/// recorded under.
fn record_export(engine: &Arc<engine::InMemoryEngine>, export: Export) -> String {
    let arn = export.arn.clone();
    state::service_state(&**engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .insert_export(export);
    arn
}

/// One export record of `table` in `region`, begun at `started_at` and due to finish one
/// `span` later, in the form `ExportTableToPointInTime` records one.
fn export_record(
    region: &str,
    table: &str,
    started_at: SystemTime,
    span: Duration,
    export_type: &str,
) -> Export {
    Export {
        arn: state::new_export_arn(region, table, started_at),
        region: region.to_string(),
        table_name: table.to_string(),
        table_id: state::new_table_id(),
        client_token: None,
        request: state::ExportRequest {
            export_time: None,
            export_format: "DYNAMODB_JSON".to_string(),
            export_type: export_type.to_string(),
            s3_bucket: "exported-tables".to_string(),
            s3_bucket_owner: None,
            s3_prefix: None,
            s3_sse_algorithm: None,
            s3_sse_kms_key_id: None,
        },
        export_time: started_at,
        started_at,
        completes_at: started_at + span,
        item_count: 0,
        billed_size_bytes: 0,
        // These records stand for exports whose task wrote its data, which is what a
        // listing reports as `COMPLETED`.
        failure: None,
    }
}

/// One FINISHED full export of `table` in `region`, begun at `started_at`: its span ended
/// in the past, so every read reports it `COMPLETED`.
fn completed_export(region: &str, table: &str, started_at: SystemTime) -> Export {
    export_record(region, table, started_at, Duration::from_secs(30), "FULL_EXPORT")
}

/// The same record still RUNNING: its span ends far enough ahead that a read reports it
/// `IN_PROGRESS` however slow the machine is.
fn running_export(region: &str, table: &str, started_at: SystemTime, export_type: &str) -> Export {
    Export {
        completes_at: SystemTime::now() + Duration::from_secs(3_600),
        ..export_record(region, table, started_at, Duration::from_secs(30), export_type)
    }
}

/// A `NextToken` carrying `cursor`, in the form this operation mints one.
///
/// A client never builds one — it copies the value a page reported — so this is only how
/// a case here names a cursor the store holds no record after.
fn token_for(cursor: &str) -> String {
    cursor.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// The `ExportSummaries` of a response, as a list.
fn summaries(body: &Value) -> Vec<Value> {
    body["ExportSummaries"].as_array().cloned().unwrap_or_default()
}

/// The `ExportArn`s a response reports, in the order it reported them.
fn listed_arns(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["ExportArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `body`'s `NextToken`, or `None` where it reported none.
fn next_token(body: &Value) -> Option<String> {
    body["NextToken"].as_str().map(str::to_string)
}

/// `arns` in the order this listing reports them: "reverse alphanumeric order of
/// ExportArn" (the `ListExports` doc).
fn reverse_alphanumeric(arns: &[String]) -> Vec<String> {
    let mut ordered = arns.to_vec();
    ordered.sort();
    ordered.reverse();
    ordered
}

/// One `CreateTable` body with a simple key, for a case that is not about the table.
fn simple_table(name: &str) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

#[tokio::test]
async fn an_account_that_has_run_no_export_is_answered_an_empty_page() {
    let (router, _engine) = emulator_and_engine();
    ok(&router, "CreateTable", simple_table("Exported")).await;

    // Rows differ in what the request asks for: the table it names, the page it asks
    // for, and the cursor it resumes from. None of them can conjure a record.
    let cases: [(&str, Value); 7] = [
        ("no member at all", json!({})),
        (
            "the ARN of a table that exists",
            json!({"TableArn": state::table_arn(REGION, "Exported")}),
        ),
        (
            "the ARN of a table that was never created",
            json!({"TableArn": state::table_arn(REGION, "NeverCreated")}),
        ),
        // The page sizes the member publishes (`ListExportsMaxLimit`, range 1..=25).
        ("the smallest page the member allows", json!({"MaxResults": 1})),
        ("a page size inside the range", json!({"MaxResults": 13})),
        ("the largest page the member allows", json!({"MaxResults": 25})),
        (
            "a cursor no record can follow",
            json!({"NextToken": token_for(&format!(
                "{}/export/99999999999999-ffffffff",
                state::table_arn(REGION, "Zzz")
            ))}),
        ),
    ];

    for (case, body) in cases {
        let answered = ok(&router, "ListExports", body.clone()).await;
        assert!(
            answered["ExportSummaries"].is_array(),
            "case {case:?}: the listing must report an ExportSummaries list, got {answered}"
        );
        assert!(
            summaries(&answered).is_empty(),
            "case {case:?}: expected no export summaries, got {answered}"
        );
        assert_eq!(
            next_token(&answered),
            None,
            "case {case:?}: a last page reports no NextToken, got {answered}"
        );
    }
}

#[tokio::test]
async fn every_member_of_a_recorded_export_is_reported() {
    let (router, engine) = emulator_and_engine();

    // One record per (`ExportStatus`, `ExportType`) pair the model declares — the two
    // catalogues this service exposes as `EXPORT_STATUS_VALUES` and `EXPORT_TYPE_VALUES`
    // (the `ExportStatus` and `ExportType` enums;
    // `smithy_query(query="shape",shape="ExportStatus,ExportType")`) — so no status or
    // type the API admits goes untested: work still running, work that finished, and work
    // whose task could not write what it read.
    let mut recorded: Vec<(String, Export)> = Vec::new();
    for (index, status) in EXPORT_STATUS_VALUES.iter().enumerate() {
        for kind in EXPORT_TYPE_VALUES {
            let started = seconds_ago(Duration::from_secs(
                3_600 * (recorded.len() as u64 + 1) + index as u64,
            ));
            let finished = export_record(REGION, "Exported", started, Duration::from_secs(30), kind);
            let export = match *status {
                "IN_PROGRESS" => running_export(REGION, "Exported", started, kind),
                "FAILED" => Export {
                    failure: Some(state::ExportFailure {
                        code: state::EXPORT_FAILURE_NO_SUCH_BUCKET.to_string(),
                        message: state::EXPORT_FAILURE_NO_SUCH_BUCKET_MESSAGE.to_string(),
                    }),
                    ..finished
                },
                _ => finished,
            };
            recorded.push(((*status).to_string(), export));
        }
    }
    for (_, export) in &recorded {
        record_export(&engine, export.clone());
    }

    let answered = ok(&router, "ListExports", json!({})).await;
    assert_eq!(
        summaries(&answered).len(),
        recorded.len(),
        "every recorded export must be reported, got {answered}"
    );

    for (status, export) in &recorded {
        let case = format!("{status} / {}", export.request.export_type);
        let reported = summaries(&answered)
            .into_iter()
            .find(|summary| summary["ExportArn"] == json!(export.arn))
            .unwrap_or(Value::Null);
        assert_eq!(
            reported["ExportStatus"],
            json!(status),
            "case {case:?}: the reported status is the one the record stands in, got {reported}"
        );
        assert_eq!(
            reported["ExportType"],
            json!(export.request.export_type),
            "case {case:?}: the reported type is the one recorded, got {reported}"
        );
        // `ExportSummary` models exactly these three members, so a summary reports
        // neither more nor fewer.
        let mut members: Vec<String> = reported
            .as_object()
            .map(|summary| summary.keys().cloned().collect())
            .unwrap_or_default();
        members.sort();
        assert_eq!(
            members,
            vec![
                "ExportArn".to_string(),
                "ExportStatus".to_string(),
                "ExportType".to_string()
            ],
            "case {case:?}: a summary carries the members ExportSummary models, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_the_records_the_request_selects() {
    let (router, engine) = emulator_and_engine();
    // One table that still exists and one that never did: an export record outlives the
    // table it was taken from, so the listing is answered from the records and not from
    // the tables.
    ok(&router, "CreateTable", simple_table("Alpha")).await;

    let alpha: Vec<String> = (1..=2)
        .map(|hours| {
            record_export(
                &engine,
                completed_export(REGION, "Alpha", seconds_ago(Duration::from_secs(3_600 * hours))),
            )
        })
        .collect();
    let beta = vec![record_export(
        &engine,
        completed_export(REGION, "Beta", seconds_ago(Duration::from_secs(600))),
    )];
    let elsewhere = vec![record_export(
        &engine,
        completed_export(OTHER_REGION, "Alpha", seconds_ago(Duration::from_secs(900))),
    )];

    let here = reverse_alphanumeric(&[alpha.clone(), beta.clone()].concat());

    // Rows differ in the region the request reaches and the table its `TableArn` names.
    let cases: [(&str, &str, Value, Vec<String>); 5] = [
        (
            "no table named, in the region the records are in",
            REGION,
            json!({}),
            here,
        ),
        (
            "the table that still exists",
            REGION,
            json!({"TableArn": state::table_arn(REGION, "Alpha")}),
            reverse_alphanumeric(&alpha),
        ),
        (
            "a table whose own records outlived it",
            REGION,
            json!({"TableArn": state::table_arn(REGION, "Beta")}),
            reverse_alphanumeric(&beta),
        ),
        (
            "a table of this region with no record of its own",
            REGION,
            json!({"TableArn": state::table_arn(REGION, "Gamma")}),
            Vec::new(),
        ),
        (
            "another region, where only its own record is addressable",
            OTHER_REGION,
            json!({}),
            reverse_alphanumeric(&elsewhere),
        ),
    ];

    for (case, region, body, expected) in cases {
        let (status, answered) = call_in(&router, region, "ListExports", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered}"
        );
        assert_eq!(
            listed_arns(&answered),
            expected,
            "case {case:?}: expected the selected records in reverse alphanumeric ExportArn \
             order, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_max_results_records_and_a_token_that_resumes_after_it() {
    let (router, engine) = emulator_and_engine();

    // Seeded oldest first, so a listing in reverse ARN order cannot be insertion order.
    let seeded: Vec<String> = (1..=7)
        .rev()
        .map(|hours| {
            record_export(
                &engine,
                completed_export(REGION, "Paged", seconds_ago(Duration::from_secs(3_600 * hours))),
            )
        })
        .collect();
    let all = reverse_alphanumeric(&seeded);

    // Rows differ in the page size asked for: both bounds the member publishes
    // (`ListExportsMaxLimit`, range 1..=25), sizes inside it either side of the record
    // count, and a request that names none.
    let cases: [(&str, Option<i32>); 6] = [
        ("one record at a time", Some(1)),
        ("two records at a time", Some(2)),
        ("a page smaller than the records held", Some(3)),
        ("a page exactly as large as the records held", Some(7)),
        ("the largest page the member allows", Some(25)),
        ("no page size named", None),
    ];

    for (case, max_results) in cases {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=all.len() {
            let mut body = json!({});
            if let Some(size) = max_results {
                body["MaxResults"] = json!(size);
            }
            if let Some(cursor) = token.as_deref() {
                body["NextToken"] = json!(cursor);
            }
            let answered = ok(&router, "ListExports", body.clone()).await;
            let page = listed_arns(&answered);
            let remaining = all.len() - walked.len();
            let expected_page = max_results
                .map_or(remaining, |size| usize::try_from(size).unwrap_or(0))
                .min(remaining);
            assert_eq!(
                page.len(),
                expected_page,
                "case {case:?}: expected {expected_page} records on this page, got {answered}"
            );
            walked.extend(page);
            token = next_token(&answered);
            match token.as_deref() {
                Some(minted) => {
                    assert!(
                        walked.len() < all.len(),
                        "case {case:?}: a NextToken is reported only while records remain, \
                         got {answered}"
                    );
                    // Opaque, and in the form this operation mints: a value a client may
                    // send straight back without reading anything out of it.
                    assert!(
                        !minted.is_empty()
                            && minted.bytes().all(|byte| byte.is_ascii_digit()
                                || (b'a'..=b'f').contains(&byte)),
                        "case {case:?}: the minted token {minted:?} must be an opaque cursor"
                    );
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: paging must walk every record exactly once, in reverse \
             alphanumeric ExportArn order"
        );
        assert_eq!(
            token, None,
            "case {case:?}: the walk must end on a page that reports no NextToken"
        );
    }
}

#[tokio::test]
async fn a_record_outside_the_documented_window_is_not_reported() {
    let (router, engine) = emulator_and_engine();

    // Rows differ in the AGE of the record: inside the documented 90-day window, exactly
    // on it, and past it. "Lists completed exports within the past 90 days" (the
    // `ListExports` doc), measured from the instant the export finished — or, for one
    // still running, from the instant it began.
    let window = EXPORT_HISTORY_WINDOW;
    let cases: [(&str, Duration, bool, bool); 5] = [
        ("finished a day ago", DAY, true, true),
        ("finished a day inside the window", window - DAY, true, true),
        ("finished exactly a window ago", window, true, false),
        ("finished a day past the window", window + DAY, true, false),
        ("still running, begun two days ago", 2 * DAY, false, true),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, age, finished, reported) in cases {
        let marker = seconds_ago(age);
        let started = marker - Duration::from_secs(60);
        let export = if finished {
            // A finished record's age is measured from the instant its span ended.
            Export {
                completes_at: marker,
                ..completed_export(REGION, "Aged", started)
            }
        } else {
            running_export(REGION, "Aged", started, "FULL_EXPORT")
        };
        let arn = record_export(&engine, export);
        if reported {
            expected.push(arn);
        } else {
            // Named so a failure below says which row was wrong.
            assert!(
                !arn.is_empty(),
                "case {case:?}: the record was given an ExportArn"
            );
        }
    }
    let expected = reverse_alphanumeric(&expected);

    let answered = ok(&router, "ListExports", json!({})).await;
    assert_eq!(
        listed_arns(&answered),
        expected,
        "only records inside the documented window are reported, got {answered}"
    );
}

#[tokio::test]
async fn a_table_arn_that_addresses_no_table_of_this_endpoint_is_refused() {
    let (router, engine) = emulator_and_engine();
    record_export(
        &engine,
        completed_export(REGION, "Alpha", seconds_ago(Duration::from_secs(600))),
    );

    // Rows differ in HOW the value fails to be an ARN of this service naming a table of
    // the endpoint the request reached: not an ARN at all, another service, a deeper
    // sub-resource, another region's table, and an ARN with no resource path.
    let index_arn = format!("{}/index/ByPart", state::table_arn(REGION, "Alpha"));
    let cases: [(&str, String); 5] = [
        ("not an ARN at all", "Alpha".to_string()),
        (
            "an ARN of another service",
            engine::emulator_arn("s3", REGION, "Alpha"),
        ),
        ("an index of the table", index_arn),
        (
            "a table of another region",
            state::table_arn(OTHER_REGION, "Alpha"),
        ),
        (
            "an ARN with no resource path",
            format!(
                "arn:aws:dynamodb:{REGION}:{}:Alpha",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, value) in cases {
        let (status, answered) =
            call(&router, "ListExports", json!({"TableArn": value.clone()})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Invalid TableArn: Invalid TableArn provided as input {value}"),
            "case {case:?}: the refusal states the member and echoes the value sent"
        );
    }
}

#[tokio::test]
async fn a_request_outside_the_members_own_bounds_is_refused() {
    let (router, engine) = emulator_and_engine();
    record_export(
        &engine,
        completed_export(REGION, "Alpha", seconds_ago(Duration::from_secs(600))),
    );

    // Rows differ in which member is out of bounds: the page size either side of the
    // range the model publishes (`ListExportsMaxLimit`, range 1..=25), and a `NextToken`
    // that is not a token this operation mints — not hexadecimal, an odd number of hex
    // digits, hex that denotes no text, and well-formed but standing for no cursor at all.
    let cases: [(&str, Value); 7] = [
        (
            "a page smaller than the member allows",
            json!({"MaxResults": 0}),
        ),
        ("a negative page", json!({"MaxResults": -1})),
        (
            "a page larger than the member allows",
            json!({"MaxResults": 26}),
        ),
        ("a token that is not hexadecimal", json!({"NextToken": "zzzz"})),
        ("a token of odd length", json!({"NextToken": "abc"})),
        ("a token that denotes no text", json!({"NextToken": "ff"})),
        ("a token carrying no cursor", json!({"NextToken": ""})),
    ];

    for (case, body) in cases {
        let (status, answered) = call(&router, "ListExports", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answered}"
        );
    }

    // A token this operation did not mint is refused in the wording this service states
    // for that case, rather than taken for the first page.
    let (_, answered) = call(&router, "ListExports", json!({"NextToken": "zzzz"})).await;
    assert_eq!(
        message(&answered),
        "Invalid NextToken",
        "a token this operation did not mint is refused as an invalid NextToken, got {answered}"
    );
}
