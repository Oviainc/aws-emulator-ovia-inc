// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListExecutions` over the awsJson1_1 wire: `POST /` with
//! `X-Amz-Target: AWSBillingAndCostManagementDataExports.ListExecutions`.
//!
//! The rule under test: `ListExecutions` reports one `ExecutionReference` per execution the
//! store holds FOR THE EXPORT the `ExportArn` names — every one, exactly once, in one
//! stable execution-id order — one page at a time, where a page holds at most `MaxResults`
//! references and the token it carries resumes immediately after it; an `ExportArn` naming
//! no export is the modelled `ResourceNotFoundException` (HTTP 404), a `NextToken` this
//! listing of this export never issued is the modelled `ValidationException` (HTTP 400),
//! and listing is a read that changes nothing a later caller sees.
//!
//! It is asserted over a TABLE that differs along what the rule generalises over: exports
//! holding none, one and several executions, execution ids of several forms (including the
//! empty id the modelled `GenericString` `length 0..1024` admits), statuses walked from the
//! model's own published `ExecutionStatusCode` list, and every page size from one up to one
//! past the number of executions plus both published `MaxResults` bounds. The recorded
//! case — a `MaxResults` outside the modelled range — is one row of
//! [`max_results_outside_the_modelled_range_is_refused`].
//!
//! Nothing in this emulator CREATES an execution (see `execution_lookup.rs` and
//! `crate::state::StoredExecution`), so the executions below are written into the store the
//! handler reads, exactly as a delivery would record them — which is what makes every
//! assertion below about a value this test put in the store rather than one the handler
//! invented. The router, the signing, the error-code helper, the store seeding and the
//! minimal export request all come from the other suites of this same test target.

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::EXECUTION_STATUS_CODE_VALUES;
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::EXPORT_NOT_FOUND_MESSAGE;

use crate::execution_lookup::{client_and_engine, record_execution};
use crate::export_lifecycle::{error_code, Client};
use crate::table_listing::minimal_export_request;

/// `MaxResults`: `range 1..300` (smithy_query operation `ListExecutions`, member
/// `MaxResults`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 300;

/// A ceiling on the pages one walk may take, so a listing that handed back a resume token
/// without making progress fails the test instead of looping forever.
const MAX_PAGES: usize = 64;

/// One row of the table: an export, and the execution ids recorded for it.
///
/// The ids differ in form — a UUID as AWS mints one, a short label, an upper-case label,
/// one carrying a space, a long one, and the EMPTY id the modelled `GenericString`
/// (`length 0..1024`, `pattern ^[\S\s]*$` — smithy_query shape `GenericString`) admits —
/// because the rule is about the executions of an export and their order, not about any one
/// id form.
fn rows() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("no_executions_yet", Vec::new()),
        (
            "one_execution",
            vec!["11111111-2222-3333-4444-555555555555".to_string()],
        ),
        (
            "two_executions",
            vec!["exec-b".to_string(), "exec-a".to_string()],
        ),
        (
            "several_executions",
            vec![
                "d3f0f2a0-0000-4000-8000-000000000001".to_string(),
                "exec".to_string(),
                "EXEC".to_string(),
                "exec with a space".to_string(),
                "e".repeat(1024),
                String::new(),
                "exec-0010".to_string(),
            ],
        ),
    ]
}

/// Call `ListExecutions` with the paging members a case sets, requiring HTTP 200.
async fn list_executions(
    client: &Client,
    export_arn: &str,
    max_results: Option<i64>,
    token: Option<&str>,
) -> Value {
    let mut request = Map::new();
    request.insert("ExportArn".to_string(), json!(export_arn));
    if let Some(max) = max_results {
        request.insert("MaxResults".to_string(), json!(max));
    }
    if let Some(token) = token {
        request.insert("NextToken".to_string(), json!(token));
    }
    client
        .ok("ListExecutions", &Value::Object(request))
        .await
}

/// The `Executions` of one response. An empty page may be elided on the wire, so an absent
/// member is read as the empty list it stands for; a page that reports references must
/// carry it.
fn executions_of(case: &str, response: &Value) -> Vec<Value> {
    match response.get("Executions") {
        Some(Value::Array(references)) => references.clone(),
        None => Vec::new(),
        other => panic!("case {case:?}: Executions must be a list of references, got {other:?}"),
    }
}

