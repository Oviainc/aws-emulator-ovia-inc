// SPDX-License-Identifier: CC0-1.0

//! `ListApplicationComponents`: the account's application components, read end to end over
//! HTTP.
//!
//! The rule under test is the one the operation states for ANY portfolio, not for one
//! request: the answer is the application components this service holds, scoped by the
//! import groups `groupIdFilter` names, selected by the member the
//! `applicationComponentCriteria` names compared to `filterValue`, ordered over the
//! components' one total order (their id) in the direction `sort` asks for, and handed out
//! in pages of at most `maxResults` that a `nextToken` resumes — each reported entry being
//! the stored record, exactly as `GetApplicationComponentDetails` reports it — while a
//! request the model admits member by member but that the operation cannot act on (a
//! criteria naming a member with no value to compare it to, a `maxResults` past the
//! documented maximum, an enum value the model does not declare, a token this service never
//! issued) is refused with 400 `ValidationException`.
//!
//! Each table below therefore spans the class the rule generalises over — every
//! `ApplicationComponentCriteria` value, several values per criteria, one group and several,
//! every page size over the fixture and both sort orders — rather than one filtered list.
//!
//! An application component enters this service only through the account's data being
//! brought in (the import-loop issue, filed separately: `StartImportFileTask` reaches
//! `ImportSuccess` having ingested nothing). So these cases seed the component records
//! directly through the shared engine — the same `application_components` map that
//! ingestion writes and this operation reads — and then drive every assertion through the
//! router, so the handler, its refusals and the serialized `ApplicationComponentDetail` are
//! exercised exactly as a client reaches them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_component_details::get_component;
use crate::import_and_assess::{at, call, Answer, TEST_REGION};

/// Every value `ApplicationComponentCriteria` admits, from
/// `smithy_query(query="shape",shape="ApplicationComponentCriteria")`. Spelled out here so
/// the table below can be checked to cover the whole catalog rather than one criteria.
const APPLICATION_COMPONENT_CRITERIA: &[&str] = &[
    "ANALYSIS_STATUS",
    "APP_NAME",
    "APP_TYPE",
    "DESTINATION",
    "ERROR_CATEGORY",
    "NOT_DEFINED",
    "SERVER_ID",
    "STRATEGY",
];

/// Every value `GroupName` admits, from `smithy_query(query="shape",shape="GroupName")`.
const GROUP_NAMES: &[&str] = &["ExternalId", "ExternalSourceType"];

/// The documented maximum of `maxResults`: "The maximum number of items to include in the
/// response. The maximum value is 100."
/// (`smithy_query(query="operation",operation="ListApplicationComponents")` ->
/// `maxResults`), spelled out independently of the crate's constant.
const MAX_RESULTS_MAX: i64 = 100;

/// One `groupIdFilter` entry of a request.
fn group(name: &str, value: &str) -> Value {
    assert!(
        GROUP_NAMES.contains(&name),
        "a group name GroupName does not declare: {name:?}"
    );
    json!({"name": name, "value": value})
}

/// One import group, as the ingestion that brought a component in stored it.
fn stored_group(name: &str, value: &str) -> Group {
    Group {
        name: Some(name.to_string()),
        value: Some(value.to_string()),
    }
}

fn external_id(value: &str) -> Group {
    stored_group("ExternalId", value)
}

fn source_type(value: &str) -> Group {
    stored_group("ExternalSourceType", value)
}

/// A component record holding nothing but its id — a `ResourceId` (`^[0-9a-b]+`, at most 44
/// characters), which is also what keys the store.
fn component(id: &str) -> ApplicationComponentRecord {
    assert!(
        !id.is_empty()
            && id.len() <= 44
            && id.chars().all(|c| c.is_ascii_digit() || c == 'a' || c == 'b'),
        "a component id ResourceId (^[0-9a-b]+, max 44) does not admit: {id:?}"
    );
    ApplicationComponentRecord {
        id: id.to_string(),
        ..Default::default()
    }
}

