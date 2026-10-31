// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListOperations`, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: the listing is only believed against what the
//! call that FILED each operation reported and what `GetOperation` reads back for it.
//!
//! RULE under test: the listing REPORTS the operation records this account and Region
//! holds for the application it names — one per call that acted on that application —
//! each row identical to what `GetOperation` reports for that record, narrowed by every
//! `Filter` the request carries over the attributes the listing itself reports, and handed
//! out in pages of at most `MaxResults` whose `NextToken` resumes exactly where the
//! previous page stopped; an `ApplicationId` no registration of this account and Region
//! answers is refused with the wording this service gives an unresolvable application
//! reference, and the call changes nothing a later caller sees.
//!
//! The rows span what the rule generalises over: every kind of operation an API of this
//! service files (register, start, refresh, stop and deregister), both sides of the clock
//! (a running record and a settled one), record counts of one and of several, every
//! filter name this service filters this listing by and every `FilterOperator` the model
//! declares, single- and multi-valued filter values and several filters at once, page
//! sizes from one record per page to past the whole set including each published
//! `MaxResults` bound and one past it, and the refusals: an unregistered id, an id whose
//! registration has been deregistered, the same id at another Region, a filter name
//! outside the documented set, and tokens this listing did not issue — including one
//! another listing of this service did.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::FILTER_OPERATOR_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    APPLICATION_RESOURCE_TYPE, MAX_PAGE_SIZE, OPERATION_FILTER_NAMES,
    OPERATION_STATUS_ERROR, OPERATION_STATUS_INPROGRESS, OPERATION_STATUS_SUCCESS,
    OPERATION_TYPE_DEREGISTER_APPLICATION, OPERATION_TYPE_REGISTER_APPLICATION,
    OPERATION_TYPE_START_APPLICATION, OPERATION_TYPE_START_APPLICATION_REFRESH,
    OPERATION_TYPE_STOP_APPLICATION, TRANSITION_SPAN,
};

use crate::application_lifecycle::{rendered, settle, valid_registration, Emulator, Reply};
use crate::configuration_checks::string_field;
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The path `ListOperations` is bound to (`smithy_query(query="operation",
/// operation="ListOperations")`: POST `/list-operations`).
const LIST_OPERATIONS: &str = "/list-operations";

/// The application the rows are filed against.
const APPLICATION: &str = "HDB-OPS";

/// A second application of the same Region, so a row can show that one application's
/// listing reports nothing of another's records.
const OTHER_APPLICATION: &str = "HDB-OPS-TWO";

/// Another Region to drive the same application id at: a registration belongs to the
/// Region the call that made it was signed for, so an application registered at the
/// harness' Region resolves at no other.
const ELSEWHERE: &str = "eu-west-1";

/// A `Filter$Operator` value, as the model's `FilterOperator` enum spells it.
const EQUALS: &str = "Equals";

/// Send one `ListOperations` request.
async fn list_operations(emulator: &Emulator, body: Value) -> Reply {
    emulator.send(LIST_OPERATIONS, body).await
}

/// `GetOperation` for `id` — the reader every expectation about a record is stated
/// against.
async fn read_operation(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .send("/get-operation", json!({"OperationId": id}))
        .await
}

