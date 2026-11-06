// SPDX-License-Identifier: CC0-1.0

//! The control operation log, end to end over HTTP: `ListControlOperations` over the
//! records `EnableControl` appends.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a call that acts on an `EnabledControl` appends ONE record to the control operation
//!   log, and the listing reports that record: the call's own `ControlOperationType`, the
//!   `operationIdentifier` the call returned, the control, the target and the enabled
//!   control it acted on, `IN_PROGRESS` with no `endTime` until the clock makes it due and
//!   `SUCCEEDED` with a stable `endTime` after — for any catalogued control,
//!   organizational-unit target, declared parameter list and tag map;
//! * the listing is the stored log narrowed by EVERY member of `filter`, and a value no
//!   record carries selects nothing;
//! * the listing is paginated by `maxResults`/`nextToken`, so a walk returns each record
//!   exactly once in the order the unpaginated listing reports, and a token the service
//!   never handed out is refused rather than read as the first page;
//! * reading the log never changes it: two identical calls report the same records.
//!
//! Every value set and bound written out by hand here names its source.
//! `ControlOperationType` (`ENABLE_CONTROL` | `DISABLE_CONTROL` |
//! `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL`) and `ControlOperationStatus`
//! (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`) come from
//! `smithy_query(query="shape", shape="ControlOperationType,ControlOperationStatus")`, and
//! `maxResults`' range 1..100 from
//! `smithy_query(query="operation", operation="ListControlOperations")` on this crate's
//! `smithy-model/`. `EnableControl` is the only call in this slice that appends a record,
//! so the other three types — and the `FAILED` status — are asserted here as selecting
//! NOTHING; they belong to the `DisableControl` / `UpdateEnabledControl` /
//! `ResetEnabledControl` stub issues.

use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{control, enable, entries, ou};
use crate::landing_zone_lifecycle::{call, emulator, past_the_settle_span, set_up_emulator, Answer};

/// POST `/list-control-operations` with `body`, asserting the call succeeded.
async fn list(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    let listed = call(router, "/list-control-operations", body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "list-control-operations {body}: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
}

/// The listed records, in the order the answer reports them.
fn records(listed: &Answer) -> Vec<serde_json::Value> {
    entries(listed.get("controlOperations"))
}

/// A record's member, as a `&str`.
fn text<'a>(record: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    record.get(name).and_then(serde_json::Value::as_str)
}

/// The `operationIdentifier` of every listed record, in the order reported.
fn listed_identifiers(listed: &Answer) -> Vec<String> {
    records(listed)
        .iter()
        .map(|record| text(record, "operationIdentifier").unwrap_or_default().to_string())
        .collect()
}

/// The one listed record carrying `operation_identifier`, or `Value::Null`.
fn listed_record(listed: &Answer, operation_identifier: &str) -> serde_json::Value {
    records(listed)
        .into_iter()
        .find(|record| text(record, "operationIdentifier") == Some(operation_identifier))
        .unwrap_or(serde_json::Value::Null)
}

/// One row of the enablement table: the case name and the `EnableControl` members it
/// varies. `parameters` and `tags` are `Value::Null` when the row omits the member.
struct Row {
    case: &'static str,
    control_identifier: String,
    target_identifier: String,
    parameters: serde_json::Value,
    tags: serde_json::Value,
}

