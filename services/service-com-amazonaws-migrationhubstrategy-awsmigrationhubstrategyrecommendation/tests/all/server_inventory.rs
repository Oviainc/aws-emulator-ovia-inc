// SPDX-License-Identifier: CC0-1.0

//! `ListServers`: the account's server inventory, read end to end over HTTP.
//!
//! The rule under test is the one `ListServers` states for ANY inventory, not for one
//! request: the answer is the servers this service holds, scoped by the import groups
//! `groupIdFilter` names, selected by the member the `serverCriteria` names compared to
//! `filterValue`, ordered over the servers' one total order (their id) in the direction
//! `sort` asks for, and handed out in pages of at most `maxResults` that a `nextToken`
//! resumes — and a request the model admits member by member but that the operation
//! cannot act on (a criteria naming a member with no value to compare it to, a
//! `maxResults` past the documented maximum, an enum value the model does not declare, a
//! token this service never issued) is refused with 400 `ValidationException`.
//!
//! Each table below therefore spans the class the rule generalises over — every
//! `ServerCriteria` value, several values per criteria, one group and several, every page
//! size over the fixture and both sort orders — rather than the one filtered list the
//! issue cited.
//!
//! A server enters this service only through an import file task's ingestion of the
//! import file, which is filed separately (the import-loop issue: `StartImportFileTask`
//! reaches `ImportSuccess` having ingested nothing). So these cases seed the inventory
//! records directly through the shared engine — the same `servers` map that ingestion
//! writes and this operation reads — and then drive every assertion through the router,
//! so the handler, its refusals and the serialized `ServerDetail` are exercised exactly
//! as a client reaches them.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ServerRecord,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer};

/// Every value `ServerCriteria` admits, from
/// `smithy_query(query="shape",shape="ServerCriteria")`. Spelled out here so the table
/// below can be checked to cover the whole catalog rather than the one criteria the issue
/// cited.
const SERVER_CRITERIA: &[&str] = &[
    "ANALYSIS_STATUS",
    "DESTINATION",
    "ERROR_CATEGORY",
    "NOT_DEFINED",
    "OS_NAME",
    "SERVER_ID",
    "STRATEGY",
];

/// Every value `GroupName` admits, from `smithy_query(query="shape",shape="GroupName")`.
const GROUP_NAMES: &[&str] = &["ExternalId", "ExternalSourceType"];

/// The documented maximum of `maxResults`: "The maximum number of items to include in the
/// response. The maximum value is 100." (`smithy_query(query="operation",operation="ListServers")`
/// -> `maxResults`), spelled out independently of the crate's constant.
const MAX_RESULTS_MAX: i64 = 100;

/// One `groupIdFilter` entry.
fn group(name: &str, value: &str) -> Value {
    assert!(
        GROUP_NAMES.contains(&name),
        "a group name GroupName does not declare: {name:?}"
    );
    json!({"name": name, "value": value})
}

/// A server record for the inventory, with only its id set.
pub(crate) fn server(id: &str) -> ServerRecord {
    assert!(
        !id.is_empty()
            && id.len() <= 44
            && id.chars().all(|c| c.is_ascii_digit() || c == 'a' || c == 'b'),
        "a server id ResourceId (^[0-9a-b]+, max 44) does not admit: {id:?}"
    );
    ServerRecord {
        id: id.to_string(),
        ..Default::default()
    }
}

