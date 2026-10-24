// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListConfigurationCheckOperations`, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: the listing is only believed against what
//! `StartConfigurationChecks` reported and what `GetConfigurationCheckOperation` reads
//! back.
//!
//! RULE under test: the listing REPORTS the configuration check operation records the
//! Region holds for the application it names — `ALL_OPERATIONS` every run of every
//! check, `LATEST_PER_CHECK` (the member's default) the most recently started run of
//! each check — each row identical to what the reader reports for that record, narrowed
//! by every `Filter` the request carries over the members the listing itself reports,
//! and handed out in pages of at most `MaxResults` whose `NextToken` resumes exactly
//! where the previous page stopped; an application this Region has not registered is
//! refused, and the call changes nothing a later caller sees.
//!
//! The rows span what the rule generalises over: both listing modes and the absent
//! member, the model's whole `ConfigurationCheckType` catalogue, every filter name this
//! service filters this listing by and every `FilterOperator` the model declares,
//! single- and multi-valued filter values, page sizes from one record to past the whole
//! set including each published `MaxResults` bound and one past it, and record counts of
//! zero, one round and two rounds.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    CONFIGURATION_CHECK_TYPE_VALUES, FILTER_OPERATOR_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    application_not_found_message, CONFIGURATION_CHECK_FILTER_NAMES, MAX_PAGE_SIZE,
    TRANSITION_SPAN,
};

use crate::application_lifecycle::{Emulator, Reply};
use crate::configuration_checks::{read_record, settle, started, string_field};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The application the rows are started against.
const APPLICATION: &str = "HDB-LIST-CHECKS";

/// The modelled error code a listing for an application this Region has not registered
/// is answered with (`smithy_query(query="operation",
/// operation="ListConfigurationCheckOperations")`: `ResourceNotFoundException`
/// @httpError 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// `ConfigurationCheckOperationListingMode$ALL_OPERATIONS`, as the model spells it.
const ALL_OPERATIONS: &str = "ALL_OPERATIONS";

/// `ConfigurationCheckOperationListingMode$LATEST_PER_CHECK`, as the model spells it —
/// and the member's declared default.
const LATEST_PER_CHECK: &str = "LATEST_PER_CHECK";

/// `OperationStatus$ERROR`, the terminal status a settled check carries.
const ERROR: &str = "ERROR";

/// `OperationStatus$INPROGRESS`, as the model spells it.
const INPROGRESS: &str = "INPROGRESS";

impl Emulator {
    async fn list_checks(&self, body: Value) -> Reply {
        self.send("/list-configuration-check-operations", body).await
    }
}