/// A reported reference's `ExecutionId`.
fn id_of(case: &str, reference: &Value) -> String {
    reference
        .get("ExecutionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listed execution must carry ExecutionId, got {reference}")
        })
        .to_string()
}

/// Walk every execution of `export_arn` with `max_results`, returning the references in the
/// order they were reported and how many pages it took.
async fn walk(client: &Client, export_arn: &str, max_results: Option<i64>) -> (Vec<Value>, usize) {
    let case = format!("{export_arn} MaxResults={max_results:?}");
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        assert!(
            pages < MAX_PAGES,
            "case {case:?}: the listing did not terminate within {MAX_PAGES} pages"
        );
        let response = list_executions(client, export_arn, max_results, token.as_deref()).await;
        let page = executions_of(&case, &response);
        pages += 1;
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "case {case:?}: a page carried {} references, more than MaxResults",
                page.len()
            );
        }
        seen.extend(page);
        match response.get("NextToken").and_then(Value::as_str) {
            Some(next) => {
                assert!(
                    !next.is_empty(),
                    "case {case:?}: a resume token must not be empty, got {response}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    (seen, pages)
}

/// Create each row's export and record its executions, returning
/// `(export ARN, ids in the order the listing must report them)` per row.
async fn seeded() -> (Client, Vec<(String, Vec<String>)>) {
    let (client, engine) = client_and_engine();
    let mut seeded: Vec<(String, Vec<String>)> = Vec::new();
    for (index, (name, ids)) in rows().into_iter().enumerate() {
        let created = client
            .ok("CreateExport", &minimal_export_request(name))
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: CreateExport returns an ExportArn"))
            .to_string();
        for (offset, id) in ids.iter().enumerate() {
            // Each execution gets a different published status code, so the reported
            // status is the recorded one rather than a constant.
            let status = (index + offset) % EXECUTION_STATUS_CODE_VALUES.len();
            record_execution(&engine, &arn, id, status);
        }
        // The store is keyed by `(export ARN, execution id)`, so the listing's order is
        // the ids' own order.
        let mut expected = ids.clone();
        expected.sort_unstable();
        seeded.push((arn, expected));
    }
    (client, seeded)
}

/// Every page size walks every execution of its export exactly once, in one stable order,
/// and only the final page omits the resume token.
#[tokio::test]
async fn list_executions_pages_over_every_recorded_execution_of_the_export() {
    let (client, seeded) = seeded().await;

    for (arn, expected) in &seeded {
        let total = expected.len() as i64;
        // The rows: the unpaged call, both published MaxResults bounds, and every page size
        // from one execution up to one past the number recorded.
        let mut sizes: Vec<Option<i64>> = vec![None, Some(MAX_RESULTS_MIN), Some(MAX_RESULTS_MAX)];
        sizes.extend((1..=total + 1).map(Some));
        let (unpaged, pages) = walk(&client, arn, None).await;
        assert_eq!(
            pages, 1,
            "case {arn:?}: an unpaged listing must answer in one page, took {pages}"
        );
        for max_results in sizes {
            let case = format!("{arn} MaxResults={max_results:?}");
            let (seen, pages) = walk(&client, arn, max_results).await;
            let ids: Vec<String> = seen.iter().map(|value| id_of(&case, value)).collect();
            assert_eq!(
                &ids, expected,
                "case {case:?}: paging must report every execution of this export exactly once, in id order"
            );
            let expected_pages = match max_results {
                Some(max) => ((total + max - 1) / max).max(1) as usize,
                None => 1,
            };
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} page(s) over {total} execution(s), took {pages}"
            );
            assert_eq!(
                seen, unpaged,
                "case {case:?}: a paged reference must match the one the unpaged listing reported"
            );
        }
    }
}