/// The `Operations` of a listing reply.
fn listed(reply: &Reply) -> Vec<Value> {
    reply
        .member("Operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Operation` a `GetOperation` reply reports, `Value::Null` when it reports none.
fn read_record(reply: &Reply) -> Value {
    reply.member("Operation").cloned().unwrap_or(Value::Null)
}

/// The `Id`s of some listed rows, as a set: a row's expectation is about WHICH records
/// the listing reports, which the order is pinned separately from.
fn id_set(rows: &[Value]) -> BTreeSet<String> {
    rows.iter().map(|row| string_field(row, "Id")).collect()
}

/// The `OperationId` a call that files an operation reported.
fn operation_id(reply: &Reply, case: &str) -> String {
    let id = reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: the call must report the operation it filed, got {reply:?}"
    );
    id
}

/// One call of this service that files an operation record against an application that is
/// already registered: the `Operation$Type` the record carries, where the call goes, and
/// the members it carries beyond the `ApplicationId` every one of them names.
struct FilingCase {
    op_type: &'static str,
    path: &'static str,
    extra: fn() -> Value,
}

/// No member beyond `ApplicationId`.
fn no_extra() -> Value {
    json!({})
}

/// A stop asked for with both of the switches `StopApplication` takes, so that row's
/// record carries `Properties` as well.
fn stop_switches() -> Value {
    json!({"StopConnectedEntity": "DBMS", "IncludeEc2InstanceShutdown": true})
}

/// Every kind of operation an API of this service files after the registration that
/// creates the application, in an order each call is servable in: a call made while a
/// change the clock still owes is outstanding is a conflict rather than a record, so
/// [`file_every_kind`] waits the clock out between them.
fn filing_cases() -> Vec<FilingCase> {
    vec![
        FilingCase {
            op_type: OPERATION_TYPE_START_APPLICATION,
            path: "/start-application",
            extra: no_extra,
        },
        FilingCase {
            op_type: OPERATION_TYPE_START_APPLICATION_REFRESH,
            path: "/start-application-refresh",
            extra: no_extra,
        },
        FilingCase {
            op_type: OPERATION_TYPE_STOP_APPLICATION,
            path: "/stop-application",
            extra: stop_switches,
        },
    ]
}

/// The request body one filing call sends against `application`.
fn filing_body(case: &FilingCase, application: &str) -> Value {
    let mut body = (case.extra)();
    body.as_object_mut()
        .expect("the extra members are a JSON object")
        .insert("ApplicationId".to_string(), json!(application));
    body
}

/// Register `application` and then make every call of [`filing_cases`] against it,
/// returning the `OperationId` of each record filed, oldest first. Every record has
/// settled by the time this returns.
async fn file_every_kind(emulator: &Emulator, application: &str) -> Vec<String> {
    let reply = emulator
        .send(
            "/register-application",
            valid_registration(application, "HANA"),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {application:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    let mut filed = vec![operation_id(&reply, OPERATION_TYPE_REGISTER_APPLICATION)];
    settle().await;

    for case in filing_cases() {
        let reply = emulator
            .send(case.path, filing_body(&case, application))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from the call that files the operation, got {reply:?}",
            case.op_type
        );
        filed.push(operation_id(&reply, case.op_type));
        settle().await;
    }
    filed
}

/// Every `Operation$Type` the records of [`file_every_kind`] carry.
fn filed_types() -> BTreeSet<String> {
    std::iter::once(OPERATION_TYPE_REGISTER_APPLICATION.to_string())
        .chain(
            filing_cases()
                .into_iter()
                .map(|case| case.op_type.to_string()),
        )
        .collect()
}

/// RULE: the listing reports the records this Region holds for the application it names —
/// one per call that acted on it — and each row is the record the reader resolves by its
/// own id, member for member.
#[tokio::test]
async fn reports_every_record_filed_against_the_application() {
    let emulator = Emulator::new();
    let filed = file_every_kind(&emulator, APPLICATION).await;

    let reply = list_operations(&emulator, json!({"ApplicationId": APPLICATION})).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListOperations, got {reply:?}"
    );
    let rows = listed(&reply);
    assert_eq!(
        id_set(&rows),
        filed.iter().cloned().collect::<BTreeSet<String>>(),
        "the listing must report every record filed against the application, got {rows:?}"
    );
    assert_eq!(
        reply.member("NextToken"),
        None,
        "a page that holds every record carries no NextToken, got {reply:?}"
    );

    // ROUND TRIP: each row is the record `GetOperation` resolves by its id. Every record
    // has settled, so neither the listing nor the read can be mid-transition.
    for row in &rows {
        let id = string_field(row, "Id");
        let fetched = read_operation(&emulator, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {id:?}: expected 200 from GetOperation, got {fetched:?}"
        );
        assert_eq!(
            &read_record(&fetched),
            row,
            "case {id:?}: a listed row must be the record the reader reports, got {row:?}"
        );
    }

    // Every kind of call that files a record is reported, and each row names the
    // application the call acted on and the terminal status the clock brought it to.
    let types: BTreeSet<String> = rows.iter().map(|row| string_field(row, "Type")).collect();
    assert_eq!(
        types,
        filed_types(),
        "every kind of operation filed must be reported, got {rows:?}"
    );
    for row in &rows {
        let case = string_field(row, "Type");
        assert_eq!(
            string_field(row, "ResourceId"),
            APPLICATION,
            "case {case:?}: a listed record names the application it was filed against, got {row:?}"
        );
        assert_eq!(
            string_field(row, "ResourceType"),
            APPLICATION_RESOURCE_TYPE,
            "case {case:?}: expected the application resource type, got {row:?}"
        );
        assert_eq!(
            string_field(row, "Status"),
            OPERATION_STATUS_SUCCESS,
            "case {case:?}: the clock owed this record its terminal status, got {row:?}"
        );
    }

    // The rows come in the order the listing pages over: its own `Id` order.
    let ids: Vec<String> = rows.iter().map(|row| string_field(row, "Id")).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "the listing must report its records in Id order, got {ids:?}"
    );

    // A count of ONE: an application whose only call was its registration reports that
    // one record and nothing of the application beside it.
    let reply = emulator
        .send(
            "/register-application",
            valid_registration(OTHER_APPLICATION, "HANA"),
        )
        .await;
    let only = operation_id(&reply, OTHER_APPLICATION);
    settle().await;
    let reply = list_operations(&emulator, json!({"ApplicationId": OTHER_APPLICATION})).await;
    assert_eq!(
        id_set(&listed(&reply)),
        BTreeSet::from([only.clone()]),
        "an application with one record reports exactly it, got {reply:?}"
    );
}

