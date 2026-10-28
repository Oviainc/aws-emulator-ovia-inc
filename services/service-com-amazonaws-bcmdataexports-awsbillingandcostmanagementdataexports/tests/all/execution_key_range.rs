// SPDX-License-Identifier: CC0-1.0

//! The executions of ONE export, as the LISTING path and the DELETE path each reach them,
//! over the awsJson1_1 wire: `ListExecutions` / `GetExecution` / `DeleteExport`.
//!
//! The rule under test: a recorded execution belongs to the export whose ARN its key
//! names, by equality of that whole ARN and not by any looser match on the key space, so
//! every path over "the executions of this export" answers the SAME set — `ListExecutions`
//! reports exactly those records, `GetExecution` addresses one of them and no other
//! export's, and `DeleteExport` takes exactly those with the export while every other
//! export's records stay where they were.
//!
//! It is asserted over a TABLE that differs along what the rule generalises over: exports
//! holding none, one and several executions; execution ids of several forms (a UUID as AWS
//! mints one, a lower-case label, an upper-case label, one carrying a space, the longest
//! the modelled `GenericString` admits — `length 0..1024`, smithy_query shape
//! `GenericString` — and the empty id that same shape admits); and export NAMES that are
//! prefixes of one another, so two exports' records sit next to each other in the key
//! space and a path that matched a prefix instead of the whole ARN would report or delete
//! its neighbour's.
//!
//! The executions below are the ones this suite WROTE into the store the handlers read,
//! through the harness the other execution suites use, with the delivery the service owes
//! each export held out of the way (see `delivery_executions.rs`) so every assertion is
//! about a record this test put there. What each path answers is read back BOTH over the
//! wire and by a scan of the WHOLE key space — never through the store's own range walk —
//! so the expectation does not simply agree with the walk it is checking.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::EXECUTION_STATUS_CODE_VALUES;
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState as State;

use crate::delivery_executions::hold_delivery;
use crate::execution_lookup::{client_and_engine, record_execution};
use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// One row of the table: the export name to create, and the execution ids to record for
/// it.
///
/// The names are prefixes of one another (`exp`, `exp_z`, `exp_z_too`, `exp_none`) so the
/// keys of the four exports are adjacent in the `(export ARN, execution id)` key space,
/// and the counts run none / one / several so the rule is asserted on an export with
/// nothing to report as well as on one with a history.
fn rows() -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "exp",
            vec![
                "11111111-2222-3333-4444-555555555555".to_string(),
                "exec-a".to_string(),
                String::new(),
            ],
        ),
        (
            "exp_z",
            vec![
                "EXEC".to_string(),
                "exec with a space".to_string(),
                "e".repeat(1024),
            ],
        ),
        ("exp_z_too", vec!["only-execution".to_string()]),
        ("exp_none", Vec::new()),
    ]
}

/// Create every row's export and record its executions, answering `(ARN, ids)` per row in
/// row order.
async fn seeded(
    client: &Client,
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<(String, Vec<String>)> {
    let definitions = cases();
    let mut seeded = Vec::new();
    for (index, (name, ids)) in rows().into_iter().enumerate() {
        let mut case = definitions[index % definitions.len()].clone();
        case.set_name(name);
        let arn = client.ok("CreateExport", &case.create_request()).await["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: CreateExport returns an ExportArn"))
            .to_string();
        hold_delivery(engine, &arn);
        for (offset, id) in ids.iter().enumerate() {
            record_execution(
                engine,
                &arn,
                id,
                offset % EXECUTION_STATUS_CODE_VALUES.len(),
            );
        }
        seeded.push((arn, ids));
    }
    seeded
}

/// The execution ids the store HOLDS for `export_arn`, read by filtering the whole key
/// space rather than through the store's own range walk — the walk under test.
fn stored_ids(engine: &Arc<engine::InMemoryEngine>, export_arn: &str) -> BTreeSet<String> {
    let state = engine.state::<State>();
    let store = state.lock();
    store
        .executions
        .keys()
        .filter(|(arn, _)| arn == export_arn)
        .map(|(_, id)| id.clone())
        .collect()
}

/// How many executions the store holds in total, whichever export they belong to.
fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<State>();
    let store = state.lock();
    store.executions.len()
}