/// The enablement table: the rows differ along what the rule generalises over — the
/// control, the target, the parameter list (absent, empty, one, every key the control
/// declares) and the tag map (absent, empty, several). Every row names a different
/// control/target pair, because a control is enabled at most once per target.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no parameters and no tags, on an OU target",
            control_identifier: control("AWS-GR_ENCRYPTED_VOLUMES"),
            target_identifier: ou("bb01"),
            parameters: serde_json::Value::Null,
            tags: serde_json::Value::Null,
        },
        Row {
            case: "an empty parameter list and an empty tag map, on another OU",
            control_identifier: control("AWS-GR_RESTRICT_ROOT_USER"),
            target_identifier: ou("bb04"),
            parameters: serde_json::json!([]),
            tags: serde_json::json!({}),
        },
        Row {
            case: "one declared parameter and one tag",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("bb02"),
            parameters: serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
            tags: serde_json::json!({ "owner": "platform" }),
        },
        Row {
            case: "every declared parameter, covering several document kinds, on another OU",
            control_identifier: control("AWS-GR_REGION_DENY"),
            target_identifier: ou("bb03"),
            parameters: serde_json::json!([
                { "key": "AllowedRegions", "value": ["us-west-2", "eu-west-1"] },
                { "key": "ExemptedPrincipalArns", "value": "arn:aws:iam::000000000000:role/r" },
                { "key": "ExemptedActions", "value": { "inner": { "deeper": [1, 2] } } }
            ]),
            tags: serde_json::json!({ "env": "test", "cost-centre": "4711" }),
        },
    ]
}

/// `EnableControl` for one row, handing back `(arn, operationIdentifier)`.
async fn enable_row(router: &MultiServiceRouter, row: &Row) -> (String, String) {
    let mut body = serde_json::json!({
        "controlIdentifier": row.control_identifier,
        "targetIdentifier": row.target_identifier,
    });
    if !row.parameters.is_null() {
        body["parameters"] = row.parameters.clone();
    }
    if !row.tags.is_null() {
        body["tags"] = row.tags.clone();
    }
    enable(router, body).await
}

