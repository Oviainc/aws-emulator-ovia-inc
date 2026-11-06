// SPDX-License-Identifier: CC0-1.0

//! The landing zone operation log, end to end over HTTP: `ListLandingZoneOperations` over
//! the records the landing zone lifecycle calls append.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies — not the handlers'
//! Rust signatures.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * every call that acts on the account's landing zone appends ONE record to the log, and
//!   the listing reports that record: the call's own `LandingZoneOperationType`, the
//!   `operationIdentifier` the call returned, and `IN_PROGRESS` until the clock makes the
//!   operation due and `SUCCEEDED` after — whatever the call's version, manifest and
//!   remediation types were;
//! * the listing is the stored log narrowed by EVERY member of `filter`, the members
//!   narrowing together, and a value no record carries selects nothing;
//! * the listing reports the records most recently started FIRST and is paginated by
//!   `maxResults`/`nextToken`, so a walk returns each record exactly once in that order,
//!   and a token the service never handed out is refused rather than read as the first
//!   page;
//! * the listing reaches back over the window the operation documents and no further, and
//!   the by-identifier read agrees: a record past that window has stopped being available;
//! * reading the log never changes it, and the log holds landing zone operations only.
//!
//! Every value set and bound written out by hand here names its source.
//! `LandingZoneOperationType` (`DELETE` | `CREATE` | `UPDATE` | `RESET`) and
//! `LandingZoneOperationStatus` (`SUCCEEDED` | `FAILED` | `IN_PROGRESS`) come from
//! `smithy_query(query="shape", shape="LandingZoneOperationType,
//! LandingZoneOperationStatus")`, `maxResults`' range 1..100 and the window the listing
//! reaches back over from `smithy_query(query="operation",
//! operation="ListLandingZoneOperations")`, and each `filter` list's maximum length of 1
//! from `smithy_query(query="shape", shape="LandingZoneOperationFilter")` on this crate's
//! `smithy-model/`. The landing zone versions and manifests the rows vary over are the
//! catalog the create and update calls accept
//! (`service_..._awscontroltowerapis::state::CATALOG_LANDING_ZONE_VERSIONS`).

use std::time::{Duration, SystemTime};

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    service_state, LandingZoneOperationType, OPERATION_DETAILS_AVAILABLE_FOR,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{control, entries, ou};
use crate::front_door_validation::FRONT_DOOR_CODE;
use crate::landing_zone_lifecycle::{
    call, emulator, emulator_with_engine, is_operation_identifier, manifest_governing,
    past_the_settle_span, Answer,
};

/// The paths of the calls that act on the account's landing zone, each from its `@http`
/// trait.
const CREATE: &str = "/create-landingzone";
const UPDATE: &str = "/update-landingzone";
const RESET: &str = "/reset-landingzone";
const DELETE: &str = "/delete-landingzone";

/// One day, the unit the documented listing window is expressed in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// How far back the listing reaches: `ListLandingZoneOperations` lists "all landing zone
/// operations from the past 90 days" (the operation's model docs), which is the window the
/// log retains a record's details for. Read off the crate's own constant rather than
/// retyped here, so this table cannot drift from the rule the store applies;
/// `tests/all/operation_detail_availability_window.rs` pins that constant's documented
/// length.
const LISTED_WINDOW: Duration = OPERATION_DETAILS_AVAILABLE_FOR;

/// Every value of the modelled `LandingZoneOperationType`, with the wire value each
/// serializes as.
const OPERATION_TYPES: &[(LandingZoneOperationType, &str)] = &[
    (LandingZoneOperationType::Create, "CREATE"),
    (LandingZoneOperationType::Update, "UPDATE"),
    (LandingZoneOperationType::Reset, "RESET"),
    (LandingZoneOperationType::Delete, "DELETE"),
];

/// POST `/list-landingzone-operations` with `body`, asserting the call succeeded.
async fn list(router: &MultiServiceRouter, body: serde_json::Value) -> Answer {
    let listed = call(router, "/list-landingzone-operations", body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "list-landingzone-operations {body}: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
}

/// The listed records, in the order the answer reports them.
fn records(listed: &Answer) -> Vec<serde_json::Value> {
    entries(listed.get("landingZoneOperations"))
}

/// A record's member, as a `&str`.
fn text<'a>(record: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    record.get(name).and_then(serde_json::Value::as_str)
}

