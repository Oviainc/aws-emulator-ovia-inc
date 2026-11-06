// SPDX-License-Identifier: CC0-1.0

//! `ListInvestigations`: which investigations a behavior graph reports, narrowed by the
//! filter criteria, ordered by the sort criteria and served one page at a time.
//!
//! The rules under test:
//!
//! 1. A listing reports the investigations the named behavior graph holds, each one
//!    carrying the summary members of the very record a read of that investigation
//!    reports, and nothing scoped to another graph.
//! 2. Every member of `FilterCriteria` narrows the listing to the investigations whose
//!    matching attribute equals the filter's value — `CreatedTime` to the ones created
//!    inside its inclusive interval — and the criteria a request names apply together.
//!    A value no investigation carries selects none, which is an empty page.
//! 3. `SortCriteria` orders the results on the field it names — chronologically for
//!    `CREATED_TIME`, and by the rank the value has in the modelled enum's own
//!    declaration order for `SEVERITY` and `STATUS` — ascending unless `DESC` is asked
//!    for, with the investigation id breaking every tie so the order is total.
//! 4. `MaxResults` is the size of a page and `NextToken` names where the next one
//!    starts, so walking the pages yields every investigation exactly once, in the one
//!    order, and only the last page carries no token.
//! 5. A behavior graph this service does not hold is the modelled
//!    `ResourceNotFoundException` (404) with the investigation surface's fixed sentence,
//!    and a `NextToken` this operation never issued is the modelled
//!    `ValidationException` (400) that echoes the token back nowhere.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_ID_LEN, INVESTIGATION_RUN_MILLIS,
    MAX_INVESTIGATION_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call, create_graph_ok, emulator, error_code, error_message, ACCOUNT, REGION,
};

/// The URIs of the investigation operations, from
/// `smithy_query(query="operation",operation="ListInvestigations,StartInvestigation,GetInvestigation,UpdateInvestigationState")`.
const LIST_URI: &str = "/investigations/listInvestigations";
const START_URI: &str = "/investigations/startInvestigation";
const GET_URI: &str = "/investigations/getInvestigation";
const UPDATE_STATE_URI: &str = "/investigations/updateInvestigationState";

/// The modelled `Severity` values in the order the enum DECLARES them
/// (`smithy_query(query="shape",shape="Severity")`) — the ladder a `SEVERITY` sort runs
/// along, written out here so the suite does not read it from the code under test.
const SEVERITY_ORDER: [&str; 5] = ["INFORMATIONAL", "LOW", "MEDIUM", "HIGH", "CRITICAL"];

/// The modelled `Status` values in declaration order
/// (`smithy_query(query="shape",shape="Status")`), used exactly as [`SEVERITY_ORDER`].
const STATUS_ORDER: [&str; 3] = ["RUNNING", "FAILED", "SUCCESSFUL"];

/// The modelled `State` values (`smithy_query(query="shape",shape="State")`).
const STATE_ACTIVE: &str = "ACTIVE";
/// See [`STATE_ACTIVE`].
const STATE_ARCHIVED: &str = "ARCHIVED";

/// The summary members `InvestigationDetail` carries
/// (`smithy_query(query="shape",shape="InvestigationDetail")`), each one also reported by
/// `GetInvestigation`.
const DETAIL_MEMBERS: [&str; 7] = [
    "InvestigationId",
    "Severity",
    "Status",
    "State",
    "CreatedTime",
    "EntityArn",
    "EntityType",
];

