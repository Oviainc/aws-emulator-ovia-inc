// SPDX-License-Identifier: CC0-1.0

//! `ListAnalyzableServers`: the servers a data collector fetched, read end to end over
//! HTTP.
//!
//! The rule under test is the one the operation states for ANY account, not for one
//! request: the answer is an `AnalyzableServerSummary` for each server of this service's
//! inventory that it holds collector discovery data for — member for member as the
//! discovery stored it, and nothing for a member it did not carry — a server it holds no
//! discovery data for reported not at all, the list ordered over the servers' one total
//! order (their inventory id) in the direction `sort` asks for, and handed out in pages of
//! at most `maxResults` that a `nextToken` resumes; and a request the model admits member
//! by member but that the operation cannot act on (a `maxResults` past the documented
//! maximum, a `SortOrder` value the model does not declare, a token this list never
//! issued) is refused with 400 `ValidationException`.
//!
//! Each table below therefore spans the class the rule generalises over: which discovery
//! members a record carries (all four, each one alone, none at all), the form of their
//! values (a `DataSourceType` entry and free text, IPv4 and IPv6), how many servers are
//! analyzable (none, one, several), every page size over the fixture and both sort orders
//! — rather than one list of one shape.
//!
//! A server enters this service only through the data the account brings in (an import
//! file task's ingestion, filed separately), so these cases seed the inventory records
//! directly through the shared engine — the same `servers` map that ingestion writes and
//! this operation reads, through the harness `server_inventory.rs` already uses — and then
//! drive every assertion through the router, so the handler, its refusals and the
//! serialized `AnalyzableServerSummary` are exercised exactly as a client reaches them.

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ServerDiscovery, ServerRecord,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer};
use crate::server_inventory::{emulator_with, server};

/// Every value `DataSourceType` admits, from
/// `smithy_query(query="shape",shape="DataSourceType")`. `AnalyzableServerSummary$source`
/// targets `String` rather than this enumeration ("The data source of the analyzable
/// server"), so the fixture carries both an entry of this catalog and free text, and the
/// operation must report either exactly as stored.
const DATA_SOURCE_TYPES: &[&str] = &[
    "ApplicationDiscoveryService",
    "MPA",
    "Import",
    "StrategyRecommendationsApplicationDataCollector",
];

/// The members `AnalyzableServerSummary` declares, from
/// `smithy_query(query="shape",shape="AnalyzableServerSummary")`. A reported summary
/// carries no key outside this set.
const SUMMARY_MEMBERS: &[&str] = &["hostname", "ipAddress", "source", "vmId"];

/// The documented maximum of `maxResults`: "The maximum number of items to include in the
/// response. The maximum value is 100." (`smithy_query(query="operation",operation="ListAnalyzableServers")`
/// -> `maxResults`), spelled out independently of the crate's constant.
const MAX_RESULTS_MAX: i64 = 100;

async fn list_analyzable_servers(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/list-analyzable-servers", Some(request)).await
}

/// The summaries `analyzableServers` reported, in the order they were reported.
fn reported(answer: &Answer) -> Vec<Value> {
    match answer.body.get("analyzableServers").and_then(Value::as_array) {
        Some(items) => items.clone(),
        None => Vec::new(),
    }
}

/// One server of the fixture: the record seeded into the inventory and the summary — if
/// any — the operation must report for it.
struct Case {
    what: &'static str,
    record: ServerRecord,
    expected: Option<Value>,
}