/// The `operationIdentifier` of every listed record, in the order reported.
fn listed_identifiers(listed: &Answer) -> Vec<String> {
    records(listed)
        .iter()
        .map(|record| {
            text(record, "operationIdentifier")
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The one listed record carrying `operation_identifier`, or `Value::Null`.
fn listed_record(listed: &Answer, operation_identifier: &str) -> serde_json::Value {
    records(listed)
        .into_iter()
        .find(|record| text(record, "operationIdentifier") == Some(operation_identifier))
        .unwrap_or(serde_json::Value::Null)
}

/// Whether `GetLandingZoneOperation` resolves `operation_identifier`.
async fn resolves(router: &MultiServiceRouter, operation_identifier: &str) -> bool {
    call(
        router,
        "/get-landingzone-operation",
        serde_json::json!({ "operationIdentifier": operation_identifier }),
    )
    .await
    .status
        == 200
}

/// One step of the lifecycle table: the call it makes, the members it sends beside the
/// `landingZoneIdentifier` filled in from the account's landing zone, and the operation
/// type the record it appends must report.
struct Step {
    case: &'static str,
    path: &'static str,
    body: serde_json::Value,
    operation_type: &'static str,
}

/// The lifecycle table: every call that appends a landing zone operation record, with its
/// members varying along what the rule generalises over — the deployed `version` (the
/// catalog's oldest, a step inside the 3.x line, its newest), the size and content of the
/// `manifest` (several governed regions, one, none) and whether `remediationTypes` is
/// carried. The sequence runs in order against the account's one landing zone, so the
/// second create is the one the preceding delete made room for.
fn lifecycle() -> Vec<Step> {
    vec![
        Step {
            case: "a create of the catalog's oldest version governing three regions",
            path: CREATE,
            body: serde_json::json!({
                "version": "2.0",
                "manifest": manifest_governing(&["us-west-2", "us-east-1", "eu-west-1"]),
            }),
            operation_type: "CREATE",
        },
        Step {
            case: "an update across the major boundary, with remediation types",
            path: UPDATE,
            body: serde_json::json!({
                "version": "3.0",
                "manifest": manifest_governing(&["us-west-2"]),
                "remediationTypes": ["INHERITANCE_DRIFT"],
            }),
            operation_type: "UPDATE",
        },
        Step {
            case: "an update to the catalog's newest version, carrying no manifest",
            path: UPDATE,
            body: serde_json::json!({ "version": "3.3" }),
            operation_type: "UPDATE",
        },
        Step {
            case: "a reset of the deployed configuration",
            path: RESET,
            body: serde_json::json!({}),
            operation_type: "RESET",
        },
        Step {
            case: "a delete of the account's landing zone",
            path: DELETE,
            body: serde_json::json!({}),
            operation_type: "DELETE",
        },
        Step {
            case: "a create governing no region, after the delete freed the account",
            path: CREATE,
            body: serde_json::json!({
                "version": "3.1",
                "manifest": manifest_governing(&[]),
            }),
            operation_type: "CREATE",
        },
    ]
}

/// The members a `LandingZoneOperationSummary` carries, from
/// `smithy_query(query="shape", shape="LandingZoneOperationSummary")`.
const SUMMARY_MEMBERS: &[&str] = &["operationType", "operationIdentifier", "status"];

/// RULE: every call that acts on the account's landing zone appends one record to the log,
/// and the listing reports that record — the call's own operation type and identifier,
/// `IN_PROGRESS` while the operation is in flight and `SUCCEEDED` once the clock has made
/// it due — whatever the call's version, manifest and remediation types were.
#[tokio::test]
async fn every_landing_zone_call_appends_the_record_the_listing_reports() {
    let router = emulator();
    let mut appended: Vec<(&'static str, String, &'static str)> = Vec::new();
    let mut arn = String::new();

    for step in lifecycle() {
        let mut body = step.body.clone();
        if step.path != CREATE {
            body["landingZoneIdentifier"] = serde_json::json!(arn);
        }
        let answer = call(&router, step.path, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {:?} ({body}): expected 200, got {} body {:?}",
            step.case, answer.status, answer.body
        );
        let operation = answer.string("operationIdentifier");
        assert!(
            is_operation_identifier(&operation),
            "case {:?}: expected a 36-char lowercase-hex operationIdentifier, got \
             {operation:?}",
            step.case
        );
        if step.path == CREATE {
            arn = answer.string("arn");
        }

        // The call "starts an asynchronous operation", so the transient status is
        // observable to a caller acting straight after it. Asserted HERE, on the record
        // this step just appended, rather than after the whole table: the settle span is
        // short on purpose.
        let listed = list(&router, serde_json::json!({})).await;
        let record = listed_record(&listed, &operation);
        assert_eq!(
            (
                text(&record, "operationIdentifier"),
                text(&record, "operationType"),
                text(&record, "status"),
            ),
            (
                Some(operation.as_str()),
                Some(step.operation_type),
                Some("IN_PROGRESS"),
            ),
            "case {:?} while the operation is in flight: expected the record the call \
             wrote, got {record:?}",
            step.case
        );
        let unmodelled: Vec<String> = record
            .as_object()
            .map(|members| {
                members
                    .keys()
                    .filter(|member| !SUMMARY_MEMBERS.contains(&member.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            unmodelled.is_empty(),
            "case {:?}: a summary carries only {SUMMARY_MEMBERS:?}, got {unmodelled:?} \
             besides in {record:?}",
            step.case
        );

        appended.push((step.case, operation.clone(), step.operation_type));

        // The clock makes the operation due; nothing sleeps server-side, the wait is the
        // test's. The next step also needs the landing zone out of `PROCESSING`.
        tokio::time::sleep(past_the_settle_span()).await;

        let settled = list(&router, serde_json::json!({})).await;
        let record = listed_record(&settled, &operation);
        assert_eq!(
            (
                text(&record, "operationType"),
                text(&record, "status"),
            ),
            (Some(step.operation_type), Some("SUCCEEDED")),
            "case {:?} once the operation is due: expected SUCCEEDED and the recorded \
             type unchanged, got {record:?}",
            step.case
        );
    }

    // The log holds exactly one record per call: no call wrote a second record, and none
    // wrote another call's.
    let listed = list(&router, serde_json::json!({})).await;
    let mut got = listed_identifiers(&listed);
    got.sort();
    let mut expected: Vec<String> = appended
        .iter()
        .map(|(_, operation, _)| operation.clone())
        .collect();
    expected.sort();
    assert_eq!(
        got, expected,
        "expected one record per call and nothing else, got {:?}",
        listed.body
    );

    // RULE (the read side): reading the log changes nothing — a second identical listing
    // reports the same records in the same order.
    let reread = list(&router, serde_json::json!({})).await;
    assert_eq!(
        listed_identifiers(&reread),
        listed_identifiers(&listed),
        "re-listing must report the same records in the same order, got {:?}",
        reread.body
    );
}

/// Drive one lifecycle call, asserting it was accepted, and answer its
/// `operationIdentifier`.
async fn start(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    body: serde_json::Value,
) -> Answer {
    let answer = call(router, path, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {path} ({body}) to return 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
}

/// RULE: the listing is the stored log narrowed by EVERY member of `filter`, the members
/// narrowing together, and a value no record carries selects nothing.
///
/// The rows span both members of `LandingZoneOperationFilter` — every value of
/// `LandingZoneOperationType` and of `LandingZoneOperationStatus`, each with a value some
/// record carries and a value none does — plus the two members together. Each filter list
/// is modelled `@length(1..1)`, so every row carries exactly one value per member.
#[tokio::test]
async fn the_listing_is_the_log_narrowed_by_every_filter_member() {
    let router = emulator();

    // One record of each type, every one of them settled, so the status rows below do not
    // race the clock.
    let created = start(
        &router,
        "create",
        CREATE,
        serde_json::json!({ "version": "3.0", "manifest": manifest_governing(&["us-west-2"]) }),
    )
    .await;
    let arn = created.string("arn");
    let create_operation = created.string("operationIdentifier");
    tokio::time::sleep(past_the_settle_span()).await;

    let update_operation = start(
        &router,
        "update",
        UPDATE,
        serde_json::json!({ "landingZoneIdentifier": arn, "version": "3.3" }),
    )
    .await
    .string("operationIdentifier");
    tokio::time::sleep(past_the_settle_span()).await;

    let reset_operation = start(
        &router,
        "reset",
        RESET,
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await
    .string("operationIdentifier");
    tokio::time::sleep(past_the_settle_span()).await;

    let delete_operation = start(
        &router,
        "delete",
        DELETE,
        serde_json::json!({ "landingZoneIdentifier": arn }),
    )
    .await
    .string("operationIdentifier");
    tokio::time::sleep(past_the_settle_span()).await;

    let settled = [
        ("CREATE", create_operation.clone()),
        ("UPDATE", update_operation.clone()),
        ("RESET", reset_operation.clone()),
        ("DELETE", delete_operation.clone()),
    ];
    let sorted = |operations: Vec<String>| {
        let mut operations = operations;
        operations.sort();
        operations
    };
    let all = sorted(
        settled
            .iter()
            .map(|(_, operation)| operation.clone())
            .collect(),
    );
    let none: Vec<String> = Vec::new();

    let mut cases: Vec<(String, serde_json::Value, Vec<String>)> = vec![
        (
            "no filter at all".to_string(),
            serde_json::json!({}),
            all.clone(),
        ),
        (
            "an empty filter narrows nothing".to_string(),
            serde_json::json!({ "filter": {} }),
            all.clone(),
        ),
        (
            "statuses SUCCEEDED, which every settled record carries".to_string(),
            serde_json::json!({ "filter": { "statuses": ["SUCCEEDED"] } }),
            all.clone(),
        ),
        (
            "statuses IN_PROGRESS, which no settled record carries".to_string(),
            serde_json::json!({ "filter": { "statuses": ["IN_PROGRESS"] } }),
            none.clone(),
        ),
        (
            "statuses FAILED, which nothing in this slice produces".to_string(),
            serde_json::json!({ "filter": { "statuses": ["FAILED"] } }),
            none.clone(),
        ),
    ];
    // Every value of the modelled type enum: the one record carrying it, and under a
    // status no record carries, nothing.
    for (_, wire) in OPERATION_TYPES {
        let carried: Vec<String> = settled
            .iter()
            .filter(|(recorded, _)| recorded == wire)
            .map(|(_, operation)| operation.clone())
            .collect();
        cases.push((
            format!("types {wire}, which one record carries"),
            serde_json::json!({ "filter": { "types": [wire] } }),
            sorted(carried.clone()),
        ));
        cases.push((
            format!("types {wire} and statuses SUCCEEDED together"),
            serde_json::json!({
                "filter": { "types": [wire], "statuses": ["SUCCEEDED"] }
            }),
            sorted(carried),
        ));
        cases.push((
            format!("types {wire} and statuses IN_PROGRESS, which no record satisfies"),
            serde_json::json!({
                "filter": { "types": [wire], "statuses": ["IN_PROGRESS"] }
            }),
            none.clone(),
        ));
    }

    for (case, body, expected) in cases {
        let listed = list(&router, body.clone()).await;
        let got = sorted(listed_identifiers(&listed));
        assert_eq!(
            got, expected,
            "case {case:?} ({body}): expected {expected:?}, got {:?}",
            listed.body
        );
    }

    // A record still in flight is selected by `IN_PROGRESS` and by nothing else: the
    // account is free again, so a fresh create starts one. Asserted straight after the
    // call, inside the settle span.
    let in_flight = start(
        &router,
        "a create whose operation is still in flight",
        CREATE,
        serde_json::json!({ "version": "3.2", "manifest": manifest_governing(&["eu-west-1"]) }),
    )
    .await
    .string("operationIdentifier");
    for (case, body, expected) in [
        (
            "statuses IN_PROGRESS, which only the in-flight record carries",
            serde_json::json!({ "filter": { "statuses": ["IN_PROGRESS"] } }),
            vec![in_flight.clone()],
        ),
        (
            "types CREATE and statuses IN_PROGRESS together",
            serde_json::json!({ "filter": { "types": ["CREATE"], "statuses": ["IN_PROGRESS"] } }),
            vec![in_flight.clone()],
        ),
        (
            "types DELETE and statuses IN_PROGRESS, which no record satisfies",
            serde_json::json!({ "filter": { "types": ["DELETE"], "statuses": ["IN_PROGRESS"] } }),
            none.clone(),
        ),
    ] {
        let listed = list(&router, body.clone()).await;
        let got = sorted(listed_identifiers(&listed));
        assert_eq!(
            got,
            sorted(expected.clone()),
            "case {case:?} ({body}): expected {expected:?}, got {:?}",
            listed.body
        );
    }
}

/// RULE (the bound around the one above): a `filter` list longer than its modelled maximum
/// of 1 is refused at the front door, so the narrowing rule is only ever reached with a
/// list inside that length.
#[tokio::test]
async fn a_filter_list_past_its_modelled_maximum_is_refused() {
    let router = emulator();

    for (case, body) in [
        (
            "two types, one past LandingZoneOperationFilter's maximum of 1",
            serde_json::json!({ "filter": { "types": ["CREATE", "DELETE"] } }),
        ),
        (
            "two statuses, one past LandingZoneOperationFilter's maximum of 1",
            serde_json::json!({ "filter": { "statuses": ["SUCCEEDED", "FAILED"] } }),
        ),
        (
            "an empty types list, one below that maximum's minimum of 1",
            serde_json::json!({ "filter": { "types": [] } }),
        ),
    ] {
        let refused = call(&router, "/list-landingzone-operations", body.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?} ({body}): expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }
}

/// A landing zone ARN of the form this service mints, for the records a case appends to the
/// log directly.
const SEEDED_ARN: &str = "arn:aws:controltower:us-west-2:000000000000:landingzone/listing";

/// Append one record per `(type, start time)` pair straight to the log, answering their
/// identifiers in the order given.
///
/// The cases below are about how the listing ORDERS and WINDOWS records whose start times
/// lie seconds and months apart, which no sequence of calls inside one test could produce;
/// everything else here drives the log through the wire path.
fn seed(
    engine: &engine::InMemoryEngine,
    records: &[(LandingZoneOperationType, SystemTime)],
) -> Vec<String> {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    records
        .iter()
        .map(|(operation_type, at)| store.record_operation(*operation_type, SEEDED_ARN, *at))
        .collect()
}

/// RULE: the listing reports the records most recently started first, and is paginated by
/// `maxResults`/`nextToken` — a walk in pages of at most `maxResults` returns every record
/// exactly once in that order and ends when the answer carries no `nextToken`. A token the
/// service never handed out is refused rather than read as the first page.
///
/// The rows are the page sizes: both published bounds of
/// `ListLandingZoneOperationsMaxResults` (range 1..100), a size that divides the record set
/// evenly, one that does not, and one equal to the set.
#[tokio::test]
async fn a_paginated_walk_returns_each_record_exactly_once_newest_first() {
    let (router, engine) = emulator_with_engine();

    // Six records a second apart, cycling every operation type: increasing start times
    // order them totally, so which record is "most recent" is unambiguous. Seeded in the
    // past, so the clock has made every one of them due.
    let base = SystemTime::now() - Duration::from_secs(3600);
    let seeded: Vec<(LandingZoneOperationType, SystemTime)> = (0..6usize)
        .map(|step| {
            (
                OPERATION_TYPES[step % OPERATION_TYPES.len()].0,
                base + Duration::from_secs(step as u64),
            )
        })
        .collect();
    let appended = seed(&engine, &seeded);

    let whole = list(&router, serde_json::json!({})).await;
    let order = listed_identifiers(&whole);
    let newest_first: Vec<String> = appended.iter().rev().cloned().collect();
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
    for (case, token) in [
        ("a token that is not a number at all", "not-a-token"),
        ("a token past the end of the listing", "99"),
    ] {
        let refused = call(
            &router,
            "/list-landingzone-operations",
            serde_json::json!({ "nextToken": token }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({token}): expected 400 ValidationException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }

    // One past each published bound of `maxResults` is refused at the front door, so the
    // page sizes walked above are the whole modelled range.
    for (case, max_results) in [
        ("maxResults one below the range minimum of 1", 0),
        ("maxResults one above the range maximum of 100", 101),
    ] {
        let refused = call(
            &router,
            "/list-landingzone-operations",
            serde_json::json!({ "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?}: expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }
}

/// RULE: the listing reaches back over the window the operation documents and no further —
/// a record that started inside it is listed, an older one is not, and the by-identifier
/// read agrees: a record past the window has stopped being available, so it resolves no
/// more than one the listing reports.
///
/// The rows are the ages: inside the window at both ends, just inside its published bound,
/// and just, a day and a month past it.
#[tokio::test]
async fn the_listing_reaches_back_over_the_documented_window_only() {
    let (router, engine) = emulator_with_engine();
    let now = SystemTime::now();

    // (case, how far back the record started, whether the listing reports it)
    let rows: Vec<(&str, Duration, bool)> = vec![
        ("started moments ago", Duration::from_secs(1), true),
        ("a day old", DAY, true),
        ("a month old", 30 * DAY, true),
        (
            "an hour inside the window",
            LISTED_WINDOW - Duration::from_secs(3600),
            true,
        ),
        (
            "a minute past the window",
            LISTED_WINDOW + Duration::from_secs(60),
            false,
        ),
        ("a day past the window", LISTED_WINDOW + DAY, false),
        ("a month past the window", LISTED_WINDOW + 30 * DAY, false),
    ];

    let seeded: Vec<(LandingZoneOperationType, SystemTime)> = rows
        .iter()
        .enumerate()
        .map(|(index, (_, age, _))| {
            (
                OPERATION_TYPES[index % OPERATION_TYPES.len()].0,
                now - *age,
            )
        })
        .collect();
    let appended = seed(&engine, &seeded);

    let listed = list(&router, serde_json::json!({})).await;
    let reported = listed_identifiers(&listed);
    for ((case, age, inside), operation) in rows.iter().zip(appended.iter()) {
        assert_eq!(
            reported.contains(operation),
            *inside,
            "case {case:?} ({age:?} old): expected the listing to report it: {inside}, got \
             {:?}",
            listed.body
        );
        // One window over one log: a record the listing no longer reaches is a record
        // whose details have stopped being available, so it does not resolve either.
        assert_eq!(
            resolves(&router, operation).await,
            *inside,
            "case {case:?} ({age:?} old): expected get-landingzone-operation to resolve \
             {operation}: {inside}"
        );
    }

    // A filter does not widen the window either.
    let filtered = list(
        &router,
        serde_json::json!({ "filter": { "statuses": ["SUCCEEDED"] } }),
    )
    .await;
    let filtered_identifiers = listed_identifiers(&filtered);
    for ((case, _, inside), operation) in rows.iter().zip(appended.iter()) {
        assert_eq!(
            filtered_identifiers.contains(operation),
            *inside,
            "case {case:?} under a filter: expected the listing to report it: {inside}, got \
             {:?}",
            filtered.body
        );
    }
}

/// RULE: the log is the account's own landing zone history — before any call acts on a
/// landing zone the listing is empty (an answer with no records, not a refusal), and an
/// identifier another of this service's logs minted is not a landing zone operation.
#[tokio::test]
async fn the_log_starts_empty_and_holds_only_landing_zone_operations() {
    let router = emulator();

    let empty = list(&router, serde_json::json!({})).await;
    assert_eq!(
        records(&empty),
        Vec::<serde_json::Value>::new(),
        "a fresh account: expected no landing zone operations, got {:?}",
        empty.body
    );

    // A control enablement mints an identifier in a DIFFERENT log, so it is not a landing
    // zone operation and this listing does not report it. Control Tower enables a control
    // only for an account it has been set up in, so the CREATE that sets it up is the one
    // record this log holds by then.
    let created = call(
        &router,
        CREATE,
        serde_json::json!({ "version": "3.3", "manifest": manifest_governing(&["us-west-2"]) }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create-landingzone: expected 200, got {} body {:?}",
        created.status, created.body
    );
    let create_operation = created.string("operationIdentifier");
    let enabled = call(
        &router,
        "/enable-control",
        serde_json::json!({
            "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
            "targetIdentifier": ou("dd01"),
        }),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "enable-control: expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let control_operation = enabled.string("operationIdentifier");

    let listed = list(&router, serde_json::json!({})).await;
    assert_eq!(
        listed_identifiers(&listed),
        vec![create_operation],
        "after a control call: expected only the landing zone CREATE, got {:?}",
        listed.body
    );
    assert_eq!(
        listed_record(&listed, &control_operation),
        serde_json::Value::Null,
        "the control log's identifier must not appear among the landing zone operations, \
         got {:?}",
        listed.body
    );
}