/// `StartInvestigation` over `entity_arn`, which must succeed; answers the id it minted.
async fn start_investigation(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    entity_arn: &str,
) -> String {
    let (status, started) = call(
        router,
        START_URI,
        &json!({
            "GraphArn": graph_arn,
            "EntityArn": entity_arn,
            "ScopeStartTime": "2024-05-01T08:00:00Z",
            "ScopeEndTime": "2024-05-01T12:00:00Z",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartInvestigation expected 200, got {status} {started:?}"
    );
    let id = started
        .get("InvestigationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no InvestigationId in {started:?}"))
        .to_string();
    assert_eq!(
        id.len(),
        INVESTIGATION_ID_LEN,
        "case {case:?}: expected a modelled InvestigationId, got {id:?}"
    );
    id
}

/// One `ListInvestigations` call that must succeed; answers the parsed body.
async fn list_ok(router: &MultiServiceRouter, case: &str, request: &Value) -> Value {
    let (status, body) = call(router, LIST_URI, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInvestigations {request} expected 200, got {status} {body:?}"
    );
    body
}

/// The `InvestigationDetails` of one answer. An empty listing still carries the member,
/// so a caller never has to tell "none" from "absent".
fn details(case: &str, body: &Value) -> Vec<Value> {
    body.get("InvestigationDetails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the answer carried no InvestigationDetails: {body:?}")
        })
}

/// One member of a listed investigation.
fn member(case: &str, entry: &Value, name: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a listed investigation carried no {name}: {entry:?}"))
        .to_string()
}

/// The ids one answer listed, in the order it served them.
fn listed_ids(case: &str, body: &Value) -> Vec<String> {
    details(case, body)
        .iter()
        .map(|entry| member(case, entry, "InvestigationId"))
        .collect()
}

/// Every investigation of `graph_arn` the listing reports under `request`, walking the
/// pages when the answer carries a token.
async fn all_listed(
    router: &MultiServiceRouter,
    case: &str,
    request: &Map<String, Value>,
) -> Vec<Value> {
    let mut listed = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..1_000 {
        let mut body = request.clone();
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let page = list_ok(router, case, &Value::Object(body)).await;
        let page_details = details(case, &page);
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        listed.extend(page_details);
        if next_token.is_none() {
            return listed;
        }
    }
    panic!("case {case:?}: the listing never ran out of pages");
}

/// A request body naming only the graph, plus whatever the row adds.
fn request_for(graph_arn: &str, extra: &[(&str, Value)]) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("GraphArn".to_string(), json!(graph_arn));
    for (name, value) in extra {
        body.insert((*name).to_string(), value.clone());
    }
    body
}