/// RULE: a record the clock has not yet brought to its terminal status is reported as the
/// running record it is, and the deregistration that ends an application is a record like
/// every other while that application still resolves.
#[tokio::test]
async fn reports_a_running_record_and_the_deregistration_that_ends_the_application() {
    let emulator = Emulator::new();
    let reply = emulator
        .send(
            "/register-application",
            valid_registration(APPLICATION, "HANA"),
        )
        .await;
    let registration = operation_id(&reply, OPERATION_TYPE_REGISTER_APPLICATION);
    let filed_at = Instant::now();

    // While the clock still owes the registration its terminal status, the listing
    // reports the record as running — the same thing the reader reports for it.
    let reply = list_operations(&emulator, json!({"ApplicationId": APPLICATION})).await;
    let running = read_operation(&emulator, &registration).await;
    if filed_at.elapsed() < TRANSITION_SPAN {
        let rows = listed(&reply);
        assert_eq!(
            id_set(&rows),
            BTreeSet::from([registration.clone()]),
            "the registration's record must be reported while it runs, got {rows:?}"
        );
        assert_eq!(
            string_field(&rows[0], "Status"),
            OPERATION_STATUS_INPROGRESS,
            "a listing must not end a running record, got {rows:?}"
        );
        assert_eq!(
            &read_record(&running),
            &rows[0],
            "a running row must be the record the reader reports, got {rows:?}"
        );
    }
    settle().await;

    // A deregistration files a record like every other call, and the application still
    // resolves until the removal the clock owes it falls due.
    let reply = emulator
        .send("/deregister-application", json!({"ApplicationId": APPLICATION}))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    let deregistered_at = Instant::now();
    let reply = list_operations(&emulator, json!({"ApplicationId": APPLICATION})).await;
    if deregistered_at.elapsed() < TRANSITION_SPAN {
        assert_eq!(
            reply.status, 200,
            "an application whose removal is still owed resolves, got {reply:?}"
        );
        let types: BTreeSet<String> = listed(&reply)
            .iter()
            .map(|row| string_field(row, "Type"))
            .collect();
        assert_eq!(
            types,
            BTreeSet::from([
                OPERATION_TYPE_REGISTER_APPLICATION.to_string(),
                OPERATION_TYPE_DEREGISTER_APPLICATION.to_string(),
            ]),
            "the deregistration is a record like every other, got {reply:?}"
        );
    }

    // Once the removal has fallen due the application resolves no more, so the listing
    // for it is the refusal an unresolvable reference always is.
    settle().await;
    let reply = list_operations(&emulator, json!({"ApplicationId": APPLICATION})).await;
    assert_refused(
        &reply,
        APPLICATION,
        &unresolvable_reference_message(APPLICATION),
    );
}

/// One filter row: the filter a request carries, over records whose attributes the test
/// already read off the reader.
struct FilterCase {
    name: &'static str,
    operator: &'static str,
    value: String,
}