/// The portfolio every selection case runs against: six components differing in name (two
/// of them sharing one), in app type, in recommended strategy and destination, in analysis
/// status, in whether an error category was recorded, in how many servers they run on
/// (none, one, two) and in how many import groups they came in with (none, one, two).
///
/// Values come from the model's own catalogs
/// (`smithy_query(query="shape",shape="AppType,ResourceSubType,SrcCodeOrDbAnalysisStatus,Strategy,TargetDestination,AppUnitErrorCategory")`).
/// Ids are deliberately NOT inserted in order, so nothing can pass by accident of insertion
/// order.
fn portfolio() -> Vec<ApplicationComponentRecord> {
    vec![
        ApplicationComponentRecord {
            name: Some("billing-web".to_string()),
            app_type: Some("IIS".to_string()),
            resource_sub_type: Some("Process".to_string()),
            analysis_status: Some("ANALYSIS_FAILED".to_string()),
            app_unit_error_category: Some("CREDENTIAL_ERROR".to_string()),
            associated_server_ids: vec!["2a".to_string()],
            group_id: vec![external_id("wave-2"), source_type("MPA")],
            ..component("1a")
        },
        ApplicationComponentRecord {
            name: Some("orders-api".to_string()),
            app_type: Some("Java".to_string()),
            resource_sub_type: Some("Process".to_string()),
            analysis_status: Some("ANALYSIS_SUCCESS".to_string()),
            strategy: Some("Refactor".to_string()),
            target_destination: Some("AWS Fargate".to_string()),
            associated_server_ids: vec!["1a".to_string()],
            group_id: vec![external_id("wave-1")],
            ..component("0a")
        },
        ApplicationComponentRecord {
            name: Some("reports-mongo".to_string()),
            app_type: Some("Mongo DB".to_string()),
            resource_sub_type: Some("Database".to_string()),
            analysis_status: Some("CONFIGURED".to_string()),
            strategy: Some("Retain".to_string()),
            target_destination: Some("None specified".to_string()),
            associated_server_ids: vec!["1a".to_string()],
            group_id: vec![external_id("wave-2")],
            ..component("2b")
        },
        ApplicationComponentRecord {
            // The SAME name as "0a": a name is not an id, so a name filter can select
            // more than one component.
            name: Some("orders-api".to_string()),
            app_type: Some("Java".to_string()),
            resource_sub_type: Some("Process".to_string()),
            analysis_status: Some("ANALYSIS_TO_BE_SCHEDULED".to_string()),
            strategy: Some("Rehost".to_string()),
            target_destination: Some("Amazon Elastic Cloud Compute (EC2)".to_string()),
            associated_server_ids: vec!["2b".to_string()],
            group_id: vec![source_type("MPA")],
            ..component("1b")
        },
        ApplicationComponentRecord {
            name: Some("legacy-batch".to_string()),
            app_type: Some("DotNetFramework".to_string()),
            resource_sub_type: Some("Process".to_string()),
            analysis_status: Some("UNCONFIGURED".to_string()),
            app_unit_error_category: Some("PERMISSION_ERROR".to_string()),
            // A component no server association and no import group was recorded for.
            associated_server_ids: Vec::new(),
            group_id: Vec::new(),
            ..component("2a")
        },
        ApplicationComponentRecord {
            name: Some("orders-db".to_string()),
            app_type: Some("Oracle".to_string()),
            resource_sub_type: Some("DatabaseProcess".to_string()),
            analysis_status: Some("ANALYSIS_SUCCESS".to_string()),
            strategy: Some("Replatform".to_string()),
            target_destination: Some("Aurora PostgreSQL".to_string()),
            database_secret_name: Some("orders-db-secret".to_string()),
            // Two servers: a value equal to EITHER selects this component.
            associated_server_ids: vec!["1a".to_string(), "1b".to_string()],
            group_id: vec![external_id("wave-1")],
            ..component("0b")
        },
    ]
}

/// Every id in [`portfolio`], in the ascending order the service reports them in.
fn all_ids_ascending() -> Vec<String> {
    let mut ids: Vec<String> = portfolio()
        .into_iter()
        .map(|component| component.id)
        .collect();
    ids.sort();
    ids
}