/// `GetInvestigation`, which must succeed.
async fn get_investigation(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    investigation_id: &str,
) -> Value {
    let (status, got) = call(
        router,
        GET_URI,
        &json!({ "GraphArn": graph_arn, "InvestigationId": investigation_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetInvestigation expected 200, got {status} {got:?}"
    );
    got
}

/// One graph seeded with investigations the clock has already settled and others it has
/// not, so the rows below span more than one value of the attributes a request filters
/// and sorts on. Answers the graph ARN and the (entity ARN, id) pairs in start order.
async fn seeded_graph(router: &MultiServiceRouter) -> (String, Vec<(String, String)>) {
    let graph_arn = create_graph_ok(router, REGION, &json!({})).await;
    let entities = [
        format!("arn:aws:iam::{ACCOUNT}:user/alice"),
        format!("arn:aws:iam::{ACCOUNT}:role/auditor"),
        format!("arn:aws:iam::{ACCOUNT}:user/bob"),
    ];
    let mut started = Vec::new();
    for entity in &entities {
        let id = start_investigation(router, "seed-early", &graph_arn, entity).await;
        started.push((entity.clone(), id));
    }
    // Past the run span, so the first batch's runs are settled and the second batch is
    // created strictly later than the first.
    tokio::time::sleep(std::time::Duration::from_millis(
        (INVESTIGATION_RUN_MILLIS as u64) + 250,
    ))
    .await;
    for suffix in ["user/carol", "role/breakglass"] {
        let entity = format!("arn:aws:iam::{ACCOUNT}:{suffix}");
        let id = start_investigation(router, "seed-late", &graph_arn, &entity).await;
        started.push((entity, id));
    }
    (graph_arn, started)
}

/// The rank `value` has in a modelled enum's declaration order.
fn rank(case: &str, ladder: &[&str], value: &str) -> usize {
    ladder
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap_or_else(|| panic!("case {case:?}: {value:?} is not one of {ladder:?}"))
}

/// A rendered `date-time` as a key that orders CHRONOLOGICALLY as text.
///
/// An RFC 3339 rendering carries its fractional seconds only when they are non-zero,
/// and with trailing zeros trimmed (`...:11Z`, `...:11.1Z`, `...:11.284Z`), so the
/// renderings themselves do NOT sort chronologically: `Z` sorts after every digit, so
/// plain text order puts `...:11Z` (the whole second) after `...:11.284Z`, and
/// `...:11.1Z` (100 ms) after `...:11.12Z` (120 ms). Padding the fraction back out to
/// the nine digits the format admits restores the order, so a case can compare two
/// reported instants without parsing a calendar.
fn chronological_key(case: &str, rendered: &str) -> String {
    let body = rendered.strip_suffix('Z').unwrap_or_else(|| {
        panic!("case {case:?}: expected a UTC ISO8601 date-time, got {rendered:?}")
    });
    let (whole, fraction) = match body.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (body, ""),
    };
    format!("{whole}.{fraction:0<9}")
}

#[tokio::test]
async fn a_listing_reports_the_investigations_of_that_graph_as_a_read_of_each_reports_them() {
    let router = emulator();
    let (graph_arn, started) = seeded_graph(&router).await;
    // A second graph with its own investigation, so the listing is checked to be scoped
    // to the graph it names.
    let other_graph = create_graph_ok(&router, "us-east-1", &json!({})).await;
    let other_id = start_investigation(
        &router,
        "other-graph",
        &other_graph,
        &format!("arn:aws:iam::{ACCOUNT}:user/dave"),
    )
    .await;

    let case = "every-investigation";
    let listed = all_listed(&router, case, &request_for(&graph_arn, &[])).await;
    let mut listed_ids: Vec<String> = listed
        .iter()
        .map(|entry| member(case, entry, "InvestigationId"))
        .collect();
    listed_ids.sort();
    let mut expected: Vec<String> = started.iter().map(|(_, id)| id.clone()).collect();
    expected.sort();
    assert_eq!(
        listed_ids, expected,
        "case {case:?}: expected every investigation of the graph, got {listed:?}"
    );
    assert!(
        !listed_ids.contains(&other_id),
        "case {case:?}: expected nothing from another graph, got {listed:?}"
    );

    // Each listed entry carries every summary member, and each of those members holds
    // exactly what a read of that same investigation reports.
    for (entity_arn, id) in &started {
        let case = format!("round-trip-{id}");
        let entry = listed
            .iter()
            .find(|entry| member(&case, entry, "InvestigationId") == *id)
            .unwrap_or_else(|| panic!("case {case:?}: {id} was not listed: {listed:?}"));
        let read = get_investigation(&router, &case, &graph_arn, id).await;
        for name in DETAIL_MEMBERS {
            assert_eq!(
                entry.get(name),
                read.get(name),
                "case {case:?}: expected the listing's {name} to be the one a read reports, \
                 got {entry:?} against {read:?}"
            );
        }
        assert_eq!(
            member(&case, entry, "EntityArn"),
            *entity_arn,
            "case {case:?}: expected the entity the start named, got {entry:?}"
        );
    }

    // The entity kind is resolved from the ARN, so a user and a role are both reported.
    let kinds: Vec<String> = listed
        .iter()
        .map(|entry| member(case, entry, "EntityType"))
        .collect();
    for kind in ["IAM_USER", "IAM_ROLE"] {
        assert!(
            kinds.contains(&kind.to_string()),
            "case {case:?}: expected a listing spanning both entity kinds, got {kinds:?}"
        );
    }

    // A listing is a report: two of them in a row answer identically once every run the
    // clock owed is settled.
    tokio::time::sleep(std::time::Duration::from_millis(
        (INVESTIGATION_RUN_MILLIS as u64) + 250,
    ))
    .await;
    let first = list_ok(&router, "repeat", &json!({ "GraphArn": graph_arn })).await;
    let again = list_ok(&router, "repeat", &json!({ "GraphArn": graph_arn })).await;
    assert_eq!(
        first, again,
        "case \"repeat\": expected a repeated listing to answer identically, got {again:?}"
    );
}

#[tokio::test]
async fn an_investigation_a_graph_holds_none_of_is_an_empty_listing_rather_than_a_refusal() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let case = "no-investigations";
    let body = list_ok(&router, case, &json!({ "GraphArn": graph_arn })).await;
    assert_eq!(
        details(case, &body),
        Vec::<Value>::new(),
        "case {case:?}: expected an empty listing, got {body:?}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: expected no pagination token, got {body:?}"
    );
}