/// The `ConfigurationCheckOperations` of a listing reply.
fn listed(reply: &Reply) -> Vec<Value> {
    reply
        .member("ConfigurationCheckOperations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id`s of some listed rows, as a set: the listing is ordered by `Id`, so a set is
/// what a row's expectation is stated over.
fn id_set(rows: &[Value]) -> BTreeSet<String> {
    rows.iter().map(|row| string_field(row, "Id")).collect()
}

/// Start every check in the model's catalogue against `application` and return the
/// records the start reported.
async fn start_whole_catalogue(emulator: &Emulator, application: &str) -> Vec<Value> {
    let reply = emulator
        .start_checks(json!({"ApplicationId": application}))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from StartConfigurationChecks, got {reply:?}"
    );
    let rows = started(&reply);
    assert_eq!(
        rows.len(),
        CONFIGURATION_CHECK_TYPE_VALUES.len(),
        "expected one started operation per modelled check, got {rows:?}"
    );
    rows
}

/// Follow the listing's pages from the first to the last, returning the rows in the
/// order the pages reported them. Asserts the paging contract as it walks: no page holds
/// more than `max_results` rows, and only the last page comes without a `NextToken`.
async fn walk_pages(emulator: &Emulator, mut body: Value, max_results: i32) -> Vec<Value> {
    let case = format!("MaxResults {max_results}");
    let mut rows: Vec<Value> = Vec::new();
    let mut pages = 0;
    loop {
        let reply = emulator.list_checks(body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {pages}: expected 200, got {reply:?}"
        );
        let page = listed(&reply);
        assert!(
            page.len() <= max_results as usize,
            "case {case:?} page {pages}: a page holds at most {max_results} rows, got {}",
            page.len()
        );
        rows.extend(page);
        pages += 1;
        // A listing that kept handing out tokens would never terminate; the walk is
        // bounded by one page per record plus the final tokenless page.
        assert!(
            pages <= rows.len() + 1,
            "case {case:?}: the paging must terminate, got {pages} pages for {} rows",
            rows.len()
        );
        match reply.member("NextToken").and_then(Value::as_str) {
            Some(token) => {
                let object = body.as_object_mut().expect("the body is a JSON object");
                object.insert("NextToken".to_string(), json!(token));
            }
            None => return rows,
        }
    }
}

/// RULE: `ALL_OPERATIONS` reports every run the application has had, `LATEST_PER_CHECK`
/// — named or left to the member's default — reports the most recently started run of
/// each check, and every row is what the reader reports for that record.
#[tokio::test]
async fn the_listing_modes_report_every_run_and_the_latest_run_per_check() {
    let emulator = Emulator::new();
    emulator.with_application(APPLICATION).await;

    // An application that has had nothing started reports an empty listing, not a
    // refusal: zero records is one of the counts the rule spans.
    for mode in [ALL_OPERATIONS, LATEST_PER_CHECK] {
        let empty = emulator
            .list_checks(json!({"ApplicationId": APPLICATION, "ListMode": mode}))
            .await;
        assert_eq!(
            empty.status, 200,
            "case {mode:?}: expected 200 before anything was started, got {empty:?}"
        );
        assert!(
            listed(&empty).is_empty() && empty.member("NextToken").is_none(),
            "case {mode:?}: expected an empty listing with no token, got {empty:?}"
        );
    }

    // Two rounds of the whole catalogue: a check can only be re-started once the
    // previous run has ended, so the clock is waited out between them. That makes the
    // second round's records the latest run of every check.
    let first_round = start_whole_catalogue(&emulator, APPLICATION).await;
    settle().await;
    let second_round = start_whole_catalogue(&emulator, APPLICATION).await;
    settle().await;

    let first_ids = id_set(&first_round);
    let second_ids = id_set(&second_round);
    let every_id: BTreeSet<String> = first_ids.union(&second_ids).cloned().collect();
    assert_eq!(
        every_id.len(),
        first_round.len() + second_round.len(),
        "every run gets its own id, got {every_id:?}"
    );

    let all = emulator
        .list_checks(json!({"ApplicationId": APPLICATION, "ListMode": ALL_OPERATIONS}))
        .await;
    assert_eq!(all.status, 200, "expected 200, got {all:?}");
    let all_rows = listed(&all);
    assert_eq!(
        id_set(&all_rows),
        every_id,
        "case {ALL_OPERATIONS:?}: expected every run of every check, got {all_rows:?}"
    );

    // ROUND TRIP: each row is the record the reader reports, member for member. Both
    // runs have settled, so neither the listing nor the read can be mid-transition.
    for row in &all_rows {
        let id = string_field(row, "Id");
        let fetched = emulator.get_check(&id).await;
        assert_eq!(
            fetched.status, 200,
            "case {id:?}: expected 200 from GetConfigurationCheckOperation, got {fetched:?}"
        );
        assert_eq!(
            &read_record(&fetched),
            row,
            "case {id:?}: a listed row must be the record the reader reports, got {row:?}"
        );
    }

    // The latest run per check: named explicitly, and left to the member's default.
    for body in [
        json!({"ApplicationId": APPLICATION, "ListMode": LATEST_PER_CHECK}),
        json!({"ApplicationId": APPLICATION}),
    ] {
        let reply = emulator.list_checks(body.clone()).await;
        assert_eq!(reply.status, 200, "case {body:?}: expected 200, got {reply:?}");
        let rows = listed(&reply);
        assert_eq!(
            id_set(&rows),
            second_ids,
            "case {body:?}: expected the latest run of each check, got {rows:?}"
        );
        let checks: BTreeSet<String> = rows
            .iter()
            .map(|row| string_field(row, "ConfigurationCheckId"))
            .collect();
        assert_eq!(
            checks,
            CONFIGURATION_CHECK_TYPE_VALUES
                .iter()
                .map(|check| (*check).to_string())
                .collect::<BTreeSet<String>>(),
            "case {body:?}: expected one row per modelled check, got {rows:?}"
        );
    }
}

/// One filter row: the filter a request carries, over records whose attributes the test
/// already knows.
struct FilterCase {
    name: &'static str,
    operator: &'static str,
    value: String,
}

/// RULE: a filter narrows the listing to the records whose value for the member it names
/// satisfies it — `Equals` exactly, the ordering operators lexicographically, a
/// comma-separated value when ANY of its values does — and several filters narrow
/// together.
#[tokio::test]
async fn filters_narrow_the_listing_by_the_members_it_reports() {
    let emulator = Emulator::new();
    emulator.with_application(APPLICATION).await;
    let records = start_whole_catalogue(&emulator, APPLICATION).await;
    // Settled, so every record carries the terminal status: what a `Status` row expects
    // is then a fact about the store rather than a race with the clock.
    settle().await;

    // The attributes each record reports for every filterable member, read off the
    // reader rather than assumed.
    let mut attributes: BTreeMap<String, BTreeMap<&str, String>> = BTreeMap::new();
    for record in &records {
        let id = string_field(record, "Id");
        let read = read_record(&emulator.get_check(&id).await);
        let mut values: BTreeMap<&str, String> = BTreeMap::new();
        for name in CONFIGURATION_CHECK_FILTER_NAMES {
            values.insert(name, string_field(&read, name));
        }
        attributes.insert(id, values);
    }

    let catalogue: Vec<String> = CONFIGURATION_CHECK_TYPE_VALUES
        .iter()
        .map(|check| (*check).to_string())
        .collect();
    let first_check = catalogue.first().cloned().expect("the catalogue is not empty");
    let some_id = attributes
        .keys()
        .next()
        .cloned()
        .expect("the catalogue started at least one record");

    let mut cases = vec![
        // One check, exactly.
        FilterCase {
            name: "ConfigurationCheckId",
            operator: "Equals",
            value: first_check.clone(),
        },
        // Several values in one filter are joined with an OR, as `Filter$Value`'s
        // documentation states.
        FilterCase {
            name: "ConfigurationCheckId",
            operator: "Equals",
            value: catalogue.join(","),
        },
        // A value no record carries matches none.
        FilterCase {
            name: "ConfigurationCheckId",
            operator: "Equals",
            value: "SAP_CHECK_99".to_string(),
        },
        // The terminal status every settled record carries, and the one none does.
        FilterCase {
            name: "Status",
            operator: "Equals",
            value: ERROR.to_string(),
        },
        FilterCase {
            name: "Status",
            operator: "Equals",
            value: INPROGRESS.to_string(),
        },
        FilterCase {
            name: "Status",
            operator: "Equals",
            value: format!("{INPROGRESS},{ERROR}"),
        },
        // The listing is already scoped to one application, so this filter keeps every
        // row or none.
        FilterCase {
            name: "ApplicationId",
            operator: "Equals",
            value: APPLICATION.to_string(),
        },
        FilterCase {
            name: "ApplicationId",
            operator: "Equals",
            value: "not-this-application".to_string(),
        },
        // One record by its own id.
        FilterCase {
            name: "Id",
            operator: "Equals",
            value: some_id.clone(),
        },
        // The ordering operators compare lexicographically.
        FilterCase {
            name: "ConfigurationCheckId",
            operator: "GreaterThanOrEquals",
            value: first_check.clone(),
        },
        FilterCase {
            name: "ConfigurationCheckId",
            operator: "LessThanOrEquals",
            value: first_check.clone(),
        },
        FilterCase {
            name: "Id",
            operator: "GreaterThanOrEquals",
            value: some_id.clone(),
        },
        FilterCase {
            name: "Id",
            operator: "LessThanOrEquals",
            value: some_id.clone(),
        },
    ];
    // Every operator the model declares is exercised, and every member this service
    // filters this listing by: the rows are read against the catalogues, so one added to
    // either fails here rather than going untested.
    for operator in FILTER_OPERATOR_VALUES {
        assert!(
            cases.iter().any(|case| case.operator == *operator),
            "the rows must exercise the modelled operator {operator:?}"
        );
    }
    for name in CONFIGURATION_CHECK_FILTER_NAMES {
        assert!(
            cases.iter().any(|case| case.name == *name),
            "the rows must exercise the filterable member {name:?}"
        );
    }

    // A second filter on top of the first, to pin that filters narrow TOGETHER rather
    // than either one of them deciding alone.
    let combined: Vec<(FilterCase, FilterCase)> = vec![
        (
            FilterCase {
                name: "Status",
                operator: "Equals",
                value: ERROR.to_string(),
            },
            FilterCase {
                name: "ConfigurationCheckId",
                operator: "Equals",
                value: first_check.clone(),
            },
        ),
        (
            FilterCase {
                name: "Status",
                operator: "Equals",
                value: INPROGRESS.to_string(),
            },
            FilterCase {
                name: "ConfigurationCheckId",
                operator: "Equals",
                value: catalogue.join(","),
            },
        ),
    ];

    while let Some(case) = cases.pop() {
        assert_filter_selects(&emulator, &attributes, &[case]).await;
    }
    for (left, right) in combined {
        assert_filter_selects(&emulator, &attributes, &[left, right]).await;
    }
}

/// Send one filtered listing and assert it reports EXACTLY the records whose attributes
/// satisfy every filter — the rule, restated over the attributes the reader reported.
async fn assert_filter_selects(
    emulator: &Emulator,
    attributes: &BTreeMap<String, BTreeMap<&str, String>>,
    filters: &[FilterCase],
) {
    let case: Vec<String> = filters
        .iter()
        .map(|filter| format!("{} {} {}", filter.name, filter.operator, filter.value))
        .collect();
    let case = case.join(" AND ");

    let expected: BTreeSet<String> = attributes
        .iter()
        .filter(|(_, values)| {
            filters.iter().all(|filter| {
                let actual = values
                    .get(filter.name)
                    .map(String::as_str)
                    .unwrap_or_default();
                filter.value.split(',').any(|wanted| match filter.operator {
                    "Equals" => actual == wanted,
                    "GreaterThanOrEquals" => actual >= wanted,
                    "LessThanOrEquals" => actual <= wanted,
                    other => panic!("case {case:?}: unmodelled operator {other:?}"),
                })
            })
        })
        .map(|(id, _)| id.clone())
        .collect();

    let body = json!({
        "ApplicationId": APPLICATION,
        "ListMode": ALL_OPERATIONS,
        "Filters": filters
            .iter()
            .map(|filter| json!({
                "Name": filter.name,
                "Value": filter.value,
                "Operator": filter.operator,
            }))
            .collect::<Vec<Value>>(),
    });
    let reply = emulator.list_checks(body).await;
    assert_eq!(reply.status, 200, "case {case:?}: expected 200, got {reply:?}");
    let rows = listed(&reply);
    assert_eq!(
        id_set(&rows),
        expected,
        "case {case:?}: expected exactly the records the filter selects, got {rows:?}"
    );
}

/// RULE: a filter this listing does not filter by is refused, and the refusal names the
/// member it could not use.
#[tokio::test]
async fn a_filter_the_listing_does_not_filter_by_is_refused() {
    let emulator = Emulator::new();
    emulator.with_application(APPLICATION).await;
    start_whole_catalogue(&emulator, APPLICATION).await;

    // Rows: a member of another listing's shape, a member of this shape that is not
    // string-valued, the right member in the wrong case (filter names are
    // case-sensitive), and something no shape carries.
    for name in ["DiscoveryStatus", "StartTime", "status", "NotAMember"] {
        let reply = emulator
            .list_checks(json!({
                "ApplicationId": APPLICATION,
                "Filters": [{"Name": name, "Value": ERROR, "Operator": "Equals"}],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {name:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert!(
            message(&reply).contains(name),
            "case {name:?}: the refusal must name the filter it could not use, got {:?}",
            message(&reply)
        );
    }

    // An operator outside the model's own enum is refused at the boundary.
    for operator in ["Contains", "equals", ""] {
        let reply = emulator
            .list_checks(json!({
                "ApplicationId": APPLICATION,
                "Filters": [{"Name": "Status", "Value": ERROR, "Operator": operator}],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {operator:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {operator:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    // So is a listing mode outside it.
    for mode in ["EVERY_OPERATION", "latest_per_check", ""] {
        let reply = emulator
            .list_checks(json!({"ApplicationId": APPLICATION, "ListMode": mode}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {mode:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {mode:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }
}

/// RULE: a caller that follows the tokens sees every record exactly once, in the
/// listing's `Id` order, whatever page size it asked for.
#[tokio::test]
async fn following_the_tokens_reports_every_record_exactly_once() {
    let emulator = Emulator::new();
    emulator.with_application(APPLICATION).await;
    let first_round = start_whole_catalogue(&emulator, APPLICATION).await;
    settle().await;
    let second_round = start_whole_catalogue(&emulator, APPLICATION).await;
    settle().await;

    let every_id: BTreeSet<String> = id_set(&first_round)
        .union(&id_set(&second_round))
        .cloned()
        .collect();

    // Page sizes spanning one record per page, a partial page, exactly the whole set,
    // past it, and each published `MaxResults` bound.
    let sizes: Vec<i32> = vec![
        1,
        2,
        every_id.len() as i32,
        every_id.len() as i32 + 1,
        MAX_PAGE_SIZE,
    ];
    for max_results in sizes {
        let rows = walk_pages(
            &emulator,
            json!({
                "ApplicationId": APPLICATION,
                "ListMode": ALL_OPERATIONS,
                "MaxResults": max_results,
            }),
            max_results,
        )
        .await;
        let ids: Vec<String> = rows.iter().map(|row| string_field(row, "Id")).collect();
        assert_eq!(
            ids.iter().cloned().collect::<BTreeSet<String>>(),
            every_id,
            "case {max_results}: following the tokens must report every record, got {ids:?}"
        );
        assert_eq!(
            ids.len(),
            every_id.len(),
            "case {max_results}: no record may be reported twice, got {ids:?}"
        );
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "case {max_results}: the pages must come in the listing's Id order, got {ids:?}"
        );
    }

    // A page size outside the model's declared range is refused at the boundary, on
    // either side of it.
    for max_results in [0, MAX_PAGE_SIZE + 1] {
        let reply = emulator
            .list_checks(json!({"ApplicationId": APPLICATION, "MaxResults": max_results}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {max_results}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {max_results}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    // A token this service did not issue is refused rather than resumed from. Every row
    // satisfies the model's `NextToken` pattern (16..2048 characters), so the refusal is
    // the handler's and not the boundary validator's.
    for token in [
        "0000000000000000",
        "not-a-token-this-service-issued",
        "ffffffffffffffffffffffffffffffff",
    ] {
        let reply = emulator
            .list_checks(json!({"ApplicationId": APPLICATION, "NextToken": token}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {token:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert!(
            message(&reply).contains(token),
            "case {token:?}: the refusal must name the token it could not resume from, got {:?}",
            message(&reply)
        );
    }
}

/// RULE: the listing is scoped to the application it names in the Region it was signed
/// for — an application that is not registered there is refused, and nothing of another
/// application or another Region is reported.
#[tokio::test]
async fn the_listing_is_scoped_to_one_application_in_one_region() {
    let emulator = Emulator::new();
    emulator.with_application(APPLICATION).await;
    emulator.with_application("other-app").await;
    let mine = id_set(&start_whole_catalogue(&emulator, APPLICATION).await);
    let theirs = id_set(&start_whole_catalogue(&emulator, "other-app").await);

    let reply = emulator
        .list_checks(json!({"ApplicationId": APPLICATION, "ListMode": ALL_OPERATIONS}))
        .await;
    assert_eq!(reply.status, 200, "expected 200, got {reply:?}");
    let listed_ids = id_set(&listed(&reply));
    assert_eq!(
        listed_ids, mine,
        "the listing reports the named application's records alone, got {listed_ids:?}"
    );
    assert!(
        listed_ids.is_disjoint(&theirs),
        "no record of another application may be listed, got {listed_ids:?}"
    );

    // Rows: an id nothing registered, the registered id in another case (ids are
    // compared exactly), and an id in the recorded probe's form.
    for id in [
        "NO-SUCH",
        "hdb-list-checks",
        "sim2real-probe-ssm-sap-app-8d37e33f",
    ] {
        let reply = emulator.list_checks(json!({"ApplicationId": id})).await;
        assert_eq!(
            reply.status, 404,
            "case {id:?}: expected 404, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            RESOURCE_NOT_FOUND,
            "case {id:?}: expected {RESOURCE_NOT_FOUND}, got {reply:?}"
        );
        let expected = application_not_found_message(id);
        let actual = message(&reply);
        assert_eq!(
            actual, expected,
            "case {id:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // The service is regional: the application of one endpoint is not reachable at
    // another, so the listing there is a refusal rather than an empty page.
    let elsewhere = "eu-west-1";
    let crossed = emulator
        .send_in_region(
            elsewhere,
            "/list-configuration-check-operations",
            json!({"ApplicationId": APPLICATION}),
        )
        .await;
    assert_eq!(
        crossed.status, 404,
        "case {elsewhere:?}: an application of another Region is not reachable here, got {crossed:?}"
    );
}

/// RULE: the listing REPORTS — it leaves every record exactly as the next caller would
/// have found it, so repeated listings of a running check agree and the run it observed
/// is still running (a second start of that check is still refused as a conflict).
#[tokio::test]
async fn listing_running_checks_does_not_change_them() {
    let emulator = Emulator::new();
    emulator.with_application(APPLICATION).await;
    start_whole_catalogue(&emulator, APPLICATION).await;
    let started_at = Instant::now();

    let mut listings = Vec::new();
    for attempt in 0..3 {
        let reply = emulator
            .list_checks(json!({"ApplicationId": APPLICATION, "ListMode": ALL_OPERATIONS}))
            .await;
        assert_eq!(
            reply.status, 200,
            "listing {attempt}: expected 200, got {reply:?}"
        );
        listings.push(listed(&reply));
    }

    // A check already running cannot be started again, so this reports whether the
    // listings left those runs in place.
    let again = emulator
        .start_checks(json!({"ApplicationId": APPLICATION}))
        .await;

    // The transient assertions hold only while the clock still owes the transition: past
    // that instant every record was due to end whether or not anybody listed.
    if started_at.elapsed() >= TRANSITION_SPAN {
        return;
    }
    for (attempt, rows) in listings.iter().enumerate().skip(1) {
        assert_eq!(
            rows, &listings[0],
            "listing {attempt}: every listing of a running set must report the same rows, got \
             {rows:?}"
        );
    }
    for row in &listings[0] {
        assert_eq!(
            string_field(row, "Status"),
            INPROGRESS,
            "a listing must not end a running check, got {row:?}"
        );
    }
    assert_eq!(
        again.status, 409,
        "the checks the listings observed must still be running, got {again:?}"
    );
}