/// A listed reference reports the status recorded for that execution, and the listing and
/// `GetExecution` answer out of ONE store: every listed id is one `GetExecution` describes,
/// with the same status.
#[tokio::test]
async fn a_listed_reference_reports_the_recorded_execution() {
    let (client, seeded) = seeded().await;

    for (arn, expected) in &seeded {
        let (listed, _) = walk(&client, arn, None).await;
        for reference in &listed {
            let id = id_of(arn, reference);
            let label = format!("{id:?} of {arn}");
            let described = client
                .ok(
                    "GetExecution",
                    &json!({ "ExportArn": arn, "ExecutionId": id }),
                )
                .await;
            assert_eq!(
                reference.get("ExecutionStatus"),
                described.get("ExecutionStatus"),
                "case {label:?}: the listing and GetExecution must report one status, got {reference} and {described}"
            );
            let status_code = reference
                .get("ExecutionStatus")
                .and_then(|status| status.get("StatusCode"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: a listed execution must carry its status, got {reference}")
                });
            assert!(
                EXECUTION_STATUS_CODE_VALUES.contains(&status_code),
                "case {label:?}: {status_code:?} must be one of the published {EXECUTION_STATUS_CODE_VALUES:?}"
            );
        }
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {arn:?}: the listing must report every recorded execution, got {listed:?}"
        );
    }

    // An execution of one export is not an execution of another: each listing reports only
    // its own export's executions. The rows record disjoint id sets, so an id of one export
    // appearing under another could only have come from the wrong sequence.
    for (arn, expected) in &seeded {
        for (other_arn, _) in seeded.iter().filter(|(other, _)| other != arn) {
            let (listed, _) = walk(&client, other_arn, None).await;
            let ids: Vec<String> = listed.iter().map(|value| id_of(other_arn, value)).collect();
            for id in expected {
                assert!(
                    !ids.contains(id),
                    "case {id:?}: an execution of {arn} must not be listed under {other_arn}, got {ids:?}"
                );
            }
        }
    }
}

/// An `ExportArn` that names no export is the modelled `ResourceNotFoundException`, which
/// reports the export it could not find — never an empty page, which would say the export
/// exists and has never run.
#[tokio::test]
async fn an_export_arn_that_names_no_export_is_a_modelled_not_found() {
    let (client, seeded) = seeded().await;
    let known = seeded[0].0.clone();

    // Rows: an export never created, one in another region, one in another account, and a
    // resource segment that is not an export at all. All satisfy the modelled `Arn`
    // pattern, so each reaches the handler.
    let absent = vec![
        ::engine::emulator_arn("bcm-data-exports", "us-east-1", "export/never_created"),
        ::engine::emulator_arn("bcm-data-exports", "eu-west-2", "export/no_executions_yet"),
        format!(
            "arn:aws:bcm-data-exports:us-east-1:210987654321:export/{}",
            "no_executions_yet"
        ),
        ::engine::emulator_arn("bcm-data-exports", "us-east-1", "table/COST_AND_USAGE_REPORT"),
    ];
    for arn in absent {
        let (status, body) = client
            .call("ListExecutions", &json!({ "ExportArn": arn }))
            .await;
        assert_eq!(
            (
                status,
                error_code(&body).as_str(),
                body["Message"].as_str(),
                body["ResourceId"].as_str(),
                body["ResourceType"].as_str(),
            ),
            (
                404,
                "ResourceNotFoundException",
                Some(EXPORT_NOT_FOUND_MESSAGE),
                Some(arn.as_str()),
                Some("Export"),
            ),
            "case {arn:?}: expected the modelled 404 reporting the absent export, got {status} with {body}"
        );
    }

    // Deleting an export takes its listing with it: the ARN then names nothing.
    client
        .ok("DeleteExport", &json!({ "ExportArn": known }))
        .await;
    let (status, body) = client
        .call("ListExecutions", &json!({ "ExportArn": known }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {known:?}: a deleted export has no execution listing, got {status} with {body}"
    );
}

/// A `MaxResults` outside the modelled range is refused, and each published bound is
/// accepted. The recorded unit this operation's issue carries is the `MaxResults=301` row.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let (client, seeded) = seeded().await;
    let arn = seeded[0].0.clone();

    for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let (status, body) = client
            .call(
                "ListExecutions",
                &json!({ "ExportArn": arn, "MaxResults": accepted }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case MaxResults={accepted}: a value inside range 1..300 must be accepted, got {status} with {body}"
        );
    }
    for refused in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1, -1, i64::from(i32::MAX)] {
        let (status, body) = client
            .call(
                "ListExecutions",
                &json!({ "ExportArn": arn, "MaxResults": refused }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case MaxResults={refused}: a value outside range 1..300 must be refused, got {status} with {body}"
        );
    }
}