#[tokio::test]
async fn each_filter_criterion_narrows_the_listing_to_the_investigations_it_names() {
    let router = emulator();
    let (graph_arn, started) = seeded_graph(&router).await;
    let all_ids: Vec<String> = started.iter().map(|(_, id)| id.clone()).collect();
    let alice_arn = started[0].0.clone();
    let archived_id = started[2].1.clone();

    // Settle every run before filtering, so each investigation's `Status` is the
    // terminal outcome its run reached and the table's rows are about the filter alone.
    tokio::time::sleep(std::time::Duration::from_millis(
        (INVESTIGATION_RUN_MILLIS as u64) + 250,
    ))
    .await;
    let (status, _) = call(
        &router,
        UPDATE_STATE_URI,
        &json!({
            "GraphArn": graph_arn,
            "InvestigationId": archived_id,
            "State": STATE_ARCHIVED,
        }),
    )
    .await;
    assert_eq!(status, 200, "archiving one investigation of the graph");

    // What each investigation now carries, read back through the full listing, so the
    // expectations below are about the stored records rather than about the filter.
    let case = "unfiltered";
    let listed = all_listed(&router, case, &request_for(&graph_arn, &[])).await;
    let created_times: Vec<String> = listed
        .iter()
        .map(|entry| member(case, entry, "CreatedTime"))
        .collect();
    let latest_created = created_times
        .iter()
        // The chronologically latest instant, which is not the textually largest
        // rendering (see `chronological_key`); the value kept is the rendering itself,
        // since that is what the filter row below carries.
        .max_by_key(|rendered| chronological_key(case, rendered))
        .expect("the seeded graph holds investigations")
        .clone();
    let ids_created_at_latest: Vec<String> = listed
        .iter()
        .filter(|entry| member(case, entry, "CreatedTime") == latest_created)
        .map(|entry| member(case, entry, "InvestigationId"))
        .collect();
    let active_ids: Vec<String> = all_ids
        .iter()
        .filter(|id| **id != archived_id)
        .cloned()
        .collect();

    // The table walks every member of `FilterCriteria`, each with a value investigations
    // DO carry and a value from the same modelled set that none carries, plus an
    // interval that covers everything, one that covers nothing, one that covers the
    // latest investigations only, an inverted one, and two criteria applied together.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "severity-informational",
            json!({ "Severity": { "Value": SEVERITY_ORDER[0] } }),
            all_ids.clone(),
        ),
        (
            "severity-critical",
            json!({ "Severity": { "Value": SEVERITY_ORDER[4] } }),
            Vec::new(),
        ),
        (
            "status-failed",
            json!({ "Status": { "Value": "FAILED" } }),
            all_ids.clone(),
        ),
        (
            "status-running",
            json!({ "Status": { "Value": "RUNNING" } }),
            Vec::new(),
        ),
        (
            "status-successful",
            json!({ "Status": { "Value": "SUCCESSFUL" } }),
            Vec::new(),
        ),
        (
            "state-active",
            json!({ "State": { "Value": STATE_ACTIVE } }),
            active_ids.clone(),
        ),
        (
            "state-archived",
            json!({ "State": { "Value": STATE_ARCHIVED } }),
            vec![archived_id.clone()],
        ),
        (
            "entity-arn-of-a-held-investigation",
            json!({ "EntityArn": { "Value": alice_arn } }),
            vec![started[0].1.clone()],
        ),
        (
            "entity-arn-nothing-was-started-on",
            json!({ "EntityArn": { "Value": format!("arn:aws:iam::{ACCOUNT}:user/nobody") } }),
            Vec::new(),
        ),
        (
            "created-time-covering-everything",
            json!({ "CreatedTime": {
                "StartInclusive": "2000-01-01T00:00:00Z",
                "EndInclusive": "2100-01-01T00:00:00Z",
            } }),
            all_ids.clone(),
        ),
        (
            "created-time-covering-nothing",
            json!({ "CreatedTime": {
                "StartInclusive": "2000-01-01T00:00:00Z",
                "EndInclusive": "2001-01-01T00:00:00Z",
            } }),
            Vec::new(),
        ),
        (
            "created-time-from-the-latest-inclusive",
            json!({ "CreatedTime": {
                "StartInclusive": latest_created,
                "EndInclusive": "2100-01-01T00:00:00Z",
            } }),
            ids_created_at_latest.clone(),
        ),
        (
            "created-time-inverted",
            json!({ "CreatedTime": {
                "StartInclusive": "2100-01-01T00:00:00Z",
                "EndInclusive": "2000-01-01T00:00:00Z",
            } }),
            Vec::new(),
        ),
        (
            "two-criteria-that-agree",
            json!({
                "Status": { "Value": "FAILED" },
                "State": { "Value": STATE_ARCHIVED },
            }),
            vec![archived_id.clone()],
        ),
        (
            "two-criteria-that-disagree",
            json!({
                "Status": { "Value": "RUNNING" },
                "State": { "Value": STATE_ARCHIVED },
            }),
            Vec::new(),
        ),
    ];

    for (case, criteria, expected) in rows {
        let listed = all_listed(
            &router,
            case,
            &request_for(&graph_arn, &[("FilterCriteria", criteria.clone())]),
        )
        .await;
        let mut got: Vec<String> = listed
            .iter()
            .map(|entry| member(case, entry, "InvestigationId"))
            .collect();
        got.sort();
        let mut want = expected.clone();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: filter {criteria} expected {want:?}, got {got:?}"
        );
    }

    // Filtering changed nothing: the unfiltered listing is what it was.
    let after = all_listed(&router, "after-filtering", &request_for(&graph_arn, &[])).await;
    assert_eq!(
        after, listed,
        "case \"after-filtering\": expected the filters to store nothing, got {after:?}"
    );
}