/// The inventory every case runs against, spanning which discovery members a record
/// carries and the form of their values.
///
/// Ids are `ResourceId`s and are deliberately NOT listed in ascending order, so nothing
/// can pass by accident of insertion order. Two records carry portfolio members beside
/// their discovery data (a name, an OS, a recommended strategy), which the summary must
/// NOT report: `AnalyzableServerSummary` declares four members and none of them is one of
/// those.
fn fixture() -> Vec<Case> {
    vec![
        Case {
            what: "all four discovery members, the source a DataSourceType entry",
            record: ServerRecord {
                discovery: Some(ServerDiscovery {
                    hostname: Some("vcenter-web-01".to_string()),
                    ip_address: Some("10.0.3.17".to_string()),
                    source: Some(DATA_SOURCE_TYPES[3].to_string()),
                    vm_id: Some("vm-1042".to_string()),
                }),
                name: Some("web-01".to_string()),
                os_type: Some("WINDOWS".to_string()),
                strategy: Some("Rehost".to_string()),
                ..server("1a")
            },
            expected: Some(json!({
                "hostname": "vcenter-web-01",
                "ipAddress": "10.0.3.17",
                "source": DATA_SOURCE_TYPES[3],
                "vmId": "vm-1042",
            })),
        },
        Case {
            what: "no discovery data at all: a server no collector fetched",
            record: ServerRecord {
                name: Some("imported-only".to_string()),
                os_type: Some("LINUX".to_string()),
                ..server("0a")
            },
            expected: None,
        },
        Case {
            what: "a virtual machine id alone",
            record: ServerRecord {
                discovery: Some(ServerDiscovery {
                    vm_id: Some("vm-7".to_string()),
                    ..ServerDiscovery::default()
                }),
                ..server("2b")
            },
            expected: Some(json!({"vmId": "vm-7"})),
        },
        Case {
            what: "a host name and an IPv6 address, which the member's String admits",
            record: ServerRecord {
                discovery: Some(ServerDiscovery {
                    hostname: Some("db-01.corp.example".to_string()),
                    ip_address: Some("2001:db8::8a2e:370:7334".to_string()),
                    ..ServerDiscovery::default()
                }),
                name: Some("db-01".to_string()),
                ..server("0b")
            },
            expected: Some(json!({
                "hostname": "db-01.corp.example",
                "ipAddress": "2001:db8::8a2e:370:7334",
            })),
        },
        Case {
            what: "a free-text source, which the member is not held to a catalog for",
            record: ServerRecord {
                discovery: Some(ServerDiscovery {
                    source: Some("customer vCenter 7.0".to_string()),
                    hostname: Some("app-01".to_string()),
                    ..ServerDiscovery::default()
                }),
                ..server("1b")
            },
            expected: Some(json!({"source": "customer vCenter 7.0", "hostname": "app-01"})),
        },
        Case {
            what: "discovery data holding nothing: fetched, and nothing known about it",
            record: ServerRecord {
                discovery: Some(ServerDiscovery::default()),
                ..server("2a")
            },
            expected: Some(json!({})),
        },
        Case {
            what: "no discovery data, a fully assessed portfolio server",
            record: ServerRecord {
                data_collection_status: Some("dataCollectionTaskSuccess".to_string()),
                target_destination: Some("AWS Fargate".to_string()),
                ..server("3a")
            },
            expected: None,
        },
    ]
}

/// The fixture's records, for seeding.
fn inventory() -> Vec<ServerRecord> {
    fixture().into_iter().map(|case| case.record).collect()
}

/// The summaries the whole fixture must report, in ascending inventory-id order.
fn expected_ascending() -> Vec<Value> {
    let mut rows: Vec<(String, Value)> = fixture()
        .into_iter()
        .filter_map(|case| {
            case.expected
                .map(|summary| (case.record.id.clone(), summary))
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows.into_iter().map(|(_, summary)| summary).collect()
}

/// Every server the service holds discovery data for is reported, member for member as the
/// discovery stored it; every server it holds none for is not reported at all.
#[tokio::test]
async fn every_discovered_server_is_reported_member_for_member() {
    let router = emulator_with(inventory());
    let answer = list_analyzable_servers(&router, json!({})).await;

    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from an unfiltered call, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        reported(&answer),
        expected_ascending(),
        "the list must report every discovered server's summary in ascending id order, got \
         {:?}",
        answer.body
    );
    assert_eq!(
        at(&answer.body, "/nextToken"),
        Value::Null,
        "a page that ended the list carries no nextToken, got {:?}",
        at(&answer.body, "/nextToken")
    );

    // And no summary carries a member the shape does not declare — in particular none of
    // the portfolio members two of the fixture's records hold beside their discovery data.
    for summary in reported(&answer) {
        let object = summary
            .as_object()
            .unwrap_or_else(|| panic!("a summary must be an object, got {summary:?}"));
        for key in object.keys() {
            assert!(
                SUMMARY_MEMBERS.contains(&key.as_str()),
                "case {summary:?}: AnalyzableServerSummary declares no member {key:?}"
            );
        }
    }
}

/// Each fixture row on its own: the one-server account reports exactly that row's summary,
/// or nothing at all when the service holds no discovery data for it.
#[tokio::test]
async fn one_server_reports_exactly_what_its_record_holds() {
    for Case {
        what,
        record,
        expected,
    } in fixture()
    {
        let router = emulator_with(vec![record]);
        let answer = list_analyzable_servers(&router, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let wanted: Vec<Value> = expected.clone().into_iter().collect();
        assert_eq!(
            reported(&answer),
            wanted,
            "case {what:?}: the list must report {wanted:?}, got {:?}",
            answer.body
        );
    }
}

/// An account whose inventory holds no server the service discovered reports an empty
/// list, and no token — never a summary nothing holds.
#[tokio::test]
async fn an_account_with_nothing_discovered_reports_an_empty_list() {
    let probes: Vec<(&str, Vec<ServerRecord>)> = vec![
        ("an empty inventory", Vec::new()),
        (
            "an inventory of servers no collector fetched",
            vec![server("0a"), server("1a")],
        ),
    ];

    for (what, servers) in probes {
        let router = emulator_with(servers);
        let answer = list_analyzable_servers(&router, json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer),
            Vec::<Value>::new(),
            "case {what:?}: the list must report nothing, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/nextToken"),
            Value::Null,
            "case {what:?}: a list that ended carries no nextToken, got {:?}",
            answer.body
        );
    }
}