/// A `NextToken` this listing of THIS export did not issue is refused with the modelled
/// `ValidationException`: a token names a position in the sequence it was cut from, so a
/// token from another listing, or from another export's executions, is not a position in
/// this one.
#[tokio::test]
async fn a_next_token_is_bound_to_the_executions_of_its_own_export() {
    let (client, seeded) = seeded().await;
    let (paged_arn, paged_ids) = seeded
        .iter()
        .find(|(_, ids)| ids.len() > 1)
        .expect("a table row with more than one execution");
    let (other_arn, _) = seeded
        .iter()
        .find(|(arn, ids)| arn != paged_arn && ids.len() > 1)
        .expect("a second table row with more than one execution");

    let issued = list_executions(&client, paged_arn, Some(1), None).await["NextToken"]
        .as_str()
        .expect("a first page over several executions must carry a NextToken")
        .to_string();
    let other_issued = list_executions(&client, other_arn, Some(1), None).await["NextToken"]
        .as_str()
        .expect("a first page over several executions must carry a NextToken")
        .to_string();

    let mut corrupted = issued.clone();
    corrupted.replace_range(0..1, if issued.starts_with('a') { "b" } else { "a" });
    // Rows: not hex, hex too short to carry the integrity tag, hex of a plausible execution
    // id with no tag, an issued token with its tag corrupted, an issued token with bytes
    // cut off, and a token issued for ANOTHER export's executions.
    for forged in [
        "not-a-token".to_string(),
        hex::encode("x"),
        hex::encode(&paged_ids[1]),
        corrupted,
        issued[..issued.len() - 2].to_string(),
        other_issued.clone(),
    ] {
        let (status, body) = client
            .call(
                "ListExecutions",
                &json!({ "ExportArn": paged_arn, "NextToken": forged }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {forged:?}: an unissued NextToken must be refused, got {status} with {body}"
        );
    }

    // Tokens cut from the OTHER listings of this service name positions in other key
    // spaces, so this listing must refuse them...
    let exports_token = client
        .ok("ListExports", &json!({ "MaxResults": 1 }))
        .await["NextToken"]
        .as_str()
        .expect("a first page over several exports must carry a NextToken")
        .to_string();
    let tables_token = client
        .ok("ListTables", &json!({ "MaxResults": 1 }))
        .await["NextToken"]
        .as_str()
        .expect("a first page over the table catalog must carry a NextToken")
        .to_string();
    for foreign in [exports_token.clone(), tables_token.clone()] {
        let (status, body) = client
            .call(
                "ListExecutions",
                &json!({ "ExportArn": paged_arn, "NextToken": foreign }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {foreign:?}: a token from another listing must be refused, got {status} with {body}"
        );
    }

    // ...and the other way round: an executions token is not a position in the exports or
    // the catalog listing.
    for listing in ["ListExports", "ListTables"] {
        let (status, body) = client
            .call(listing, &json!({ "NextToken": issued.clone() }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {listing:?}: an executions token must be refused, got {status} with {body}"
        );
    }

    // The token this listing DID issue still resumes it, immediately after its page.
    let resumed = list_executions(&client, paged_arn, None, Some(&issued)).await;
    let ids: Vec<String> = executions_of("resumed", &resumed)
        .iter()
        .map(|value| id_of("resumed", value))
        .collect();
    assert_eq!(
        ids,
        paged_ids[1..].to_vec(),
        "case \"issued token\": the token must resume after the page it ended, got {resumed}"
    );
}

/// Listing executions is a read: it changes nothing a later caller sees — not what the
/// listing itself answers, not what `GetExecution` answers, and not the export's own status.
#[tokio::test]
async fn listing_executions_changes_nothing() {
    let (client, seeded) = seeded().await;
    let (arn, ids) = seeded
        .iter()
        .find(|(_, ids)| ids.len() > 1)
        .expect("a table row with more than one execution");

    let before = list_executions(&client, arn, None, None).await;
    let export_before = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    let execution_before = client
        .ok(
            "GetExecution",
            &json!({ "ExportArn": arn, "ExecutionId": ids[0] }),
        )
        .await;

    // A paged walk and a resumed token, so every path through the handler runs.
    let _ = walk(&client, arn, Some(1)).await;

    let after = list_executions(&client, arn, None, None).await;
    assert_eq!(
        before, after,
        "a listing must report the same executions on every call"
    );
    assert_eq!(
        export_before,
        client.ok("GetExport", &json!({ "ExportArn": arn })).await,
        "listing executions must not change the export it listed"
    );
    assert_eq!(
        execution_before,
        client
            .ok(
                "GetExecution",
                &json!({ "ExportArn": arn, "ExecutionId": ids[0] }),
            )
            .await,
        "listing executions must not change what GetExecution reports"
    );
}