/// One emulator over one shared engine, with `application_components` seeded into this
/// service's store — the same map an ingestion writes and this operation reads.
fn emulator_with(components: Vec<ApplicationComponentRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        for record in components {
            portfolio
                .application_components
                .insert(record.id.clone(), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn list_components(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/list-applicationcomponents", Some(request)).await
}

/// The ids `applicationComponentInfos` reported, in the order they were reported.
fn reported_ids(answer: &Answer) -> Vec<String> {
    match answer
        .body
        .get("applicationComponentInfos")
        .and_then(Value::as_array)
    {
        Some(items) => items
            .iter()
            .map(|component| component["id"].as_str().unwrap_or_default().to_string())
            .collect(),
        None => Vec::new(),
    }
}

/// A `ListApplicationComponents` request and the ids it must report, in order.
struct Selection {
    what: &'static str,
    request: Value,
    expected: Vec<&'static str>,
}

/// Each `ApplicationComponentCriteria` value selects on the member it names, compared to
/// `filterValue` for equality; `NOT_DEFINED` names no member, so it selects everything.
///
/// Every criteria the catalog declares gets at least two rows: a value that selects, and a
/// value from the same model catalog that nothing in the portfolio carries.
fn criteria_cases() -> Vec<Selection> {
    let every: Vec<&'static str> = ["0a", "0b", "1a", "1b", "2a", "2b"].to_vec();
    assert_eq!(
        all_ids_ascending(),
        every.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
        "the fixture's ascending order is not what this table expects"
    );

    vec![
        Selection {
            what: "no applicationComponentCriteria at all",
            request: json!({}),
            expected: every.clone(),
        },
        Selection {
            what: "NOT_DEFINED, which names no member to filter on",
            request: json!({"applicationComponentCriteria": "NOT_DEFINED"}),
            expected: every.clone(),
        },
        Selection {
            what: "NOT_DEFINED with a filterValue, which it has no member to apply to",
            request: json!({
                "applicationComponentCriteria": "NOT_DEFINED",
                "filterValue": "Java",
            }),
            expected: every.clone(),
        },
        Selection {
            what: "APP_NAME on a name two components share",
            request: json!({
                "applicationComponentCriteria": "APP_NAME",
                "filterValue": "orders-api",
            }),
            expected: vec!["0a", "1b"],
        },
        Selection {
            what: "APP_NAME on a name exactly one component carries",
            request: json!({
                "applicationComponentCriteria": "APP_NAME",
                "filterValue": "legacy-batch",
            }),
            expected: vec!["2a"],
        },
        Selection {
            what: "APP_NAME on a name nothing carries",
            request: json!({
                "applicationComponentCriteria": "APP_NAME",
                "filterValue": "orders-api-v2",
            }),
            expected: Vec::new(),
        },
        Selection {
            what: "APP_TYPE on an AppType two components carry",
            request: json!({"applicationComponentCriteria": "APP_TYPE", "filterValue": "Java"}),
            expected: vec!["0a", "1b"],
        },
        Selection {
            what: "APP_TYPE on an AppType whose value carries a space",
            request: json!({"applicationComponentCriteria": "APP_TYPE", "filterValue": "Mongo DB"}),
            expected: vec!["2b"],
        },
        Selection {
            what: "APP_TYPE on an AppType nothing carries",
            request: json!({"applicationComponentCriteria": "APP_TYPE", "filterValue": "Tomcat"}),
            expected: Vec::new(),
        },
        Selection {
            what: "SERVER_ID on a server three components run on",
            request: json!({"applicationComponentCriteria": "SERVER_ID", "filterValue": "1a"}),
            expected: vec!["0a", "0b", "2b"],
        },
        Selection {
            what: "SERVER_ID on the SECOND server of a component's list",
            request: json!({"applicationComponentCriteria": "SERVER_ID", "filterValue": "1b"}),
            expected: vec!["0b"],
        },
        Selection {
            what: "SERVER_ID on a server nothing runs on",
            request: json!({"applicationComponentCriteria": "SERVER_ID", "filterValue": "0a0a"}),
            expected: Vec::new(),
        },
        Selection {
            what: "STRATEGY on a recommended strategy",
            request: json!({"applicationComponentCriteria": "STRATEGY", "filterValue": "Rehost"}),
            expected: vec!["1b"],
        },
        Selection {
            what: "STRATEGY on a Strategy value nothing was recommended",
            request: json!({
                "applicationComponentCriteria": "STRATEGY",
                "filterValue": "Retirement",
            }),
            expected: Vec::new(),
        },
        Selection {
            what: "DESTINATION on a recommended target",
            request: json!({
                "applicationComponentCriteria": "DESTINATION",
                "filterValue": "Aurora PostgreSQL",
            }),
            expected: vec!["0b"],
        },
        Selection {
            what: "DESTINATION on TargetDestination's \"None specified\"",
            request: json!({
                "applicationComponentCriteria": "DESTINATION",
                "filterValue": "None specified",
            }),
            expected: vec!["2b"],
        },
        Selection {
            what: "DESTINATION on a TargetDestination nothing was recommended",
            request: json!({
                "applicationComponentCriteria": "DESTINATION",
                "filterValue": "Amazon Relational Database Service on MySQL",
            }),
            expected: Vec::new(),
        },
        Selection {
            what: "ANALYSIS_STATUS on a status two components are in",
            request: json!({
                "applicationComponentCriteria": "ANALYSIS_STATUS",
                "filterValue": "ANALYSIS_SUCCESS",
            }),
            expected: vec!["0a", "0b"],
        },
        Selection {
            what: "ANALYSIS_STATUS on a SrcCodeOrDbAnalysisStatus nothing is in",
            request: json!({
                "applicationComponentCriteria": "ANALYSIS_STATUS",
                "filterValue": "ANALYSIS_STARTED",
            }),
            expected: Vec::new(),
        },
        Selection {
            what: "ERROR_CATEGORY on a recorded error category",
            request: json!({
                "applicationComponentCriteria": "ERROR_CATEGORY",
                "filterValue": "CREDENTIAL_ERROR",
            }),
            expected: vec!["1a"],
        },
        Selection {
            what: "ERROR_CATEGORY on an AppUnitErrorCategory no analysis recorded",
            request: json!({
                "applicationComponentCriteria": "ERROR_CATEGORY",
                "filterValue": "OTHER_ERROR",
            }),
            expected: Vec::new(),
        },
    ]
}

/// `groupIdFilter` scopes the list to the components that came in with one of the groups it
/// names: one group, several (which widens the selection), a group nothing came in with,
/// and a group filter combined with a criteria (which narrows it).
fn group_filter_cases() -> Vec<Selection> {
    vec![
        Selection {
            what: "one ExternalId group",
            request: json!({"groupIdFilter": [group("ExternalId", "wave-1")]}),
            expected: vec!["0a", "0b"],
        },
        Selection {
            what: "another ExternalId group, one of whose components is in two groups",
            request: json!({"groupIdFilter": [group("ExternalId", "wave-2")]}),
            expected: vec!["1a", "2b"],
        },
        Selection {
            what: "an ExternalSourceType group, a different GroupName",
            request: json!({"groupIdFilter": [group("ExternalSourceType", "MPA")]}),
            expected: vec!["1a", "1b"],
        },
        Selection {
            what: "two groups of different names, which widens the selection",
            request: json!({
                "groupIdFilter": [
                    group("ExternalId", "wave-1"),
                    group("ExternalSourceType", "MPA"),
                ],
            }),
            expected: vec!["0a", "0b", "1a", "1b"],
        },
        Selection {
            what: "a group value nothing came in with",
            request: json!({"groupIdFilter": [group("ExternalId", "wave-9")]}),
            expected: Vec::new(),
        },
        Selection {
            what: "a group name that is right with a value that is not",
            request: json!({"groupIdFilter": [group("ExternalSourceType", "wave-1")]}),
            expected: Vec::new(),
        },
        Selection {
            what: "an empty group filter, which scopes nothing",
            request: json!({"groupIdFilter": []}),
            expected: vec!["0a", "0b", "1a", "1b", "2a", "2b"],
        },
        Selection {
            what: "a group filter and a criteria together, which narrows",
            request: json!({
                "groupIdFilter": [group("ExternalId", "wave-2")],
                "applicationComponentCriteria": "APP_TYPE",
                "filterValue": "IIS",
            }),
            expected: vec!["1a"],
        },
        Selection {
            what: "a group filter and a criteria that select disjoint components",
            request: json!({
                "groupIdFilter": [group("ExternalId", "wave-1")],
                "applicationComponentCriteria": "ERROR_CATEGORY",
                "filterValue": "CREDENTIAL_ERROR",
            }),
            expected: Vec::new(),
        },
    ]
}

/// Every selection row reports exactly the components its filter selects, in ascending id
/// order.
#[tokio::test]
async fn a_filter_selects_the_components_the_member_it_names_matches() {
    let router = emulator_with(portfolio());

    let mut cases = criteria_cases();
    let covered: Vec<&str> = cases
        .iter()
        .filter_map(|case| {
            case.request
                .get("applicationComponentCriteria")
                .and_then(Value::as_str)
        })
        .collect();
    for criteria in APPLICATION_COMPONENT_CRITERIA {
        assert!(
            covered.contains(criteria),
            "this table does not cover the ApplicationComponentCriteria value {criteria:?}"
        );
    }

    cases.extend(group_filter_cases());
    for Selection {
        what,
        request,
        expected,
    } in cases
    {
        let answer = list_components(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200 from {request}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_ids(&answer),
            expected,
            "case {what:?}: {request} must report {expected:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/nextToken"),
            Value::Null,
            "case {what:?}: a page that ended the list carries no nextToken, got {:?}",
            at(&answer.body, "/nextToken")
        );
    }
}

/// `sort` reverses the order over the components' one total order, their id: `DESC` is
/// `ASC` read backwards, and an absent `sort` is `ASC`. Checked unfiltered and under a
/// filter, so the order is a property of the answer rather than of the whole portfolio.
#[tokio::test]
async fn sort_order_reverses_the_reported_order() {
    let router = emulator_with(portfolio());

    let probes = vec![
        ("the whole portfolio", json!({})),
        (
            "the components of one group",
            json!({"groupIdFilter": [group("ExternalId", "wave-2")]}),
        ),
        (
            "the components one criteria selects",
            json!({"applicationComponentCriteria": "SERVER_ID", "filterValue": "1a"}),
        ),
    ];

    for (what, request) in probes {
        let mut ascending_request = request.clone();
        ascending_request["sort"] = json!("ASC");
        let mut descending_request = request.clone();
        descending_request["sort"] = json!("DESC");

        let implied = reported_ids(&list_components(&router, request.clone()).await);
        let ascending = reported_ids(&list_components(&router, ascending_request).await);
        let descending = reported_ids(&list_components(&router, descending_request).await);

        assert_eq!(
            implied, ascending,
            "case {what:?}: a request naming no sort is ASC, got {implied:?} against \
             {ascending:?}"
        );
        let mut sorted = ascending.clone();
        sorted.sort();
        assert_eq!(
            ascending, sorted,
            "case {what:?}: ASC reports the components in ascending id order, got {ascending:?}"
        );
        let reversed: Vec<String> = ascending.iter().rev().cloned().collect();
        assert_eq!(
            descending, reversed,
            "case {what:?}: DESC is ASC reversed ({reversed:?}), got {descending:?}"
        );
    }
}

/// A walk through `nextToken` reports every selected component exactly once, in the page
/// size asked for and in the order asked for.
///
/// The rows differ in page size (every size over the fixture, including one past its
/// length), in sort order and in whether a filter is applied — the invariant is the same
/// for all of them: the concatenated pages are exactly the single-page answer, no page is
/// longer than `maxResults`, and only the page that ends the list carries no token.
#[tokio::test]
async fn pagination_walks_every_selected_component_exactly_once() {
    let router = emulator_with(portfolio());

    let filters = vec![
        ("the whole portfolio", json!({})),
        (
            "the components of two groups",
            json!({
                "groupIdFilter": [
                    group("ExternalId", "wave-1"),
                    group("ExternalSourceType", "MPA"),
                ],
            }),
        ),
        (
            "one component",
            json!({"applicationComponentCriteria": "APP_NAME", "filterValue": "legacy-batch"}),
        ),
        (
            "no component",
            json!({"applicationComponentCriteria": "STRATEGY", "filterValue": "Retirement"}),
        ),
    ];

    for (filtered, filter) in filters {
        for sort in ["ASC", "DESC"] {
            let mut whole = filter.clone();
            whole["sort"] = json!(sort);
            let expected = reported_ids(&list_components(&router, whole.clone()).await);

            for size in 1..=(expected.len() + 1).max(2) {
                let case = format!("{filtered} sorted {sort} in pages of {size}");
                let mut walked: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;
                loop {
                    let mut request = whole.clone();
                    request["maxResults"] = json!(size);
                    if let Some(token) = &token {
                        request["nextToken"] = json!(token);
                    }
                    let answer = list_components(&router, request).await;
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected HTTP 200 on page {pages}, got {} with {:?}",
                        answer.status, answer.body
                    );
                    let page = reported_ids(&answer);
                    assert!(
                        page.len() <= size,
                        "case {case:?}: a page holds at most {size} components, got {page:?}"
                    );
                    walked.extend(page);
                    pages += 1;
                    assert!(
                        pages <= expected.len() + 2,
                        "case {case:?}: the walk did not end after {pages} pages ({walked:?})"
                    );
                    match at(&answer.body, "/nextToken") {
                        Value::Null => break,
                        Value::String(next) => {
                            assert!(
                                !next.trim().is_empty() && next.len() <= 2048,
                                "case {case:?}: a nextToken is a non-blank NextToken within \
                                 2048 characters, got {next:?}"
                            );
                            token = Some(next);
                        }
                        other => panic!("case {case:?}: nextToken must be a string, got {other:?}"),
                    }
                }
                assert_eq!(
                    walked, expected,
                    "case {case:?}: the walk must report {expected:?} exactly once each, got \
                     {walked:?}"
                );
            }
        }
    }
}

/// A `ListApplicationComponents` request the model admits member by member and the
/// operation still cannot act on.
struct Refusal {
    what: &'static str,
    request: Value,
}

/// Every refusal: 400 `ValidationException`, and nothing reported.
#[tokio::test]
async fn a_request_the_operation_cannot_act_on_is_refused() {
    let router = emulator_with(portfolio());

    let mut cases: Vec<Refusal> = Vec::new();
    // A criteria naming a member has nothing to compare it to without a filterValue —
    // every criteria of the catalog except the one that names no member.
    for criteria in APPLICATION_COMPONENT_CRITERIA {
        if *criteria == "NOT_DEFINED" {
            continue;
        }
        cases.push(Refusal {
            what: "an applicationComponentCriteria naming a member, with no filterValue",
            request: json!({"applicationComponentCriteria": criteria}),
        });
    }
    cases.extend(vec![
        Refusal {
            what: "an applicationComponentCriteria the enumeration does not declare",
            request: json!({
                "applicationComponentCriteria": "APP_VERSION",
                "filterValue": "Java",
            }),
        },
        Refusal {
            what: "an applicationComponentCriteria that is a value of the neighbouring \
                   criteria shape",
            request: json!({"applicationComponentCriteria": "OS_NAME", "filterValue": "LINUX"}),
        },
        Refusal {
            what: "a sort SortOrder does not declare",
            request: json!({"sort": "ascending"}),
        },
        Refusal {
            what: "a groupIdFilter name GroupName does not declare",
            request: json!({"groupIdFilter": [{"name": "ImportName", "value": "wave-1"}]}),
        },
        Refusal {
            what: "maxResults of 0, a page of nothing",
            request: json!({"maxResults": 0}),
        },
        Refusal {
            what: "a negative maxResults",
            request: json!({"maxResults": -1}),
        },
        Refusal {
            what: "maxResults of 101, one past the documented maximum of 100",
            request: json!({"maxResults": MAX_RESULTS_MAX + 1}),
        },
        Refusal {
            what: "maxResults far past the documented maximum",
            request: json!({"maxResults": 1000}),
        },
        Refusal {
            what: "a nextToken that is not even an encoding this service issues",
            request: json!({"nextToken": "not-a-token-this-service-issued"}),
        },
        Refusal {
            what: "a nextToken that decodes but carries no marker of ours",
            request: json!({"nextToken": "deadbeef"}),
        },
    ]);

    for Refusal { what, request } in cases {
        let answer = list_components(&router, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400 for {request}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException for {request}, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/applicationComponentInfos"),
            Value::Null,
            "case {what:?}: a refused request reports no component, got {:?}",
            answer.body
        );
    }
}

/// A token is only good for the list it was issued for: handed back under a different
/// selection it is refused, rather than resuming at whatever position that component
/// happens to have in some other list. Reordering the groups of one filter is the SAME
/// selection, and the page size is per call, so neither invalidates a token. A token one of
/// this service's OTHER lists issued is foreign here.
#[tokio::test]
async fn a_token_is_only_good_for_the_list_it_was_issued_for() {
    let router = emulator_with(portfolio());

    let first = list_components(&router, json!({"maxResults": 1})).await;
    let token = first.string("/nextToken");
    assert!(
        !token.is_empty(),
        "the first page of six components must carry a nextToken, got {:?}",
        first.body
    );

    // Each row is the same token under a selection it was not issued for.
    let elsewhere = vec![
        (
            "a criteria the unfiltered list did not have",
            json!({
                "nextToken": token,
                "applicationComponentCriteria": "APP_TYPE",
                "filterValue": "Java",
            }),
        ),
        (
            "a criteria that still selects the component the token names",
            json!({
                "nextToken": token,
                "applicationComponentCriteria": "SERVER_ID",
                "filterValue": "1a",
            }),
        ),
        (
            "the other sort order",
            json!({"nextToken": token, "sort": "DESC"}),
        ),
        (
            "a group filter the unfiltered list did not have",
            json!({"nextToken": token, "groupIdFilter": [group("ExternalId", "wave-1")]}),
        ),
    ];
    for (what, request) in elsewhere {
        let answer = list_components(&router, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: a token handed back under another selection is refused, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // A token the neighbouring list of this very service issued names a position in THAT
    // list, so it is foreign here.
    let servers_token = call(&router, "POST", "/list-servers", Some(json!({"maxResults": 1})))
        .await
        .string("/nextToken");
    if !servers_token.is_empty() {
        let answer = list_components(&router, json!({"nextToken": servers_token})).await;
        assert_eq!(
            answer.status, 400,
            "a ListServers token is foreign to this list, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // The same token on the list it came from still resumes it — with the page size it was
    // issued under, and with another one, which `maxResults`' own documentation makes a
    // per-call choice.
    for size in [1, 2, 100] {
        let resumed =
            list_components(&router, json!({"maxResults": size, "nextToken": token})).await;
        let expected: Vec<String> = all_ids_ascending()
            .into_iter()
            .skip(1)
            .take(size as usize)
            .collect();
        assert_eq!(
            reported_ids(&resumed),
            expected,
            "case {size:?}: the token must resume the list it was issued for, got {:?}",
            resumed.body
        );
    }

    // A group filter whose groups arrive in the other order is the same selection, so a
    // token issued under one is accepted under the other.
    let groups = [
        group("ExternalId", "wave-1"),
        group("ExternalSourceType", "MPA"),
    ];
    let page = list_components(
        &router,
        json!({"maxResults": 1, "groupIdFilter": [groups[0].clone(), groups[1].clone()]}),
    )
    .await;
    let grouped_token = page.string("/nextToken");
    assert!(
        !grouped_token.is_empty(),
        "a first page of four components must carry a nextToken, got {:?}",
        page.body
    );
    let reordered = list_components(
        &router,
        json!({
            "maxResults": 1,
            "nextToken": grouped_token,
            "groupIdFilter": [groups[1].clone(), groups[0].clone()],
        }),
    )
    .await;
    assert_eq!(
        reported_ids(&reordered),
        vec!["0b".to_string()],
        "the same groups in another order select the same components, so the token still \
         resumes, got {:?}",
        reordered.body
    );
}

/// A request at each published bound is accepted: the refusals above answer a genuine
/// violation, not every request.
#[tokio::test]
async fn a_request_at_a_published_bound_is_accepted() {
    let router = emulator_with(portfolio());
    let total = all_ids_ascending().len();

    let accepted = vec![
        (
            "maxResults of 1, at the minimum page",
            json!({"maxResults": 1}),
            1,
        ),
        (
            "maxResults of 100, AT the documented maximum",
            json!({"maxResults": MAX_RESULTS_MAX}),
            total,
        ),
        (
            "no maxResults, which takes the documented maximum",
            json!({}),
            total,
        ),
        (
            "NOT_DEFINED with no filterValue, which needs none",
            json!({"applicationComponentCriteria": "NOT_DEFINED"}),
            total,
        ),
        (
            "a filterValue with no criteria to apply it to",
            json!({"filterValue": "Java"}),
            total,
        ),
        (
            // `filterValue` is `String` under `@length(max 256)`
            // (`smithy_query(query="shape",shape="ListApplicationComponentsRequest")`).
            "a filterValue AT the 256-character maximum",
            json!({
                "applicationComponentCriteria": "APP_NAME",
                "filterValue": "x".repeat(256),
            }),
            0,
        ),
    ];

    for (what, request, expected) in accepted {
        let answer = list_components(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: {request} was refused with {:?}",
            answer.body
        );
        assert_eq!(
            reported_ids(&answer).len(),
            expected,
            "case {what:?}: expected {expected} component(s), got {:?}",
            answer.body
        );
    }
}

/// Every reported entry is the stored record, member for member: the entry this list
/// reports for a component is exactly the `applicationComponentDetail`
/// `GetApplicationComponentDetails` reports for that same component, so the two reads
/// cannot drift apart and nothing in the list is derived from the request that read it.
#[tokio::test]
async fn a_reported_component_is_the_stored_record() {
    let router = emulator_with(portfolio());

    let answer = list_components(&router, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "ListApplicationComponents over a seeded portfolio answered {} with {:?}",
        answer.status, answer.body
    );
    let listed = answer
        .body
        .get("applicationComponentInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        portfolio().len(),
        "the list must report every seeded component, got {:?}",
        answer.body
    );

    for entry in &listed {
        let id = entry["id"].as_str().unwrap_or_default().to_string();
        let detail = get_component(&router, &id).await;
        assert_eq!(
            detail.status, 200,
            "case {id:?}: the component must still read back one at a time, got {} with {:?}",
            detail.status, detail.body
        );
        assert_eq!(
            entry,
            &at(&detail.body, "/applicationComponentDetail"),
            "case {id:?}: the listed entry must be the detail the single read reports, got \
             {entry:?} against {:?}",
            at(&detail.body, "/applicationComponentDetail")
        );
    }

    // And a component the ingestion knew nothing else about reports nothing but its id: no
    // structure nothing filled in.
    let bare = emulator_with(vec![component("0a")]);
    let answer = list_components(&bare, json!({})).await;
    for member in [
        "name",
        "appType",
        "resourceSubType",
        "inclusionStatus",
        "analysisStatus",
        "statusMessage",
        "recommendationSet",
        "appUnitError",
        "databaseConfigDetail",
        "sourceCodeRepositories",
        "osVersion",
        "osDriver",
        "lastAnalyzedTimestamp",
        "associatedServerId",
        "runtimeStatus",
        "runtimeStatusMessage",
        "listAntipatternSeveritySummary",
        "antipatternReportS3Object",
        "antipatternReportStatus",
        "resultList",
    ] {
        let pointer = format!("/applicationComponentInfos/0/{member}");
        assert_eq!(
            at(&answer.body, &pointer),
            Value::Null,
            "case {member:?}: a record holding no {member} reports none, got {:?}",
            at(&answer.body, &pointer)
        );
    }
    assert_eq!(
        at(&answer.body, "/applicationComponentInfos/0/id"),
        json!("0a"),
        "the record still reports its id, got {:?}",
        answer.body
    );
}

/// An account whose data has not been brought in holds no application component: a 200
/// with no component and no token, for every filter — not a refusal, and not a canned
/// component.
#[tokio::test]
async fn an_account_with_no_component_reports_an_empty_list() {
    let router = emulator_with(Vec::new());

    let probes = vec![
        ("no filter at all", json!({})),
        (
            "a criteria and a value",
            json!({"applicationComponentCriteria": "APP_TYPE", "filterValue": "Java"}),
        ),
        (
            "a group filter",
            json!({"groupIdFilter": [group("ExternalId", "wave-1")]}),
        ),
        ("a page size", json!({"maxResults": 10, "sort": "DESC"})),
    ];

    for (what, request) in probes {
        let answer = list_components(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200 for {request}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(&answer.body, "/applicationComponentInfos"),
            json!([]),
            "case {what:?}: an account holding no component reports none, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/nextToken"),
            Value::Null,
            "case {what:?}: there is no further page to resume, got {:?}",
            answer.body
        );
    }
}

/// Reading the list does not change it: the same request answers the same thing after a
/// walk through every page, and nothing a read did is visible to the next one.
#[tokio::test]
async fn reading_the_list_changes_nothing() {
    let router = emulator_with(portfolio());

    let before = list_components(&router, json!({})).await;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"maxResults": 2});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = list_components(&router, request).await;
        match at(&page.body, "/nextToken") {
            Value::String(next) => token = Some(next),
            _ => break,
        }
    }
    let after = list_components(&router, json!({})).await;

    assert_eq!(
        before.body, after.body,
        "a read must not change what a later read sees: {:?} then {:?}",
        before.body, after.body
    );
}