/// The inventory every selection case runs against: six servers differing in OS, in
/// recommended strategy and destination, in data-collection status, in whether an error
/// category was recorded, and in how many import groups they came in with (none, one,
/// two).
///
/// Values come from the model's own catalogs
/// (`smithy_query(query="shape",shape="OSType,Strategy,TargetDestination,RunTimeAssessmentStatus,ServerErrorCategory")`).
/// Ids are `ResourceId`s and are deliberately NOT inserted in order, so nothing can pass
/// by accident of insertion order.
fn inventory() -> Vec<ServerRecord> {
    vec![
        ServerRecord {
            os_type: Some("LINUX".to_string()),
            os_version: Some("Ubuntu 20.04".to_string()),
            name: Some("app-01".to_string()),
            strategy: Some("Refactor".to_string()),
            target_destination: Some("Amazon Elastic Container Service (ECS)".to_string()),
            data_collection_status: Some("dataCollectionTaskStarted".to_string()),
            group_id: vec![external_id("wave-2")],
            ..server("1a")
        },
        ServerRecord {
            os_type: Some("WINDOWS".to_string()),
            os_version: Some("WindowsServer 2019".to_string()),
            name: Some("web-01".to_string()),
            strategy: Some("Rehost".to_string()),
            target_destination: Some("Amazon Elastic Cloud Compute (EC2)".to_string()),
            data_collection_status: Some("dataCollectionTaskSuccess".to_string()),
            group_id: vec![external_id("wave-1")],
            ..server("0a")
        },
        ServerRecord {
            os_type: Some("LINUX".to_string()),
            os_version: Some("RHEL 8".to_string()),
            name: Some("mainframe-01".to_string()),
            strategy: Some("Retain".to_string()),
            target_destination: Some("None specified".to_string()),
            data_collection_status: Some("dataCollectionTaskPartialSuccess".to_string()),
            error_category: Some("PERMISSION_ERROR".to_string()),
            group_id: Vec::new(),
            ..server("2b")
        },
        ServerRecord {
            os_type: Some("WINDOWS".to_string()),
            os_version: Some("WindowsServer 2019".to_string()),
            name: Some("web-02".to_string()),
            strategy: Some("Replatform".to_string()),
            target_destination: Some("AWS Fargate".to_string()),
            data_collection_status: Some("dataCollectionTaskSuccess".to_string()),
            group_id: vec![external_id("wave-1")],
            ..server("0b")
        },
        ServerRecord {
            os_type: Some("WINDOWS".to_string()),
            os_version: Some("WindowsServer 2012".to_string()),
            name: Some("legacy-01".to_string()),
            data_collection_status: Some("dataCollectionTaskToBeScheduled".to_string()),
            group_id: vec![external_id("wave-2"), source_type("MPA")],
            ..server("2a")
        },
        ServerRecord {
            os_type: Some("LINUX".to_string()),
            os_version: Some("Amazon Linux 2".to_string()),
            name: Some("db-01".to_string()),
            strategy: Some("Relocate".to_string()),
            target_destination: Some("Aurora PostgreSQL".to_string()),
            data_collection_status: Some("dataCollectionTaskFailed".to_string()),
            error_category: Some("CONNECTIVITY_ERROR".to_string()),
            group_id: vec![source_type("MPA")],
            ..server("1b")
        },
    ]
}

fn external_id(
    value: &str,
) -> service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group
{
    stored_group("ExternalId", value)
}

fn source_type(
    value: &str,
) -> service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group
{
    stored_group("ExternalSourceType", value)
}

fn stored_group(
    name: &str,
    value: &str,
) -> service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group
{
    service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group {
        name: Some(name.to_string()),
        value: Some(value.to_string()),
    }
}

/// Every id in [`inventory`], in the ascending order the service reports them in.
fn all_ids_ascending() -> Vec<String> {
    let mut ids: Vec<String> = inventory().into_iter().map(|server| server.id).collect();
    ids.sort();
    ids
}