/// RULE: a call that acts on an `EnabledControl` appends one record to the control
/// operation log, and the listing reports that record — the call's own operation type and
/// identifier, the control, target and enabled control it acted on, `IN_PROGRESS` with no
/// `endTime` while it is in flight and `SUCCEEDED` with a stable `endTime` once the clock
/// has made it due — whatever the enablement's members were.
#[tokio::test]
async fn an_enable_control_call_is_the_record_the_listing_reports() {
    let router = set_up_emulator().await;
    let mut started: Vec<(&'static str, Row, String, String)> = Vec::new();

    for row in rows() {
        let (arn, operation_identifier) = enable_row(&router, &row).await;

        // `EnableControl` "starts an asynchronous operation", so the transient state is
        // observable to a caller acting straight after the call. Asserted HERE, on the
        // enablement this iteration just started, rather than after the whole table: the
        // settle span is short on purpose, so a case that enabled every row first could
        // race the clock.
        let listed = list(&router, serde_json::json!({})).await;
        let record = listed_record(&listed, &operation_identifier);
        assert_eq!(
            (
                text(&record, "operationIdentifier"),
                text(&record, "operationType"),
                text(&record, "status"),
                text(&record, "controlIdentifier"),
                text(&record, "targetIdentifier"),
                text(&record, "enabledControlIdentifier"),
            ),
            (
                Some(operation_identifier.as_str()),
                Some("ENABLE_CONTROL"),
                Some("IN_PROGRESS"),
                Some(row.control_identifier.as_str()),
                Some(row.target_identifier.as_str()),
                Some(arn.as_str()),
            ),
            "case {:?} while the operation is in flight: expected the record the call \
             wrote, got {record:?}",
            row.case
        );
        assert!(
            record.get("startTime").is_some(),
            "case {:?}: expected a startTime, got {record:?}",
            row.case
        );
        assert!(
            record.get("endTime").is_none(),
            "case {:?}: expected no endTime while the operation is in flight, got \
             {record:?}",
            row.case
        );
        assert!(
            record.get("statusMessage").is_none(),
            "case {:?}: a statusMessage is reported in case of failure only, got \
             {record:?}",
            row.case
        );

        started.push((row.case, row, arn, operation_identifier));
    }

    // The log holds exactly one record per call: no call wrote a second record, and none
    // wrote another call's.
    let listed = list(&router, serde_json::json!({})).await;
    let mut listed_all = listed_identifiers(&listed);
    listed_all.sort();
    let mut expected: Vec<String> = started
        .iter()
        .map(|(_, _, _, operation)| operation.clone())
        .collect();
    expected.sort();
    assert_eq!(
        listed_all, expected,
        "expected one record per call and nothing else, got {:?}",
        listed.body
    );

    // The clock makes every operation due; nothing sleeps server-side, the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    let settled = list(&router, serde_json::json!({})).await;
    for (case, row, arn, operation_identifier) in &started {
        let record = listed_record(&settled, operation_identifier);
        assert_eq!(
            (
                text(&record, "operationType"),
                text(&record, "status"),
                text(&record, "controlIdentifier"),
                text(&record, "targetIdentifier"),
                text(&record, "enabledControlIdentifier"),
            ),
            (
                Some("ENABLE_CONTROL"),
                Some("SUCCEEDED"),
                Some(row.control_identifier.as_str()),
                Some(row.target_identifier.as_str()),
                Some(arn.as_str()),
            ),
            "case {case:?} once the operation is due: expected SUCCEEDED and the recorded \
             identifiers unchanged, got {record:?}"
        );
        let end_time = record.get("endTime").cloned();
        assert!(
            end_time.is_some(),
            "case {case:?}: expected an endTime once the operation is due, got {record:?}"
        );

        // RULE (the read side): reading the log changes nothing — a terminal record is
        // stable, so a second listing reports the same endTime rather than re-stamping it.
        let reread = list(&router, serde_json::json!({})).await;
        assert_eq!(
            listed_record(&reread, operation_identifier)
                .get("endTime")
                .cloned(),
            end_time,
            "case {case:?}: re-listing must report the same endTime, got {:?}",
            listed_record(&reread, operation_identifier)
        );
    }
}

/// RULE: the listing is the stored log narrowed by EVERY member of `filter`, the members
/// narrowing together, and a value no record carries selects nothing.
///
/// The rows span every member of `ControlOperationFilter`, each with a value some record
/// carries and a value none does, plus two members together. Each filter list is modelled
/// `@length(1..1)`, so every row carries exactly one value per member.
#[tokio::test]
async fn the_listing_is_the_log_narrowed_by_every_filter_member() {
    let router = set_up_emulator().await;

    let mut enabled: Vec<(String, String, String, String)> = Vec::new();
    for row in rows() {
        let (arn, operation) = enable_row(&router, &row).await;
        enabled.push((
            row.control_identifier.clone(),
            row.target_identifier.clone(),
            arn,
            operation,
        ));
    }

    // Past the settle span, so every record is terminal and the status rows below do not
    // race the clock.
    tokio::time::sleep(past_the_settle_span()).await;

    /// The operations of the rows `predicate` selects, sorted.
    fn expected(
        enabled: &[(String, String, String, String)],
        predicate: impl Fn(&(String, String, String, String)) -> bool,
    ) -> Vec<String> {
        let mut operations: Vec<String> = enabled
            .iter()
            .filter(|row| predicate(row))
            .map(|(_, _, _, operation)| operation.clone())
            .collect();
        operations.sort();
        operations
    }

    let first_control = enabled[0].0.clone();
    let first_target = enabled[0].1.clone();
    let third_arn = enabled[2].2.clone();
    let all = expected(&enabled, |_| true);
    let none: Vec<String> = Vec::new();

    let cases: Vec<(&str, serde_json::Value, Vec<String>)> = vec![
        ("no filter at all", serde_json::json!({}), all.clone()),
        (
            "an empty filter narrows nothing",
            serde_json::json!({ "filter": {} }),
            all.clone(),
        ),
        (
            "controlIdentifiers on a control some record carries",
            serde_json::json!({ "filter": { "controlIdentifiers": [first_control] } }),
            expected(&enabled, |row| row.0 == first_control),
        ),
        (
            "controlIdentifiers on a control no record carries",
            serde_json::json!({
                "filter": { "controlIdentifiers": [control("AWS-GR_RDS_STORAGE_ENCRYPTED")] }
            }),
            none.clone(),
        ),
        (
            "targetIdentifiers on a target some record carries",
            serde_json::json!({ "filter": { "targetIdentifiers": [first_target] } }),
            expected(&enabled, |row| row.1 == first_target),
        ),
        (
            "targetIdentifiers on a target no record carries",
            serde_json::json!({ "filter": { "targetIdentifiers": [ou("ffff")] } }),
            none.clone(),
        ),
        (
            "enabledControlIdentifiers on an enablement some record acted on",
            serde_json::json!({ "filter": { "enabledControlIdentifiers": [third_arn] } }),
            expected(&enabled, |row| row.2 == third_arn),
        ),
        (
            "enabledControlIdentifiers on an ARN no record acted on",
            serde_json::json!({
                "filter": { "enabledControlIdentifiers": [ou("eeee")] }
            }),
            none.clone(),
        ),
        (
            "statuses SUCCEEDED, which every settled record carries",
            serde_json::json!({ "filter": { "statuses": ["SUCCEEDED"] } }),
            all.clone(),
        ),
        (
            "statuses IN_PROGRESS, which no settled record carries",
            serde_json::json!({ "filter": { "statuses": ["IN_PROGRESS"] } }),
            none.clone(),
        ),
        (
            "statuses FAILED, which nothing in this slice produces",
            serde_json::json!({ "filter": { "statuses": ["FAILED"] } }),
            none.clone(),
        ),
        (
            "controlOperationTypes ENABLE_CONTROL, the type every record carries",
            serde_json::json!({ "filter": { "controlOperationTypes": ["ENABLE_CONTROL"] } }),
            all.clone(),
        ),
        (
            "controlOperationTypes DISABLE_CONTROL, which no call here produced",
            serde_json::json!({ "filter": { "controlOperationTypes": ["DISABLE_CONTROL"] } }),
            none.clone(),
        ),
        (
            "controlOperationTypes UPDATE_ENABLED_CONTROL, which no call here produced",
            serde_json::json!({
                "filter": { "controlOperationTypes": ["UPDATE_ENABLED_CONTROL"] }
            }),
            none.clone(),
        ),
        (
            "controlOperationTypes RESET_ENABLED_CONTROL, which no call here produced",
            serde_json::json!({
                "filter": { "controlOperationTypes": ["RESET_ENABLED_CONTROL"] }
            }),
            none.clone(),
        ),
        (
            "the control and the target together, selecting their one record",
            serde_json::json!({
                "filter": {
                    "controlIdentifiers": [first_control],
                    "targetIdentifiers": [first_target],
                }
            }),
            expected(&enabled, |row| row.0 == first_control && row.1 == first_target),
        ),
        (
            "a control and a target no ONE record carries together",
            serde_json::json!({
                "filter": {
                    "controlIdentifiers": [first_control],
                    "targetIdentifiers": [enabled[2].1.clone()],
                }
            }),
            none.clone(),
        ),
        (
            "every member together, selecting the one record that satisfies them all",
            serde_json::json!({
                "filter": {
                    "controlIdentifiers": [enabled[2].0.clone()],
                    "targetIdentifiers": [enabled[2].1.clone()],
                    "enabledControlIdentifiers": [third_arn],
                    "statuses": ["SUCCEEDED"],
                    "controlOperationTypes": ["ENABLE_CONTROL"],
                }
            }),
            expected(&enabled, |row| row.2 == third_arn),
        ),
        (
            "every member together, with one member no record satisfies",
            serde_json::json!({
                "filter": {
                    "controlIdentifiers": [enabled[2].0.clone()],
                    "targetIdentifiers": [enabled[2].1.clone()],
                    "enabledControlIdentifiers": [third_arn],
                    "statuses": ["IN_PROGRESS"],
                    "controlOperationTypes": ["ENABLE_CONTROL"],
                }
            }),
            none.clone(),
        ),
    ];

    for (case, body, expected) in cases {
        let listed = list(&router, body.clone()).await;
        let mut got = listed_identifiers(&listed);
        got.sort();
        assert_eq!(
            got, expected,
            "case {case:?} ({body}): expected {expected:?}, got {:?}",
            listed.body
        );
    }
}

/// RULE: the listing is paginated — a walk in pages of at most `maxResults` returns every
/// record the narrowing selected exactly once, in the order the unpaginated listing
/// reports them, and ends when the answer carries no `nextToken`. A token the service
/// never handed out is refused rather than read as the first page.
///
/// The rows are the page sizes: both published bounds of `ListControlOperationsMaxResults`
/// (range 1..100), a size that divides the record set evenly, one that does not, and one
/// equal to the set.
#[tokio::test]
async fn a_paginated_walk_returns_each_record_exactly_once() {
    let router = set_up_emulator().await;

    // Six records, so a page size of 2 and 3 divide the set and 4 does not. One catalogued
    // control on six organizational units: a control is enabled at most once per target.
    let mut started: Vec<String> = Vec::new();
    for index in 0..6usize {
        let (_, operation) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou(&format!("cc{index:02}")),
            }),
        )
        .await;
        started.push(operation);
    }

    // The unpaginated order is the one a walk must reproduce: most recently started
    // first, which is the reverse of the order the calls were made in.
    let whole = list(&router, serde_json::json!({})).await;
    let order = listed_identifiers(&whole);
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();
    assert_eq!(
        order, newest_first,
        "the unpaginated listing: expected the records most recently started first, got \
         {:?}",
        whole.body
    );

    for page_size in [1usize, 2, 3, 4, 6, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = serde_json::json!({ "maxResults": page_size });
            if let Some(token) = &token {
                body["nextToken"] = serde_json::json!(token);
            }
            let page = list(&router, body).await;
            let identifiers = listed_identifiers(&page);
            assert!(
                identifiers.len() <= page_size,
                "page size {page_size}: expected at most {page_size} records in a page, \
                 got {identifiers:?}"
            );
            walked.extend(identifiers);
            token = page.get("nextToken").as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= order.len() + 1,
                "page size {page_size}: the walk did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, order,
            "page size {page_size}: expected every record exactly once in the listing's \
             order, got {walked:?}"
        );
        let full_pages = order.len().div_ceil(page_size);
        assert_eq!(
            pages, full_pages,
            "page size {page_size}: expected {full_pages} pages for {} records, got {pages}",
            order.len()
        );
    }

    // A token this operation never handed out is refused, not silently read as the first
    // page — the same rule this service's other listings apply to their own tokens.
    let refused = call(
        &router,
        "/list-control-operations",
        serde_json::json!({ "nextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "a token the service never handed out: expected 400 ValidationException, got {:?} \
         body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
}

/// RULE: the log is the account's own, so before any call acts on a control the listing
/// is empty — an answer with no records, not a refusal — and an identifier another of this
/// service's logs minted is not a control operation.
#[tokio::test]
async fn the_log_starts_empty_and_holds_only_control_operations() {
    use crate::landing_zone_lifecycle::simple_manifest;

    let router = emulator();

    let empty = list(&router, serde_json::json!({})).await;
    assert_eq!(
        records(&empty),
        Vec::<serde_json::Value>::new(),
        "a fresh account: expected no control operations, got {:?}",
        empty.body
    );

    // A landing zone call mints an identifier in a DIFFERENT log, so it is not a control
    // operation and the control listing does not report it.
    let created = call(
        &router,
        "/create-landingzone",
        serde_json::json!({ "version": "3.3", "manifest": simple_manifest() }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create-landingzone: expected 200, got {} body {:?}",
        created.status, created.body
    );
    let landing_zone_operation = created.string("operationIdentifier");

    let listed = list(&router, serde_json::json!({})).await;
    assert_eq!(
        listed_identifiers(&listed),
        Vec::<String>::new(),
        "after a landing zone call: expected no control operations, got {:?}",
        listed.body
    );
    assert_eq!(
        listed_record(&listed, &landing_zone_operation),
        serde_json::Value::Null,
        "the landing zone log's identifier must not appear among the control operations, \
         got {:?}",
        listed.body
    );
}
