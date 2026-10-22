// SPDX-License-Identifier: CC0-1.0

//! `ListOperations` end to end: the page it answers with is computed from the
//! operation records this service's mutating calls filed, narrowed by every supplied
//! filter, and resumed through the `NextToken` it mints.
//!
//! The tests assert the RULE rather than one recorded request: each filter name and
//! condition the model declares is exercised as a row of a table, paging is swept over
//! several page sizes and record counts, and every combination the documented
//! name/condition matrix does not list is refused.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`); the
//! dispatch harness is the shared one in `cloud_map_core`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloud_map_core::{emulator_with_engine, failure, namespace, ok, service};

/// Register an instance and return the `OperationId` the call filed.
async fn register(
    router: &MultiServiceRouter,
    service_id: &str,
    instance_id: &str,
) -> String {
    let registered = ok(
        router,
        "RegisterInstance",
        json!({
            "ServiceId": service_id,
            "InstanceId": instance_id,
            "Attributes": { "AWS_INSTANCE_IPV4": "10.0.0.1" },
        }),
    )
    .await;
    registered["OperationId"]
        .as_str()
        .expect("RegisterInstance returns an OperationId")
        .to_string()
}

/// The `(Id, Status)` pairs one `ListOperations` response carries, in response order.
fn summaries(response: &Value) -> Vec<(String, String)> {
    response["Operations"]
        .as_array()
        .unwrap_or_else(|| panic!("Operations must be a list, got {response}"))
        .iter()
        .map(|summary| {
            (
                summary["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("every summary carries an Id, got {summary}"))
                    .to_string(),
                summary["Status"]
                    .as_str()
                    .unwrap_or_else(|| panic!("every summary carries a Status, got {summary}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The `OperationId`s one `ListOperations` response carries.
fn ids(response: &Value) -> BTreeSet<String> {
    summaries(response)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Longer than [`cloudmap::state::OPERATION_SUCCESS_SPAN`], so every operation filed
/// before the sleep has reached its terminal status by the time the list is read.
fn settle() {
    // Blocking is fine here: the test owns the runtime, and the service never sleeps
    // for this span — it stores the due instant and materialises it on read.
    std::thread::sleep(cloudmap::state::OPERATION_SUCCESS_SPAN + Duration::from_millis(50));
}

/// Every operation a mutating call files is reported by `ListOperations`, with the same
/// status `GetOperation` reports for it, and listing twice reports the same thing:
/// a read never changes what a later caller sees.
#[tokio::test]
async fn list_operations_reports_every_record_the_mutating_calls_filed() {
    let (router, _engine) = emulator_with_engine();

    let (create_namespace, namespace_id) = namespace(&router, "listops-ns").await;
    let service_id = service(&router, &namespace_id, "listops-svc").await;
    let register_instance = register(&router, &service_id, "i-list-1").await;
    let deregistered = ok(
        &router,
        "DeregisterInstance",
        json!({ "ServiceId": service_id, "InstanceId": "i-list-1" }),
    )
    .await;
    let deregister_instance = deregistered["OperationId"]
        .as_str()
        .expect("DeregisterInstance returns an OperationId")
        .to_string();
    settle();

    let expected: BTreeSet<String> = [
        create_namespace.clone(),
        register_instance.clone(),
        deregister_instance.clone(),
    ]
    .into_iter()
    .collect();

    let listed = ok(&router, "ListOperations", json!({})).await;
    assert_eq!(
        ids(&listed),
        expected,
        "case all-operations: expected {expected:?}, got {listed}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "case all-operations: one page holds every record, so no NextToken is due, got {listed}"
    );

    // Each reported status is the one the operation's own record carries.
    for (id, status) in summaries(&listed) {
        let fetched = ok(&router, "GetOperation", json!({ "OperationId": id })).await;
        assert_eq!(
            fetched["Operation"]["Status"],
            json!(status),
            "case {id}: ListOperations reported {status}, GetOperation reports {fetched}"
        );
    }

    let again = ok(&router, "ListOperations", json!({})).await;
    assert_eq!(
        summaries(&again),
        summaries(&listed),
        "case repeated-read: listing must not change what a later caller sees, got {again}"
    );
}

/// Each filter name and condition the model declares selects the records the
/// documented matrix says it does, and several filters intersect: "If you specify more
/// than one filter, an operation must match all filters to be returned".
#[tokio::test]
async fn every_filter_name_and_condition_selects_the_matching_operations() {
    let (router, engine) = emulator_with_engine();

    let (create_a, namespace_a) = namespace(&router, "filters-ns-a").await;
    let (create_b, namespace_b) = namespace(&router, "filters-ns-b").await;
    let service_a = service(&router, &namespace_a, "filters-svc-a").await;
    let service_b = service(&router, &namespace_b, "filters-svc-b").await;
    let register_a = register(&router, &service_a, "i-a").await;
    let register_b = register(&router, &service_b, "i-b").await;
    settle();
    // One record whose success the clock does not owe yet, so the two STATUS rows
    // below are decided by the stored due instant rather than by how long the test's
    // own requests take.
    let pending = seed_pending(
        &engine,
        "REGISTER_INSTANCE",
        [
            ("NAMESPACE".to_string(), namespace_a.clone()),
            ("SERVICE".to_string(), service_a.clone()),
            ("INSTANCE".to_string(), "i-a-2".to_string()),
        ],
    );

    let all: BTreeSet<String> = [
        create_a.clone(),
        create_b.clone(),
        register_a.clone(),
        register_b.clone(),
        pending.clone(),
    ]
    .into_iter()
    .collect();
    let settled: BTreeSet<String> = all.iter().filter(|id| **id != pending).cloned().collect();
    let set = |ids: [&String; 2]| -> BTreeSet<String> {
        ids.into_iter().cloned().collect()
    };

    // A date range that spans every record, in the Unix date/time format
    // `OperationFilter$Condition` documents for `BETWEEN`.
    let far_future = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs()
        + 3_600;

    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("no filters", json!([]), all.clone()),
        (
            "NAMESPACE_ID EQ (condition omitted, the documented default)",
            json!([{ "Name": "NAMESPACE_ID", "Values": [namespace_a] }]),
            [create_a.clone(), register_a.clone(), pending.clone()]
                .into_iter()
                .collect(),
        ),
        (
            "NAMESPACE_ID EQ (condition spelled out)",
            json!([{ "Name": "NAMESPACE_ID", "Values": [namespace_b], "Condition": "EQ" }]),
            set([&create_b, &register_b]),
        ),
        (
            "SERVICE_ID EQ",
            json!([{ "Name": "SERVICE_ID", "Values": [service_b], "Condition": "EQ" }]),
            [register_b.clone()].into_iter().collect(),
        ),
        (
            "TYPE EQ",
            json!([{ "Name": "TYPE", "Values": ["CREATE_NAMESPACE"], "Condition": "EQ" }]),
            set([&create_a, &create_b]),
        ),
        (
            "TYPE IN, two values",
            json!([{
                "Name": "TYPE",
                "Values": ["CREATE_NAMESPACE", "REGISTER_INSTANCE"],
                "Condition": "IN",
            }]),
            all.clone(),
        ),
        (
            "TYPE IN, a value nothing was filed under",
            json!([{ "Name": "TYPE", "Values": ["DELETE_NAMESPACE"], "Condition": "IN" }]),
            BTreeSet::new(),
        ),
        (
            "STATUS EQ the terminal status",
            json!([{ "Name": "STATUS", "Values": ["SUCCESS"], "Condition": "EQ" }]),
            settled.clone(),
        ),
        (
            "STATUS EQ the non-terminal status",
            json!([{ "Name": "STATUS", "Values": ["SUBMITTED"] }]),
            [pending.clone()].into_iter().collect(),
        ),
        (
            "STATUS IN, one matching value and one unused one",
            json!([{ "Name": "STATUS", "Values": ["SUBMITTED", "FAIL"], "Condition": "IN" }]),
            [pending.clone()].into_iter().collect(),
        ),
        (
            "UPDATE_DATE BETWEEN a range spanning every record",
            json!([{
                "Name": "UPDATE_DATE",
                "Values": ["0", far_future.to_string()],
                "Condition": "BETWEEN",
            }]),
            all.clone(),
        ),
        (
            // A bound before the epoch is well formed, and clamps to the earliest
            // instant a stored record can carry.
            "UPDATE_DATE BETWEEN a range starting before the epoch",
            json!([{
                "Name": "UPDATE_DATE",
                "Values": ["-86400", far_future.to_string()],
                "Condition": "BETWEEN",
            }]),
            all.clone(),
        ),
        (
            "UPDATE_DATE BETWEEN a range before every record",
            json!([{ "Name": "UPDATE_DATE", "Values": ["0", "1"], "Condition": "BETWEEN" }]),
            BTreeSet::new(),
        ),
        (
            "two filters intersect",
            json!([
                { "Name": "TYPE", "Values": ["REGISTER_INSTANCE"] },
                { "Name": "NAMESPACE_ID", "Values": [namespace_b] },
            ]),
            [register_b.clone()].into_iter().collect(),
        ),
        (
            "two filters that cannot both hold",
            json!([
                { "Name": "TYPE", "Values": ["CREATE_NAMESPACE"] },
                { "Name": "SERVICE_ID", "Values": [service_a] },
            ]),
            BTreeSet::new(),
        ),
    ];

    for (case, filters, expected) in cases {
        let response = ok(&router, "ListOperations", json!({ "Filters": filters })).await;
        assert_eq!(
            ids(&response),
            expected,
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }
}

/// A filter combination the documented matrix does not list, a filter value outside the
/// model's enum, a malformed date and an unrecognised `NextToken` are each refused with
/// the modelled `InvalidInput` (HTTP 400) — never served as "matches nothing".
#[tokio::test]
async fn an_unsupported_filter_is_refused_with_invalid_input() {
    let (router, _engine) = emulator_with_engine();
    let (_operation, namespace_id) = namespace(&router, "refusal-ns").await;

    // Every row is a request the documented matrix refuses:
    // "EQ ... is supported for NAMESPACE_ID, SERVICE_ID, STATUS, and TYPE", "IN ... is
    // supported for STATUS and TYPE", "BETWEEN ... is supported for UPDATE_DATE".
    let cases: Vec<(&str, Value)> = vec![
        (
            "IN on a resource id",
            json!({ "Filters": [{
                "Name": "NAMESPACE_ID",
                "Values": [namespace_id, "ns-other"],
                "Condition": "IN",
            }] }),
        ),
        (
            "BEGINS_WITH, a condition no operation filter supports",
            json!({ "Filters": [{
                "Name": "STATUS",
                "Values": ["SUCCESS"],
                "Condition": "BEGINS_WITH",
            }] }),
        ),
        (
            "EQ on the date range",
            json!({ "Filters": [{
                "Name": "UPDATE_DATE",
                "Values": ["0"],
                "Condition": "EQ",
            }] }),
        ),
        (
            "BETWEEN with one bound",
            json!({ "Filters": [{
                "Name": "UPDATE_DATE",
                "Values": ["0"],
                "Condition": "BETWEEN",
            }] }),
        ),
        (
            "BETWEEN with the end date first",
            json!({ "Filters": [{
                "Name": "UPDATE_DATE",
                "Values": ["4000000000", "0"],
                "Condition": "BETWEEN",
            }] }),
        ),
        (
            "BETWEEN with a date that is not in Unix date/time format",
            json!({ "Filters": [{
                "Name": "UPDATE_DATE",
                "Values": ["2026-10-22T01:35:20Z", "4000000000"],
                "Condition": "BETWEEN",
            }] }),
        ),
        (
            "EQ with two values",
            json!({ "Filters": [{
                "Name": "STATUS",
                "Values": ["SUCCESS", "FAIL"],
                "Condition": "EQ",
            }] }),
        ),
        (
            "no values at all",
            json!({ "Filters": [{ "Name": "TYPE", "Values": [] }] }),
        ),
        (
            // `OperationStatus` is `SUBMITTED`/`PENDING`/`SUCCESS`/`FAIL`; the prose
            // spelling "SUCCEED" is not a value the enum accepts.
            "a STATUS value outside the model's enum",
            json!({ "Filters": [{ "Name": "STATUS", "Values": ["SUCCEED"] }] }),
        ),
        (
            "a TYPE value outside the model's enum",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["CREATE_SERVICE"] }] }),
        ),
        (
            "a NextToken this operation never minted",
            json!({ "NextToken": "not-a-token-from-this-emulator" }),
        ),
        (
            // A token carrying this operation's own marker but numbers that no
            // instant can hold: rebuilding the boundary must refuse it, not abort
            // the request.
            "a NextToken whose seconds and nanoseconds overflow any instant",
            json!({ "NextToken": "lop1:18446744073709551615.4294967295:x" }),
        ),
        (
            "a NextToken whose nanoseconds are not sub-second",
            json!({ "NextToken": "lop1:1700000000.1000000000:x" }),
        ),
        (
            "a NextToken with no instant at all",
            json!({ "NextToken": "lop1::x" }),
        ),
        (
            "a NextToken missing its operation id",
            json!({ "NextToken": "lop1:1700000000.000000000" }),
        ),
    ];

    for (case, body) in cases {
        let (status, code) = failure(&router, "ListOperations", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with code {code}"
        );
        assert_eq!(
            code, "InvalidInput",
            "case {case:?}: expected InvalidInput, got {code}"
        );
    }
}

/// Paging covers every record exactly once, in one stable order, for every page size —
/// swept exhaustively over small record counts and page sizes rather than sampled,
/// because the boundary cases (one record per page, a page size larger than the store)
/// are the ones a token can get wrong.
#[tokio::test]
async fn paging_covers_every_record_exactly_once_for_every_page_size() {
    for record_count in [0_usize, 1, 2, 5, 9] {
        for page_size in [1_usize, 2, 3, 100] {
            let (router, engine) = emulator_with_engine();
            let seeded = seed_operations(&engine, record_count);

            let mut paged: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // One request per record plus the final empty page is the most any correct
            // paginator needs; more means the token is not advancing.
            for _ in 0..record_count + 2 {
                let mut body = json!({ "MaxResults": page_size });
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let response = ok(&router, "ListOperations", body).await;
                let page = summaries(&response);
                assert!(
                    page.len() <= page_size,
                    "case {record_count} records, MaxResults {page_size}: a page may hold at \
                     most MaxResults operations, got {response}"
                );
                paged.extend(page.into_iter().map(|(id, _)| id));
                token = response["NextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }

            assert_eq!(
                token, None,
                "case {record_count} records, MaxResults {page_size}: paging must terminate"
            );
            assert_eq!(
                paged, seeded,
                "case {record_count} records, MaxResults {page_size}: expected every record \
                 once in create order, got {paged:?}"
            );
        }
    }
}

/// The documented interaction between paging and filtering: "Cloud Map gets MaxResults
/// operations and then filters them based on the specified criteria. It's possible that
/// no operations in the first MaxResults operations matched the specified criteria but
/// that subsequent groups of MaxResults operations do contain operations that match the
/// criteria." So an early page may answer with nothing and still carry a `NextToken`.
#[tokio::test]
async fn a_filtered_page_may_be_empty_and_still_carry_a_next_token() {
    let (router, engine) = emulator_with_engine();
    let seeded = seed_operations(&engine, 4);
    let last = seeded.last().expect("four records were seeded").clone();

    // Only the newest record targets this namespace, and it is on the LAST page.
    let filters = json!([{ "Name": "NAMESPACE_ID", "Values": [target_of(3)] }]);

    let mut token: Option<String> = None;
    let mut pages: Vec<(usize, bool)> = Vec::new();
    let mut matched: Vec<String> = Vec::new();
    for _ in 0..seeded.len() + 2 {
        let mut body = json!({ "MaxResults": 1, "Filters": filters.clone() });
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let response = ok(&router, "ListOperations", body).await;
        let page = summaries(&response);
        token = response["NextToken"].as_str().map(str::to_string);
        pages.push((page.len(), token.is_some()));
        matched.extend(page.into_iter().map(|(id, _)| id));
        if token.is_none() {
            break;
        }
    }

    assert_eq!(
        matched,
        vec![last.clone()],
        "case filter-on-the-last-page: expected only {last}, got {matched:?}"
    );
    assert_eq!(
        pages,
        vec![(0, true), (0, true), (0, true), (1, false)],
        "case filter-on-the-last-page: the first three pages must be empty yet carry a \
         NextToken, got {pages:?}"
    );
}

/// The namespace id the `index`th seeded record targets, as [`seed_operations`] sets it.
fn target_of(index: usize) -> String {
    format!("ns-seeded-{index}")
}

/// File one operation record whose success is not owed for another minute, so its
/// non-terminal `SUBMITTED` status is stable for the length of a test rather than
/// racing the real clock, and return its `OperationId`.
fn seed_pending(
    engine: &Arc<engine::InMemoryEngine>,
    operation_type: &str,
    targets: [(String, String); 3],
) -> String {
    let state = engine
        .state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let mut state = state.lock();
    state.record_operation(
        operation_type,
        SystemTime::now() + Duration::from_secs(60),
        targets,
    )
}

/// File `count` operation records straight into the store, one millisecond apart and
/// far enough in the past that every one has reached its terminal status, and return
/// their `OperationId`s in the order `ListOperations` must report them.
fn seed_operations(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<String> {
    let state = engine
        .state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let mut state = state.lock();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    (0..count)
        .map(|index| {
            state.record_operation(
                "CREATE_NAMESPACE",
                base + Duration::from_millis(index as u64),
                [("NAMESPACE".to_string(), target_of(index))],
            )
        })
        .collect()
}