/// `sort` reverses the order over the servers' one total order, their inventory id: `DESC`
/// is `ASC` read backwards, and an absent `sort` is `ASC`.
#[tokio::test]
async fn sort_order_reverses_the_reported_order() {
    let router = emulator_with(inventory());

    let implied = reported(&list_analyzable_servers(&router, json!({})).await);
    let ascending = reported(&list_analyzable_servers(&router, json!({"sort": "ASC"})).await);
    let descending = reported(&list_analyzable_servers(&router, json!({"sort": "DESC"})).await);

    assert_eq!(
        implied, ascending,
        "a request naming no sort is ASC, got {implied:?} against {ascending:?}"
    );
    assert_eq!(
        ascending,
        expected_ascending(),
        "ASC reports the discovered servers in ascending id order, got {ascending:?}"
    );
    let reversed: Vec<Value> = ascending.iter().rev().cloned().collect();
    assert_eq!(
        descending, reversed,
        "DESC is ASC reversed ({reversed:?}), got {descending:?}"
    );
}

/// A walk through `nextToken` reports every analyzable server exactly once, in the page
/// size asked for and in the order asked for.
///
/// The rows differ in how many servers are analyzable (the whole fixture, one, none), in
/// page size (every size over the fixture, including one past its length) and in sort
/// order — the invariant is the same for all of them: the concatenated pages are exactly
/// the single-page answer, no page is longer than `maxResults`, and only the page that ends
/// the list carries no token.
#[tokio::test]
async fn pagination_walks_every_analyzable_server_exactly_once() {
    let accounts: Vec<(&str, Vec<ServerRecord>)> = vec![
        ("the whole fixture", inventory()),
        (
            "one analyzable server",
            vec![
                ServerRecord {
                    discovery: Some(ServerDiscovery {
                        vm_id: Some("vm-1".to_string()),
                        ..ServerDiscovery::default()
                    }),
                    ..server("0a")
                },
                server("1a"),
            ],
        ),
        ("no analyzable server", vec![server("0a")]),
    ];

    for (account, servers) in accounts {
        let router = emulator_with(servers);
        for sort in ["ASC", "DESC"] {
            let whole = json!({"sort": sort});
            let expected = reported(&list_analyzable_servers(&router, whole.clone()).await);

            for size in 1..=(expected.len() + 1).max(2) {
                let case = format!("{account} sorted {sort} in pages of {size}");
                let mut walked: Vec<Value> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;
                loop {
                    let mut request = whole.clone();
                    request["maxResults"] = json!(size);
                    if let Some(token) = &token {
                        request["nextToken"] = json!(token);
                    }
                    let answer = list_analyzable_servers(&router, request).await;
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected HTTP 200 on page {pages}, got {} with {:?}",
                        answer.status, answer.body
                    );
                    let page = reported(&answer);
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
                                "case {case:?}: a nextToken is a non-blank NextToken within 2048 \
                                 characters, got {next:?}"
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

/// Every request the operation cannot act on: 400 `ValidationException`, and nothing
/// reported.
#[tokio::test]
async fn a_request_the_operation_cannot_act_on_is_refused() {
    let router = emulator_with(inventory());

    let cases: Vec<(&str, Value)> = vec![
        ("maxResults of 0, a page of nothing", json!({"maxResults": 0})),
        ("a negative maxResults", json!({"maxResults": -1})),
        (
            "maxResults of 101, one past the documented maximum of 100",
            json!({"maxResults": MAX_RESULTS_MAX + 1}),
        ),
        (
            "maxResults far past the documented maximum",
            json!({"maxResults": 1000}),
        ),
        ("a sort SortOrder does not declare", json!({"sort": "ascending"})),
        (
            "a sort that is the right value in the wrong case",
            json!({"sort": "asc"}),
        ),
        (
            "a nextToken that is not even an encoding this service issues",
            json!({"nextToken": "not-a-token-this-service-issued"}),
        ),
        (
            "a nextToken that decodes but carries no marker of ours",
            json!({"nextToken": "deadbeef"}),
        ),
    ];

    for (what, request) in cases {
        let answer = list_analyzable_servers(&router, request.clone()).await;
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
            at(&answer.body, "/analyzableServers"),
            Value::Null,
            "case {what:?}: a refused request reports no server, got {:?}",
            answer.body
        );
    }
}

/// A token is only good for the list it was issued for: handed back under the other sort
/// order, or to the neighbouring list, it is refused rather than resuming at whatever
/// position its server has there. The page size is per call, so changing it does not
/// invalidate a token.
#[tokio::test]
async fn a_token_is_only_good_for_the_list_it_was_issued_for() {
    let router = emulator_with(inventory());

    let first = list_analyzable_servers(&router, json!({"maxResults": 1, "sort": "ASC"})).await;
    let token = first.string("/nextToken");
    assert!(
        !token.is_empty(),
        "the first page of several discovered servers must carry a nextToken, got {:?}",
        first.body
    );

    let elsewhere: Vec<(&str, Value)> = vec![
        (
            "the other sort order",
            json!({"maxResults": 1, "sort": "DESC", "nextToken": token.clone()}),
        ),
        (
            "an implied ASC it was issued for, which is the SAME list",
            json!({"maxResults": 1, "nextToken": token.clone()}),
        ),
        (
            "another page size, which is per call",
            json!({"maxResults": 2, "sort": "ASC", "nextToken": token.clone()}),
        ),
    ];
    let same_list = ["an implied ASC it was issued for, which is the SAME list",
        "another page size, which is per call"];

    for (what, request) in elsewhere {
        let answer = list_analyzable_servers(&router, request.clone()).await;
        if same_list.contains(&what) {
            assert_eq!(
                answer.status, 200,
                "case {what:?}: {request} resumes the list the token was issued for, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            continue;
        }
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
    }

    // A `ListServers` token names a position in another list, so it is foreign here — and
    // one of ours is foreign there.
    let server_page = call(
        &router,
        "POST",
        "/list-servers",
        Some(json!({"maxResults": 1})),
    )
    .await;
    let foreign = server_page.string("/nextToken");
    assert!(
        !foreign.is_empty(),
        "the ListServers fixture must page, got {:?}",
        server_page.body
    );
    let answer = list_analyzable_servers(&router, json!({"nextToken": foreign})).await;
    assert_eq!(
        answer.status, 400,
        "a ListServers token is foreign to this list, got {} with {:?}",
        answer.status, answer.body
    );
    let answer = call(
        &router,
        "POST",
        "/list-servers",
        Some(json!({"nextToken": token})),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a ListAnalyzableServers token is foreign to ListServers, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The read reports and does not mutate: the same call answers the same thing twice, and
/// the inventory the neighbouring read reports is the same after it as before.
#[tokio::test]
async fn reading_the_list_changes_nothing_a_later_caller_sees() {
    let router = emulator_with(inventory());

    let before = call(&router, "POST", "/list-servers", Some(json!({}))).await;
    let first = list_analyzable_servers(&router, json!({})).await;
    // A paged walk too, in case taking a page were to consume anything.
    for size in [1, 2] {
        let _ = list_analyzable_servers(&router, json!({"maxResults": size})).await;
    }
    let second = list_analyzable_servers(&router, json!({})).await;
    let after = call(&router, "POST", "/list-servers", Some(json!({}))).await;

    assert_eq!(
        reported(&second),
        reported(&first),
        "two identical calls must answer the same thing: {:?} then {:?}",
        first.body,
        second.body
    );
    assert_eq!(
        after.body, before.body,
        "reading the analyzable servers must not change the inventory a later caller sees: \
         {:?} then {:?}",
        before.body, after.body
    );
}