#[tokio::test]
async fn the_results_are_ordered_by_the_field_the_request_names_in_the_order_it_asks_for() {
    let router = emulator();
    let (graph_arn, started) = seeded_graph(&router).await;
    let mut expected_ids: Vec<String> = started.iter().map(|(_, id)| id.clone()).collect();
    expected_ids.sort();

    // The rows differ along the sort field, the direction, and whether either member is
    // named at all: `SortCriteria`'s two members are both optional.
    let rows: Vec<(&str, Value)> = vec![
        ("no-sort-criteria", Value::Null),
        ("order-only-asc", json!({ "SortOrder": "ASC" })),
        ("order-only-desc", json!({ "SortOrder": "DESC" })),
        ("created-time-default-order", json!({ "Field": "CREATED_TIME" })),
        (
            "created-time-asc",
            json!({ "Field": "CREATED_TIME", "SortOrder": "ASC" }),
        ),
        (
            "created-time-desc",
            json!({ "Field": "CREATED_TIME", "SortOrder": "DESC" }),
        ),
        ("severity-asc", json!({ "Field": "SEVERITY", "SortOrder": "ASC" })),
        (
            "severity-desc",
            json!({ "Field": "SEVERITY", "SortOrder": "DESC" }),
        ),
        ("status-asc", json!({ "Field": "STATUS", "SortOrder": "ASC" })),
        ("status-desc", json!({ "Field": "STATUS", "SortOrder": "DESC" })),
    ];

    let mut distinct_created_time_rows = 0usize;
    for (case, criteria) in rows {
        let extra: Vec<(&str, Value)> = if criteria.is_null() {
            Vec::new()
        } else {
            vec![("SortCriteria", criteria.clone())]
        };
        let listed = all_listed(&router, case, &request_for(&graph_arn, &extra)).await;
        let ids: Vec<String> = listed
            .iter()
            .map(|entry| member(case, entry, "InvestigationId"))
            .collect();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        assert_eq!(
            sorted_ids, expected_ids,
            "case {case:?}: a sort reports every investigation exactly once, got {ids:?}"
        );

        let descending = criteria
            .get("SortOrder")
            .and_then(Value::as_str)
            .is_some_and(|order| order == "DESC");
        let field = criteria.get("Field").and_then(Value::as_str);
        // The key each entry sorts by, read out of the entry's own reported members.
        let keys: Vec<(i64, String)> = listed
            .iter()
            .map(|entry| {
                let primary = match field {
                    Some("SEVERITY") => {
                        rank(case, &SEVERITY_ORDER, &member(case, entry, "Severity")) as i64
                    }
                    Some("STATUS") => {
                        rank(case, &STATUS_ORDER, &member(case, entry, "Status")) as i64
                    }
                    // `CreatedTime` is compared as the chronological key its rendering
                    // implies (see `chronological_key`); a field-less row sorts on the
                    // id alone.
                    _ => 0,
                };
                let secondary = match field {
                    Some("CREATED_TIME") => format!(
                        "{}#{}",
                        chronological_key(case, &member(case, entry, "CreatedTime")),
                        member(case, entry, "InvestigationId")
                    ),
                    _ => member(case, entry, "InvestigationId"),
                };
                (primary, secondary)
            })
            .collect();
        if field == Some("CREATED_TIME") {
            let distinct: std::collections::BTreeSet<&str> = listed
                .iter()
                .filter_map(|entry| entry.get("CreatedTime").and_then(Value::as_str))
                .collect();
            if distinct.len() > 1 {
                distinct_created_time_rows += 1;
            }
        }
        for pair in keys.windows(2) {
            let (left, right) = (&pair[0], &pair[1]);
            if descending {
                assert!(
                    left > right,
                    "case {case:?}: expected a strictly descending order, got {left:?} before \
                     {right:?} in {ids:?}"
                );
            } else {
                assert!(
                    left < right,
                    "case {case:?}: expected a strictly ascending order, got {left:?} before \
                     {right:?} in {ids:?}"
                );
            }
        }
    }
    assert!(
        distinct_created_time_rows > 0,
        "the seeded graph must hold investigations created at different instants for the \
         CREATED_TIME rows to order anything, got {distinct_created_time_rows} such rows"
    );
}