/// RULE: a filter narrows the listing to the records whose value for the attribute it
/// names satisfies it — `Equals` exactly, the ordering operators lexicographically, a
/// comma-separated value when ANY of its values does — and several filters narrow
/// together.
#[tokio::test]
async fn filters_narrow_the_listing_by_the_attributes_it_reports() {
    let emulator = Emulator::new();
    let filed = file_every_kind(&emulator, APPLICATION).await;

    // The attributes each record reports for every filterable name, read off the reader
    // rather than assumed.
    let mut attributes: BTreeMap<String, BTreeMap<&str, String>> = BTreeMap::new();
    for id in &filed {
        let record = read_record(&read_operation(&emulator, id).await);
        let values: BTreeMap<&str, String> = OPERATION_FILTER_NAMES
            .iter()
            .map(|name| (*name, string_field(&record, name)))
            .collect();
        attributes.insert(id.clone(), values);
    }

    let some_id = filed.first().cloned().expect("a record was filed");
    let arn = attributes[&some_id]["ResourceArn"].clone();
    let mut cases = vec![
        // One record by its own id, and an id no record of this application carries.
        FilterCase {
            name: "Id",
            operator: EQUALS,
            value: some_id.clone(),
        },
        FilterCase {
            name: "Id",
            operator: EQUALS,
            value: "11111111-2222-3333-4444-555555555555".to_string(),
        },
        // One kind of call, and two of them: several values in one filter are joined
        // with an OR, as `Filter$Value`'s documentation states.
        FilterCase {
            name: "Type",
            operator: EQUALS,
            value: OPERATION_TYPE_START_APPLICATION.to_string(),
        },
        FilterCase {
            name: "Type",
            operator: EQUALS,
            value: format!("{OPERATION_TYPE_START_APPLICATION},{OPERATION_TYPE_STOP_APPLICATION}"),
        },
        // The terminal status every settled record carries, and the one none does.
        FilterCase {
            name: "Status",
            operator: EQUALS,
            value: OPERATION_STATUS_SUCCESS.to_string(),
        },
        FilterCase {
            name: "Status",
            operator: EQUALS,
            value: OPERATION_STATUS_ERROR.to_string(),
        },
        FilterCase {
            name: "Status",
            operator: EQUALS,
            value: format!("{OPERATION_STATUS_INPROGRESS},{OPERATION_STATUS_SUCCESS}"),
        },
        // The listing is already scoped to one application, so a filter on the resource
        // keeps every row or none.
        FilterCase {
            name: "ResourceId",
            operator: EQUALS,
            value: APPLICATION.to_string(),
        },
        FilterCase {
            name: "ResourceId",
            operator: EQUALS,
            value: OTHER_APPLICATION.to_string(),
        },
        FilterCase {
            name: "ResourceType",
            operator: EQUALS,
            value: APPLICATION_RESOURCE_TYPE.to_string(),
        },
        FilterCase {
            name: "ResourceType",
            operator: EQUALS,
            value: "DATABASE".to_string(),
        },
        FilterCase {
            name: "ResourceArn",
            operator: EQUALS,
            value: arn.clone(),
        },
        // The ordering operators compare the attribute lexicographically.
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
        FilterCase {
            name: "Type",
            operator: "GreaterThanOrEquals",
            value: OPERATION_TYPE_START_APPLICATION.to_string(),
        },
        FilterCase {
            name: "Type",
            operator: "LessThanOrEquals",
            value: OPERATION_TYPE_START_APPLICATION.to_string(),
        },
    ];
    // Every name this service filters this listing by and every operator the model
    // declares is exercised: the rows are read against the catalogues, so one added to
    // either fails here rather than going untested.
    for name in OPERATION_FILTER_NAMES {
        assert!(
            cases.iter().any(|case| case.name == *name),
            "the rows must exercise the filterable attribute {name:?}"
        );
    }
    for operator in FILTER_OPERATOR_VALUES {
        assert!(
            cases.iter().any(|case| case.operator == *operator),
            "the rows must exercise the modelled operator {operator:?}"
        );
    }

    // Two filters at once, to pin that they narrow TOGETHER rather than either one of
    // them deciding alone.
    let combined: Vec<(FilterCase, FilterCase)> = vec![
        (
            FilterCase {
                name: "Status",
                operator: EQUALS,
                value: OPERATION_STATUS_SUCCESS.to_string(),
            },
            FilterCase {
                name: "Type",
                operator: EQUALS,
                value: OPERATION_TYPE_STOP_APPLICATION.to_string(),
            },
        ),
        (
            FilterCase {
                name: "Status",
                operator: EQUALS,
                value: OPERATION_STATUS_INPROGRESS.to_string(),
            },
            FilterCase {
                name: "ResourceArn",
                operator: EQUALS,
                value: arn,
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
        "Filters": filters
            .iter()
            .map(|filter| json!({
                "Name": filter.name,
                "Value": filter.value,
                "Operator": filter.operator,
            }))
            .collect::<Vec<Value>>(),
    });
    let reply = list_operations(emulator, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200, got {reply:?}"
    );
    let rows = listed(&reply);
    assert_eq!(
        id_set(&rows),
        expected,
        "case {case:?}: expected exactly the records the filter selects, got {rows:?}"
    );
}

/// RULE: a filter this listing cannot apply is refused, and the refusal names what it
/// could not use.
#[tokio::test]
async fn a_filter_the_listing_does_not_apply_is_refused() {
    let emulator = Emulator::new();
    file_every_kind(&emulator, APPLICATION).await;

    // Rows: a name of another listing's catalogue, a member of this shape that is not
    // one the listing filters by, the right name in the wrong case (filter names are
    // case-sensitive) and a name no shape of this service carries.
    for name in [
        "ApplicationId",
        "StatusMessage",
        "StartTime",
        "id",
        "NotAMember",
    ] {
        let reply = list_operations(
            &emulator,
            json!({
                "ApplicationId": APPLICATION,
                "Filters": [{"Name": name, "Value": OPERATION_STATUS_SUCCESS, "Operator": EQUALS}],
            }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {name:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let actual = message(&reply);
        assert!(
            actual.contains(name),
            "case {name:?}: the refusal must name the filter it could not use, got {actual:?}"
        );
    }

    // An operator outside the model's own enum is refused at the boundary.
    for operator in ["Contains", "equals", ""] {
        let reply = list_operations(
            &emulator,
            json!({
                "ApplicationId": APPLICATION,
                "Filters": [{"Name": "Status", "Value": OPERATION_STATUS_SUCCESS, "Operator": operator}],
            }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {operator:?}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {operator:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }
}

/// Follow the listing's pages from the first to the last, returning the rows in the order
/// the pages reported them. Asserts the paging contract as it walks: no page holds more
/// than `max_results` rows, and only the last page comes without a `NextToken`.
async fn walk_pages(emulator: &Emulator, mut body: Value, max_results: i32) -> Vec<Value> {
    let case = format!("MaxResults {max_results}");
    let mut rows: Vec<Value> = Vec::new();
    let mut pages = 0;
    loop {
        let reply = list_operations(emulator, body.clone()).await;
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

/// RULE: a caller that follows the tokens sees every record exactly once, in the
/// listing's `Id` order, whatever page size it asked for — and a token this listing did
/// not issue is refused rather than resumed from.
#[tokio::test]
async fn following_the_tokens_reports_every_record_exactly_once() {
    let emulator = Emulator::new();
    let filed = file_every_kind(&emulator, APPLICATION).await;
    let every_id: BTreeSet<String> = filed.iter().cloned().collect();

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
            json!({"ApplicationId": APPLICATION, "MaxResults": max_results}),
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
        let reply = list_operations(
            &emulator,
            json!({"ApplicationId": APPLICATION, "MaxResults": max_results}),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {max_results}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {max_results}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    // A token this listing did not issue is refused: made-up values, and a token ANOTHER
    // listing of this service handed out, whose keys mean something else. Every row
    // satisfies the model's `NextToken` pattern (16..2048 characters), so the refusal is
    // the handler's and not the boundary validator's.
    let mut tokens: Vec<String> = vec![
        "0000000000000000".to_string(),
        "not-a-token-this-service-issued".to_string(),
        "ffffffffffffffffffffffffffffffff".to_string(),
    ];
    let reply = emulator
        .send("/list-applications", json!({"MaxResults": 1}))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListApplications, got {reply:?}"
    );
    if let Some(token) = reply.member("NextToken").and_then(Value::as_str) {
        tokens.push(token.to_string());
    }
    for token in tokens {
        let reply = list_operations(
            &emulator,
            json!({"ApplicationId": APPLICATION, "NextToken": token}),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {token:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let actual = message(&reply);
        assert!(
            actual.contains(&token),
            "case {token:?}: the refusal must name the token it could not resume from, got \
             {actual:?}"
        );
    }
}

/// RULE: the listing is scoped to the application it names in the Region it was signed
/// for — nothing of another application or another Region is reported, and a reference no
/// registration of this account and Region answers is refused.
#[tokio::test]
async fn the_listing_is_scoped_to_one_application_in_one_region() {
    let emulator = Emulator::new();
    let mine: BTreeSet<String> = file_every_kind(&emulator, APPLICATION)
        .await
        .into_iter()
        .collect();
    let theirs: BTreeSet<String> = file_every_kind(&emulator, OTHER_APPLICATION)
        .await
        .into_iter()
        .collect();

    for (application, expected, other) in [
        (APPLICATION, &mine, &theirs),
        (OTHER_APPLICATION, &theirs, &mine),
    ] {
        let reply = list_operations(&emulator, json!({"ApplicationId": application})).await;
        assert_eq!(
            reply.status, 200,
            "case {application:?}: expected 200, got {reply:?}"
        );
        let listed_ids = id_set(&listed(&reply));
        assert_eq!(
            &listed_ids, expected,
            "case {application:?}: the listing reports that application's records alone, got \
             {listed_ids:?}"
        );
        assert!(
            listed_ids.is_disjoint(other),
            "case {application:?}: no record of another application may be listed, got \
             {listed_ids:?}"
        );
    }

    // Rows: an id nothing registered, the registered id in another case (ids are compared
    // exactly), an id in the recorded probe's form, and the extremes of the modelled
    // `ApplicationId` length.
    for reference in [
        "NO-SUCH".to_string(),
        APPLICATION.to_lowercase(),
        "sim2real-probe-ssm-sap-app-8d37e33f".to_string(),
        "a".to_string(),
        "L".repeat(60),
    ] {
        let reply = list_operations(&emulator, json!({"ApplicationId": reference})).await;
        assert_refused(
            &reply,
            &reference,
            &unresolvable_reference_message(&reference),
        );
        assert!(
            reply.member("Operations").is_none(),
            "case {reference:?}: a refused reference may report nothing, got {reply:?}"
        );
    }

    // The service is regional: the application of one endpoint is not registered at
    // another, so the listing there is that refusal rather than a page of this Region's
    // records.
    let crossed = emulator
        .send_in_region(ELSEWHERE, LIST_OPERATIONS, json!({"ApplicationId": APPLICATION}))
        .await;
    assert_refused(
        &crossed,
        ELSEWHERE,
        &unresolvable_reference_message(APPLICATION),
    );
}

/// RULE: the listing REPORTS — it leaves every record exactly as the next caller would
/// have found it, so repeated listings agree and the reader reports the same record
/// before and after.
#[tokio::test]
async fn listing_the_records_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let filed = file_every_kind(&emulator, APPLICATION).await;
    let before: Vec<String> = {
        let mut witnesses = Vec::new();
        for id in &filed {
            witnesses.push(rendered(&read_operation(&emulator, id).await));
        }
        witnesses
    };

    let first = list_operations(&emulator, json!({"ApplicationId": APPLICATION})).await;
    for attempt in 1..3 {
        let again = list_operations(&emulator, json!({"ApplicationId": APPLICATION})).await;
        assert_eq!(
            listed(&again),
            listed(&first),
            "listing {attempt}: every listing of a settled set must report the same rows, got \
             {again:?}"
        );
    }

    for (id, witness) in filed.iter().zip(&before) {
        let after = rendered(&read_operation(&emulator, id).await);
        assert_eq!(
            &after, witness,
            "case {id:?}: listing the records must not change what the reader reports"
        );
    }
}

/// What this service reports for an application reference no registration of this account
/// and Region answers: `ListOperations` models no `ResourceNotFoundException`
/// (`smithy_query(query="operation", operation="ListOperations")`: `ValidationException`
/// 400 and `InternalServerException` 500 only), so that condition is the 400 it does
/// model, and its message is this operation's own wording for it — the id the request
/// carried, quoted, inside fixed text naming nothing of the environment the call reached.
/// Written out here so the assertion pins the wording;
/// `tests/all/validation_refusal_wording.rs` spans the rule over more references.
fn unresolvable_reference_message(reference: &str) -> String {
    format!("The application '{reference}' does not exist")
}

/// A refusal the handler raised: 400, `ValidationException`, and the exact message the
/// service words for the condition.
fn assert_refused(reply: &Reply, case: &str, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let actual = message(reply);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}