/// One emulator over one shared engine, with `servers` seeded into this service's store —
/// the same map an import's ingestion writes and `ListServers` reads.
///
/// `pub(crate)` so the suites beside this one seed the same inventory through the same
/// harness rather than standing up a second copy of it.
pub(crate) fn emulator_with(servers: Vec<ServerRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        for record in servers {
            store.servers.insert(record.id.clone(), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn list_servers(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/list-servers", Some(request)).await
}

/// The ids `serverInfos` reported, in the order they were reported.
fn reported_ids(answer: &Answer) -> Vec<String> {
    match answer.body.get("serverInfos").and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .map(|server| server["id"].as_str().unwrap_or_default().to_string())
            .collect(),
        None => Vec::new(),
    }
}

/// A `ListServers` request and the ids it must report, in order.
struct Selection {
    what: &'static str,
    request: Value,
    expected: Vec<&'static str>,
}

/// Each `ServerCriteria` value selects on the member it names, compared to `filterValue`
/// for equality; `NOT_DEFINED` names no member, so it selects everything.
///
/// Every criteria the catalog declares gets at least one row, most of them two: a value
/// that selects and a value from the same catalog that nothing in the inventory carries.
fn criteria_cases() -> Vec<Selection> {
    let all = all_ids_ascending();
    let every: Vec<&'static str> = ["0a", "0b", "1a", "1b", "2a", "2b"].to_vec();
    assert_eq!(
        all,
        every.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
        "the fixture's ascending order is not what this table expects"
    );

    vec![
        Selection {
            what: "no serverCriteria at all",
            request: json!({}),
            expected: every.clone(),
        },
        Selection {
            what: "NOT_DEFINED, which names no member to filter on",
            request: json!({"serverCriteria": "NOT_DEFINED"}),
            expected: every.clone(),
        },
        Selection {
            what: "NOT_DEFINED with a filterValue, which it has no member to apply to",
            request: json!({"serverCriteria": "NOT_DEFINED", "filterValue": "Rehost"}),
            expected: every.clone(),
        },
        Selection {
            what: "SERVER_ID on an id the inventory holds",
            request: json!({"serverCriteria": "SERVER_ID", "filterValue": "1a"}),
            expected: vec!["1a"],
        },
        Selection {
            what: "SERVER_ID on an id nothing holds",
            request: json!({"serverCriteria": "SERVER_ID", "filterValue": "0a0a"}),
            expected: Vec::new(),
        },
        Selection {
            what: "OS_NAME on an OSType value, which osInfo reports as the type",
            request: json!({"serverCriteria": "OS_NAME", "filterValue": "LINUX"}),
            expected: vec!["1a", "1b", "2b"],
        },
        Selection {
            what: "OS_NAME on the OS the servers report as their version",
            request: json!({"serverCriteria": "OS_NAME", "filterValue": "WindowsServer 2019"}),
            expected: vec!["0a", "0b"],
        },
        Selection {
            what: "OS_NAME on an OS exactly one server reports",
            request: json!({"serverCriteria": "OS_NAME", "filterValue": "RHEL 8"}),
            expected: vec!["2b"],
        },
        Selection {
            what: "STRATEGY on a recommended strategy",
            request: json!({"serverCriteria": "STRATEGY", "filterValue": "Rehost"}),
            expected: vec!["0a"],
        },
        Selection {
            what: "STRATEGY on a Strategy value nothing was recommended",
            request: json!({"serverCriteria": "STRATEGY", "filterValue": "Retirement"}),
            expected: Vec::new(),
        },
        Selection {
            what: "DESTINATION on a recommended target",
            request: json!({"serverCriteria": "DESTINATION", "filterValue": "AWS Fargate"}),
            expected: vec!["0b"],
        },
        Selection {
            what: "DESTINATION on TargetDestination's \"None specified\"",
            request: json!({"serverCriteria": "DESTINATION", "filterValue": "None specified"}),
            expected: vec!["2b"],
        },
        Selection {
            what: "ANALYSIS_STATUS on a status two servers carry",
            request: json!({
                "serverCriteria": "ANALYSIS_STATUS",
                "filterValue": "dataCollectionTaskSuccess",
            }),
            expected: vec!["0a", "0b"],
        },
        Selection {
            what: "ANALYSIS_STATUS on a RunTimeAssessmentStatus nothing is in",
            request: json!({
                "serverCriteria": "ANALYSIS_STATUS",
                "filterValue": "dataCollectionTaskStopped",
            }),
            expected: Vec::new(),
        },
        Selection {
            what: "ERROR_CATEGORY on a recorded error category",
            request: json!({
                "serverCriteria": "ERROR_CATEGORY",
                "filterValue": "CONNECTIVITY_ERROR",
            }),
            expected: vec!["1b"],
        },
        Selection {
            what: "ERROR_CATEGORY on a ServerErrorCategory no analysis recorded",
            request: json!({"serverCriteria": "ERROR_CATEGORY", "filterValue": "OTHER_ERROR"}),
            expected: Vec::new(),
        },
    ]
}

/// `groupIdFilter` scopes the list to the servers that came in with one of the groups it
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
            what: "another ExternalId group, one of whose servers is in two groups",
            request: json!({"groupIdFilter": [group("ExternalId", "wave-2")]}),
            expected: vec!["1a", "2a"],
        },
        Selection {
            what: "an ExternalSourceType group, a different GroupName",
            request: json!({"groupIdFilter": [group("ExternalSourceType", "MPA")]}),
            expected: vec!["1b", "2a"],
        },
        Selection {
            what: "two groups of different names, which widens the selection",
            request: json!({
                "groupIdFilter": [
                    group("ExternalId", "wave-1"),
                    group("ExternalSourceType", "MPA"),
                ],
            }),
            expected: vec!["0a", "0b", "1b", "2a"],
        },
        Selection {
            what: "a group value nothing came in with",
            request: json!({"groupIdFilter": [group("ExternalId", "wave-9")]}),
            expected: Vec::new(),
        },
        Selection {
            what: "a group name that is right with a value that is not",
            request: json!({"groupIdFilter": [group("ExternalSourceType", "ExternalId")]}),
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
                "serverCriteria": "OS_NAME",
                "filterValue": "LINUX",
            }),
            expected: vec!["1a"],
        },
        Selection {
            what: "a group filter and a criteria that select disjoint servers",
            request: json!({
                "groupIdFilter": [group("ExternalId", "wave-1")],
                "serverCriteria": "ERROR_CATEGORY",
                "filterValue": "CONNECTIVITY_ERROR",
            }),
            expected: Vec::new(),
        },
    ]
}