/// The execution ids `ListExecutions` REPORTS for `export_arn`.
///
/// No `MaxResults` is sent, so the whole set is one page and a `NextToken` beside it would
/// mean the listing cut a page it then could not finish.
async fn listed_ids(case: &str, client: &Client, export_arn: &str) -> BTreeSet<String> {
    let body = client
        .ok("ListExecutions", &json!({ "ExportArn": export_arn }))
        .await;
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: an unlimited page is the last one, got {body}"
    );
    match body.get("Executions") {
        // An empty list may be elided on the wire, which is the empty set it stands for.
        None => BTreeSet::new(),
        Some(Value::Array(references)) => references
            .iter()
            .map(|reference| {
                reference
                    .get("ExecutionId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a listed execution carries ExecutionId, got {reference}")
                    })
                    .to_string()
            })
            .collect(),
        other => panic!("case {case:?}: Executions is a list of references, got {other:?}"),
    }
}

/// Every path over the executions of one export answers the same set: what the store
/// holds for that ARN, what the listing reports for it, and what a lookup finds under it.
#[tokio::test]
async fn every_path_over_one_exports_executions_answers_the_same_set() {
    let (client, engine) = client_and_engine();
    let seeded = seeded(&client, &engine).await;

    for (arn, ids) in &seeded {
        let expected: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(
            stored_ids(&engine, arn),
            expected,
            "case {arn:?}: the store holds exactly the executions recorded for this export"
        );
        assert_eq!(
            listed_ids(arn, &client, arn).await,
            expected,
            "case {arn:?}: the listing reports exactly the executions of this export"
        );

        for id in ids {
            let body = client
                .ok("GetExecution", &json!({ "ExportArn": arn, "ExecutionId": id }))
                .await;
            assert_eq!(
                body["ExecutionId"].as_str(),
                Some(id.as_str()),
                "case {id:?} under {arn}: the lookup answers the execution addressed, got {body}"
            );

            // The same id under every OTHER export: an execution belongs to ONE export,
            // however close the two ARNs sit in the key space.
            for (other, _) in seeded.iter().filter(|(other, _)| other != arn) {
                let (status, body) = client
                    .call(
                        "GetExecution",
                        &json!({ "ExportArn": other, "ExecutionId": id }),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (NOT_FOUND_STATUS, "ResourceNotFoundException"),
                    "case {id:?} under {other}: an execution of {arn} is not an execution of this export, got {status} with {body}"
                );
            }
        }
    }
}

/// Deleting an export takes exactly ITS OWN executions: the deleted ARN holds and reports
/// none afterwards, and every other export still holds and reports every one of its own.
#[tokio::test]
async fn deleting_an_export_takes_exactly_its_own_executions() {
    let (client, engine) = client_and_engine();
    let seeded = seeded(&client, &engine).await;
    let total: usize = seeded.iter().map(|(_, ids)| ids.len()).sum();
    assert_eq!(
        stored_count(&engine),
        total,
        "the store holds every execution this suite recorded before any delete"
    );

    for (position, (doomed, doomed_ids)) in seeded.iter().enumerate() {
        client
            .ok("DeleteExport", &json!({ "ExportArn": doomed }))
            .await;

        assert_eq!(
            stored_ids(&engine, doomed),
            BTreeSet::new(),
            "case {doomed:?}: the delete leaves the store holding none of this export's executions"
        );
        assert_eq!(
            listed_ids(doomed, &client, doomed).await,
            BTreeSet::new(),
            "case {doomed:?}: a deleted export's executions are reported by nobody"
        );
        for id in doomed_ids {
            let (status, body) = client
                .call(
                    "GetExecution",
                    &json!({ "ExportArn": doomed, "ExecutionId": id }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (NOT_FOUND_STATUS, "ResourceNotFoundException"),
                "case {id:?} of {doomed}: an execution of a deleted export is addressable by nobody, got {status} with {body}"
            );
        }

        // Every export not yet deleted keeps every one of its own executions, both in the
        // store and in what the listing reports.
        for (survivor, survivor_ids) in seeded.iter().skip(position + 1) {
            let expected: BTreeSet<String> = survivor_ids.iter().cloned().collect();
            assert_eq!(
                stored_ids(&engine, survivor),
                expected,
                "case {survivor:?}: deleting {doomed} must not touch another export's executions"
            );
            assert_eq!(
                listed_ids(survivor, &client, survivor).await,
                expected,
                "case {survivor:?}: deleting {doomed} must not change what another export lists"
            );
        }
    }

    assert_eq!(
        stored_count(&engine),
        0,
        "every export is deleted, so the store holds no execution of any of them"
    );
}