#[tokio::test]
async fn a_page_is_the_size_the_request_asks_for_and_the_token_walks_the_rest_exactly_once() {
    let router = emulator();
    let (graph_arn, started) = seeded_graph(&router).await;
    let held = started.len();

    // Settle every run first, so what a page reports cannot change between pages.
    tokio::time::sleep(std::time::Duration::from_millis(
        (INVESTIGATION_RUN_MILLIS as u64) + 250,
    ))
    .await;
    let unpaged = listed_ids(
        "unpaged",
        &list_ok(&router, "unpaged", &json!({ "GraphArn": graph_arn })).await,
    );
    assert_eq!(
        unpaged.len(),
        held,
        "a request naming no MaxResults serves every investigation at once, got {unpaged:?}"
    );

    // The rows walk the page size: both published bounds of the modelled `MaxResults`
    // range (1..100) and the sizes either side of what the graph holds.
    let sizes = [1, 2, held as i32, (held as i32) + 1, MAX_INVESTIGATION_PAGE_SIZE as i32];
    for max_results in sizes {
        let case = format!("max-results-{max_results}");
        let mut walked = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = request_for(&graph_arn, &[("MaxResults", json!(max_results))]);
            if let Some(token) = &next_token {
                body.insert("NextToken".to_string(), json!(token));
            }
            let page = list_ok(&router, &case, &Value::Object(body)).await;
            let ids = listed_ids(&case, &page);
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert!(
                ids.len() <= max_results as usize,
                "case {case:?}: a page is never larger than the size asked for, got {ids:?}"
            );
            if next_token.is_some() {
                assert_eq!(
                    ids.len(),
                    max_results as usize,
                    "case {case:?}: only the last page is short, got {ids:?}"
                );
            }
            walked.extend(ids);
            pages += 1;
            assert!(pages <= held + 1, "case {case:?}: the listing never ended");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case:?}: expected the pages to serve every investigation exactly once, in \
             the unpaged order, got {walked:?}"
        );
        let expected_pages = held.div_ceil(max_results as usize).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {held} investigations, got \
             {pages}"
        );
    }

    // A page size outside the modelled range is refused by the constraint walk, so the
    // rows are the two values just past the published bounds.
    for max_results in [0, (MAX_INVESTIGATION_PAGE_SIZE as i32) + 1] {
        let (status, answered) = call(
            &router,
            LIST_URI,
            &json!({ "GraphArn": graph_arn, "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case MaxResults={max_results}: got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_graph_or_a_token_this_service_does_not_hold_is_refused() {
    let router = emulator();
    let (graph_arn, started) = seeded_graph(&router).await;

    // A well-formed `GraphArn` the service never minted: the rows differ along which
    // field of the ARN names nothing, so each satisfies the modelled pattern and the
    // refusal is this operation's rather than the constraint walk's.
    let absent_graphs = [
        (
            "unknown-id",
            format!("arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}", "a".repeat(32)),
        ),
        ("another-region", graph_arn.replace(REGION, "eu-central-1")),
        ("another-account", graph_arn.replace(ACCOUNT, "123456789012")),
    ];
    for (case, absent) in absent_graphs {
        let (status, answered) = call(&router, LIST_URI, &json!({ "GraphArn": absent })).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: got {answered:?}"
        );
        assert_eq!(
            error_message(&answered),
            INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the investigation surface's fixed graph-not-found \
             sentence, got {answered:?}"
        );
    }

    // A `NextToken` this operation never issued: the rows differ along why it resolves
    // to no position — not a token at all, a token naming something that is not an
    // investigation id, and one naming a well-formed id the result set does not hold
    // (which is also how an expired token reads).
    let absent_id = "9".repeat(INVESTIGATION_ID_LEN);
    assert!(
        started.iter().all(|(_, id)| *id != absent_id),
        "the row itself must name no stored investigation"
    );
    // A token is the hex of the key it wraps, which is how a caller sees one; `hex` is
    // the crate the service encodes them with, so no hand-rolled codec appears here.
    let tokens = [
        ("not-a-token", "not-a-token".to_string()),
        ("hex-of-a-graph-arn", hex::encode(&graph_arn)),
        ("hex-of-a-short-id", hex::encode("1234")),
        ("hex-of-an-absent-id", hex::encode(&absent_id)),
    ];
    for (case, token) in tokens {
        let (status, answered) = call(
            &router,
            LIST_URI,
            &json!({ "GraphArn": graph_arn, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: got {answered:?}"
        );
        let message = error_message(&answered);
        assert!(
            !message.contains(&token),
            "case {case:?}: the refusal echoes the token back nowhere, got {message:?}"
        );
    }

    // Every refusal above stored nothing: the graph still holds what it held.
    let listed = all_listed(&router, "after-the-refusals", &request_for(&graph_arn, &[])).await;
    assert_eq!(
        listed.len(),
        started.len(),
        "expected a refused listing to store nothing, got {listed:?}"
    );
}