/// Every selection row reports exactly the servers its filter selects, in ascending id
/// order.
#[tokio::test]
async fn a_filter_selects_the_servers_the_member_it_names_matches() {
    let router = emulator_with(inventory());

    let mut criteria_covered: Vec<&str> = Vec::new();
    let mut cases = criteria_cases();
    for case in &cases {
        if let Some(criteria) = case.request.get("serverCriteria").and_then(Value::as_str) {
            criteria_covered.push(criteria);
        }
    }
    for criteria in SERVER_CRITERIA {
        assert!(
            criteria_covered.contains(criteria),
            "this table does not cover the ServerCriteria value {criteria:?}"
        );
    }

    cases.extend(group_filter_cases());
    for Selection {
        what,
        request,
        expected,
    } in cases
    {
        let answer = list_servers(&router, request.clone()).await;
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

/// `sort` reverses the order over the servers' one total order, their id: `DESC` is
/// `ASC` read backwards, and an absent `sort` is `ASC`. Checked unfiltered and under a
/// filter, so the order is a property of the answer rather than of the whole inventory.
#[tokio::test]
async fn sort_order_reverses_the_reported_order() {
    let router = emulator_with(inventory());

    let probes = vec![
        ("the whole inventory", json!({})),
        (
            "the servers of one group",
            json!({"groupIdFilter": [group("ExternalId", "wave-2")]}),
        ),
        (
            "the servers one criteria selects",
            json!({"serverCriteria": "OS_NAME", "filterValue": "LINUX"}),
        ),
    ];

    for (what, request) in probes {
        let mut ascending_request = request.clone();
        ascending_request["sort"] = json!("ASC");
        let mut descending_request = request.clone();
        descending_request["sort"] = json!("DESC");

        let implied = reported_ids(&list_servers(&router, request.clone()).await);
        let ascending = reported_ids(&list_servers(&router, ascending_request).await);
        let descending = reported_ids(&list_servers(&router, descending_request).await);

        assert_eq!(
            implied, ascending,
            "case {what:?}: a request naming no sort is ASC, got {implied:?} against \
             {ascending:?}"
        );
        let mut sorted = ascending.clone();
        sorted.sort();
        assert_eq!(
            ascending, sorted,
            "case {what:?}: ASC reports the servers in ascending id order, got {ascending:?}"
        );
        let reversed: Vec<String> = ascending.iter().rev().cloned().collect();
        assert_eq!(
            descending, reversed,
            "case {what:?}: DESC is ASC reversed ({reversed:?}), got {descending:?}"
        );
    }
}

/// A walk through `nextToken` reports every selected server exactly once, in the page
/// size asked for and in the order asked for.
///
/// The rows differ in page size (every size over the fixture, including one past its
/// length), in sort order and in whether a filter is applied — the invariant is the same
/// for all of them: the concatenated pages are exactly the single-page answer, no page is
/// longer than `maxResults`, and only the page that ends the list carries no token.
#[tokio::test]
async fn pagination_walks_every_selected_server_exactly_once() {
    let router = emulator_with(inventory());

    let filters = vec![
        ("the whole inventory", json!({})),
        (
            "the servers of two groups",
            json!({
                "groupIdFilter": [
                    group("ExternalId", "wave-1"),
                    group("ExternalSourceType", "MPA"),
                ],
            }),
        ),
        (
            "one server",
            json!({"serverCriteria": "SERVER_ID", "filterValue": "2a"}),
        ),
        (
            "no server",
            json!({"serverCriteria": "STRATEGY", "filterValue": "Retirement"}),
        ),
    ];

    for (filtered, filter) in filters {
        for sort in ["ASC", "DESC"] {
            let mut whole = filter.clone();
            whole["sort"] = json!(sort);
            let expected = reported_ids(&list_servers(&router, whole.clone()).await);

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
                    let answer = list_servers(&router, request).await;
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected HTTP 200 on page {pages}, got {} with {:?}",
                        answer.status, answer.body
                    );
                    let page = reported_ids(&answer);
                    assert!(
                        page.len() <= size,
                        "case {case:?}: a page holds at most {size} servers, got {page:?}"
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

/// A `ListServers` request the model admits member by member and the operation still
/// cannot act on.
struct Refusal {
    what: &'static str,
    request: Value,
}

/// Every refusal: 400 `ValidationException`, and nothing reported.
#[tokio::test]
async fn a_request_the_operation_cannot_act_on_is_refused() {
    let router = emulator_with(inventory());

    let mut cases: Vec<Refusal> = Vec::new();
    // A criteria naming a member has nothing to compare it to without a filterValue —
    // every criteria of the catalog except the one that names no member.
    for criteria in SERVER_CRITERIA {
        if *criteria == "NOT_DEFINED" {
            continue;
        }
        cases.push(Refusal {
            what: "a serverCriteria naming a member, with no filterValue",
            request: json!({"serverCriteria": criteria}),
        });
    }
    cases.extend(vec![
        Refusal {
            what: "a serverCriteria ServerCriteria does not declare",
            request: json!({"serverCriteria": "OS_VERSION", "filterValue": "RHEL 8"}),
        },
        Refusal {
            what: "a serverCriteria that is a value of the neighbouring criteria shape",
            request: json!({"serverCriteria": "APP_TYPE", "filterValue": "Java"}),
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
        let answer = list_servers(&router, request.clone()).await;
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
            at(&answer.body, "/serverInfos"),
            Value::Null,
            "case {what:?}: a refused request reports no server, got {:?}",
            answer.body
        );
    }
}

/// A token is only good for the list it was issued for: handed back under a different
/// selection it is refused, rather than resuming at whatever position that server happens
/// to have in some other list. Reordering the groups of one filter is the SAME selection,
/// and the page size is per call, so neither invalidates a token.
#[tokio::test]
async fn a_token_is_only_good_for_the_list_it_was_issued_for() {
    let router = emulator_with(inventory());

    let first = list_servers(&router, json!({"maxResults": 1})).await;
    let token = first.string("/nextToken");
    assert!(
        !token.is_empty(),
        "the first page of six servers must carry a nextToken, got {:?}",
        first.body
    );

    // Each row is the same token under a selection it was not issued for.
    let elsewhere = vec![
        (
            "a criteria the unfiltered list did not have",
            json!({"nextToken": token, "serverCriteria": "SERVER_ID", "filterValue": "2b"}),
        ),
        (
            "a criteria that still selects the server the token names",
            json!({"nextToken": token, "serverCriteria": "OS_NAME", "filterValue": "WINDOWS"}),
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
        let answer = list_servers(&router, request.clone()).await;
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

    // The same token on the list it came from still resumes it — with the page size it
    // was issued under, and with another one, which `maxResults`' own documentation makes
    // a per-call choice.
    for size in [1, 2, 100] {
        let resumed = list_servers(&router, json!({"maxResults": size, "nextToken": token})).await;
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
    let groups = [group("ExternalId", "wave-1"), group("ExternalSourceType", "MPA")];
    let page = list_servers(
        &router,
        json!({"maxResults": 1, "groupIdFilter": [groups[0].clone(), groups[1].clone()]}),
    )
    .await;
    let grouped_token = page.string("/nextToken");
    assert!(
        !grouped_token.is_empty(),
        "a first page of four servers must carry a nextToken, got {:?}",
        page.body
    );
    let reordered = list_servers(
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
        "the same groups in another order select the same servers, so the token still \
         resumes, got {:?}",
        reordered.body
    );
}

/// A request at each published bound is accepted: the refusals above answer a genuine
/// violation, not every request.
#[tokio::test]
async fn a_request_at_a_published_bound_is_accepted() {
    let router = emulator_with(inventory());
    let total = all_ids_ascending().len();

    let accepted = vec![
        ("maxResults of 1, at the minimum page", json!({"maxResults": 1}), 1),
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
            json!({"serverCriteria": "NOT_DEFINED"}),
            total,
        ),
        (
            "a filterValue with no serverCriteria to apply it to",
            json!({"filterValue": "Rehost"}),
            total,
        ),
    ];

    for (what, request, expected) in accepted {
        let answer = list_servers(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: {request} was refused with {:?}",
            answer.body
        );
        assert_eq!(
            reported_ids(&answer).len(),
            expected,
            "case {what:?}: expected {expected} server(s), got {:?}",
            answer.body
        );
    }
}

/// Every member of the reported `ServerDetail` is the stored record's own, and a member
/// the record does not hold is absent rather than reported as an empty structure.
#[tokio::test]
async fn a_reported_server_is_the_stored_record() {
    let analyzed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let full = ServerRecord {
        name: Some("web-01".to_string()),
        server_type: Some("VirtualMachine".to_string()),
        os_type: Some("WINDOWS".to_string()),
        os_version: Some("WindowsServer 2019".to_string()),
        file_system_type: Some("NTFS".to_string()),
        cpu_architecture: Some("x86_64".to_string()),
        data_collection_status: Some("dataCollectionTaskFailed".to_string()),
        status_message: Some("Data collection failed for the server.".to_string()),
        strategy: Some("Rehost".to_string()),
        target_destination: Some("Amazon Elastic Cloud Compute (EC2)".to_string()),
        error_category: Some("CREDENTIAL_ERROR".to_string()),
        last_analyzed: Some(analyzed_at),
        group_id: vec![external_id("wave-1")],
        ..server("0a")
    };
    let router = emulator_with(vec![full.clone(), server("1a")]);

    let answer = list_servers(&router, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "ListServers over a seeded inventory answered {} with {:?}",
        answer.status, answer.body
    );

    // Every member of the fully populated record, as `ServerDetail` reports it.
    let mapped: Vec<(&str, Value)> = vec![
        ("/serverInfos/0/id", json!("0a")),
        ("/serverInfos/0/name", json!("web-01")),
        ("/serverInfos/0/serverType", json!("VirtualMachine")),
        ("/serverInfos/0/systemInfo/osInfo/type", json!("WINDOWS")),
        (
            "/serverInfos/0/systemInfo/osInfo/version",
            json!("WindowsServer 2019"),
        ),
        ("/serverInfos/0/systemInfo/fileSystemType", json!("NTFS")),
        ("/serverInfos/0/systemInfo/cpuArchitecture", json!("x86_64")),
        (
            "/serverInfos/0/dataCollectionStatus",
            json!("dataCollectionTaskFailed"),
        ),
        (
            "/serverInfos/0/statusMessage",
            json!("Data collection failed for the server."),
        ),
        ("/serverInfos/0/recommendationSet/strategy", json!("Rehost")),
        (
            "/serverInfos/0/recommendationSet/targetDestination",
            json!("Amazon Elastic Cloud Compute (EC2)"),
        ),
        (
            "/serverInfos/0/serverError/serverErrorCategory",
            json!("CREDENTIAL_ERROR"),
        ),
    ];
    for (pointer, expected) in mapped {
        assert_eq!(
            at(&answer.body, pointer),
            expected,
            "case {pointer:?}: expected the stored {expected:?}, got {:?}",
            at(&answer.body, pointer)
        );
    }
    assert_eq!(
        at(&answer.body, "/serverInfos/0/lastAnalyzedTimestamp")
            .as_f64()
            .unwrap_or_default(),
        1_700_000_000.0,
        "the reported lastAnalyzedTimestamp must be the stored instant, got {:?}",
        at(&answer.body, "/serverInfos/0/lastAnalyzedTimestamp")
    );

    // The record that holds nothing but an id reports nothing but an id: no structure
    // nothing filled in.
    for member in [
        "name",
        "serverType",
        "systemInfo",
        "recommendationSet",
        "serverError",
        "dataCollectionStatus",
        "statusMessage",
        "lastAnalyzedTimestamp",
        "listAntipatternSeveritySummary",
        "antipatternReportS3Object",
        "antipatternReportStatus",
        "applicationComponentStrategySummary",
    ] {
        let pointer = format!("/serverInfos/1/{member}");
        assert_eq!(
            at(&answer.body, &pointer),
            Value::Null,
            "case {member:?}: a record holding no {member} reports none, got {:?}",
            at(&answer.body, &pointer)
        );
    }
    assert_eq!(
        at(&answer.body, "/serverInfos/1/id"),
        json!("1a"),
        "the second record still reports its id, got {:?}",
        answer.body
    );
}

/// An account that has imported nothing has an empty inventory: a 200 with no server and
/// no token, for every filter — not a refusal, and not a canned server.
#[tokio::test]
async fn an_account_with_no_imported_server_reports_an_empty_inventory() {
    let router = emulator_with(Vec::new());

    let probes = vec![
        ("no filter at all", json!({})),
        (
            "a criteria and a value",
            json!({"serverCriteria": "OS_NAME", "filterValue": "LINUX"}),
        ),
        (
            "a group filter",
            json!({"groupIdFilter": [group("ExternalId", "wave-1")]}),
        ),
        ("a page size", json!({"maxResults": 10, "sort": "DESC"})),
    ];

    for (what, request) in probes {
        let answer = list_servers(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200 for {request}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(&answer.body, "/serverInfos"),
            json!([]),
            "case {what:?}: an account that imported nothing reports no server, got {:?}",
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

/// Reading the inventory does not change it: the same request answers the same thing
/// after a walk through every page, and nothing a read did is visible to the next one.
#[tokio::test]
async fn reading_the_inventory_changes_nothing() {
    let router = emulator_with(inventory());

    let before = list_servers(&router, json!({})).await;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"maxResults": 2});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = list_servers(&router, request).await;
        match at(&page.body, "/nextToken") {
            Value::String(next) => token = Some(next),
            _ => break,
        }
    }
    let after = list_servers(&router, json!({})).await;

    assert_eq!(
        reported_ids(&before),
        reported_ids(&after),
        "a read must not change what a later read sees: {:?} then {:?}",
        before.body,
        after.body
    );
    assert_eq!(
        before.body, after.body,
        "a read must not change what a later read sees: {:?} then {:?}",
        before.body, after.body
    );
}
