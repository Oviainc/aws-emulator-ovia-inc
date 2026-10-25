// SPDX-License-Identifier: CC0-1.0

//! `ListCollectors`: the data collectors installed in the account's environment, read end
//! to end over HTTP.
//!
//! The rule under test is the one the operation states for ANY account, not for one
//! request: the answer is a `Collector` for every collector record this service holds in
//! the Region the request was signed for — member for member as the registration recorded
//! it, and nothing for a member it did not carry — taken in the collectors' one total
//! order (their id) and handed out in pages of at most `maxResults` that a `nextToken`
//! resumes exactly once per collector; and a request the model admits member by member but
//! that the operation cannot act on (a `maxResults` outside the documented bound, a token
//! this list never issued) is refused with 400 `ValidationException`.
//!
//! Each table below therefore spans the class the rule generalises over: which members a
//! record carries (all of them, each kind alone, the id alone), the form of their values
//! (both `CollectorHealth` values, IPv4 and IPv6, each kind of remote configuration a
//! `ConfigurationSummary` holds), how many collectors are registered (none, one, several),
//! every page size over the fixture plus each published bound and one past it — rather
//! than one list of one shape.
//!
//! A collector enters this service by registering itself from the environment it runs in
//! (`Collector$registeredTimeStamp` — "Time when the collector registered with the
//! service"), a path outside the modelled API surface, so these cases seed the records
//! directly through the shared engine — the same `collectors` map that registration writes
//! and this operation reads, through the harness its sibling suites use — and then drive
//! every assertion through the router, so the handler, its refusals and the serialized
//! `Collector` are exercised exactly as a client reaches them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::{
    ConfigurationSummary, IPAddressBasedRemoteInfo, PipelineInfo, RemoteSourceCodeAnalysisServerInfo,
    VcenterBasedRemoteInfo, VersionControlInfo,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    CollectorRecord, ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, minimal_import, start_import, Answer, TEST_REGION};

/// Every value `CollectorHealth` admits, from
/// `smithy_query(query="shape",shape="CollectorHealth")`. Spelled out here rather than
/// imported from the crate, so a wrong catalog in `src/` cannot agree with itself.
const COLLECTOR_HEALTH: &[&str] = &["COLLECTOR_HEALTHY", "COLLECTOR_UNHEALTHY"];

/// The members `Collector` declares, from `smithy_query(query="shape",shape="Collector")`.
/// A reported collector carries no key outside this set.
const COLLECTOR_MEMBERS: &[&str] = &[
    "collectorId",
    "ipAddress",
    "hostName",
    "collectorHealth",
    "collectorVersion",
    "registeredTimeStamp",
    "lastActivityTimeStamp",
    "configurationSummary",
];

/// The documented maximum of `maxResults`: "The maximum number of items to include in the
/// response. The maximum value is 100."
/// (`smithy_query(query="operation",operation="ListCollectors")` -> `maxResults`), spelled
/// out independently of the crate's constant.
const MAX_RESULTS_MAX: i64 = 100;

/// This service's one sentence for a value that fails a rule it enforces.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// This service's one sentence for a parameter it cannot resolve.
const INVALID_OR_MISSING_PARAMETERS: &str = "Looks like you provided invalid or missing parameters.";

/// One emulator over one shared engine, with `collectors` seeded into this service's store
/// — the same map a collector's registration writes and `ListCollectors` reads.
fn emulator_with(collectors: Vec<CollectorRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        for record in collectors {
            portfolio.collectors.insert(record.id.clone(), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// `GET /list-collectors`, with `query` appended when it is not empty.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/list-collectors".to_string()
    } else {
        format!("/list-collectors?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// `GET /list-collectors` against this service's endpoint in `region`, signed for that
/// same Region: the host and the SigV4 credential scope are what select a Region, exactly
/// as a client configured for it sends them.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/list-collectors".to_string()
    } else {
        format!("/list-collectors?{query}")
    };
    let request = ::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", format!("migrationhub-strategy.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{region}/migrationhub-strategy/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} in {region} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The collectors `Collectors` reported, in the order they were reported.
fn reported(answer: &Answer) -> Vec<Value> {
    match answer.body.get("Collectors").and_then(Value::as_array) {
        Some(items) => items.clone(),
        None => Vec::new(),
    }
}

/// A collector record with only its id set.
fn collector(id: &str) -> CollectorRecord {
    assert!(
        !id.trim().is_empty() && id.len() <= 1024,
        "a collectorId (.*\\S.*, max 1024) does not admit: {id:?}"
    );
    CollectorRecord {
        id: id.to_string(),
        ..Default::default()
    }
}

/// One collector of the fixture: the record seeded into the store and the `Collector` the
/// operation must report for it.
struct Case {
    what: &'static str,
    record: CollectorRecord,
    expected: Value,
}

/// The collectors every case runs against, spanning which members a record carries and the
/// form of their values.
///
/// Ids are deliberately NOT listed in ascending order, so nothing can pass by accident of
/// insertion order. Enumeration values come from the model's own catalogs
/// (`smithy_query(query="shape",shape="CollectorHealth,OSType,AuthType,VersionControlType,PipelineType")`).
fn fixture() -> Vec<Case> {
    vec![
        Case {
            what: "every member, a healthy collector with a vCenter configuration",
            record: CollectorRecord {
                ip_address: Some("10.0.4.21".to_string()),
                host_name: Some("collector-host-01.corp.example".to_string()),
                health: Some(COLLECTOR_HEALTH[0].to_string()),
                version: Some("1.0.13.13".to_string()),
                registered_time_stamp: Some("2026-10-20T09:14:02.000Z".to_string()),
                last_activity_time_stamp: Some("2026-10-24T00:11:48.000Z".to_string()),
                configuration_summary: Some(ConfigurationSummary {
                    vcenter_based_remote_info_list: Some(vec![VcenterBasedRemoteInfo {
                        vcenter_configuration_time_stamp: Some(
                            "2026-10-20T09:20:00.000Z".to_string(),
                        ),
                        os_type: Some("WINDOWS".to_string()),
                    }]),
                    ..ConfigurationSummary::default()
                }),
                ..collector("c1")
            },
            expected: json!({
                "collectorId": "c1",
                "ipAddress": "10.0.4.21",
                "hostName": "collector-host-01.corp.example",
                "collectorHealth": COLLECTOR_HEALTH[0],
                "collectorVersion": "1.0.13.13",
                "registeredTimeStamp": "2026-10-20T09:14:02.000Z",
                "lastActivityTimeStamp": "2026-10-24T00:11:48.000Z",
                "configurationSummary": {
                    "vcenterBasedRemoteInfoList": [{
                        "vcenterConfigurationTimeStamp": "2026-10-20T09:20:00.000Z",
                        "osType": "WINDOWS",
                    }],
                },
            }),
        },
        Case {
            what: "the id alone: a collector that registered and reported nothing else",
            record: collector("c0"),
            expected: json!({"collectorId": "c0"}),
        },
        Case {
            what: "an unhealthy collector, addressed over IPv6, with no configuration",
            record: CollectorRecord {
                ip_address: Some("2001:db8::4:21".to_string()),
                health: Some(COLLECTOR_HEALTH[1].to_string()),
                last_activity_time_stamp: Some("2026-09-01T12:00:00.000Z".to_string()),
                ..collector("c3")
            },
            expected: json!({
                "collectorId": "c3",
                "ipAddress": "2001:db8::4:21",
                "collectorHealth": COLLECTOR_HEALTH[1],
                "lastActivityTimeStamp": "2026-09-01T12:00:00.000Z",
            }),
        },
        Case {
            what: "every other kind of remote configuration a summary holds, several at once",
            record: CollectorRecord {
                host_name: Some("collector-host-02".to_string()),
                version: Some("1.0.14.0".to_string()),
                configuration_summary: Some(ConfigurationSummary {
                    ip_address_based_remote_info_list: Some(vec![
                        IPAddressBasedRemoteInfo {
                            ip_address_configuration_time_stamp: Some(
                                "2026-10-21T10:00:00.000Z".to_string(),
                            ),
                            auth_type: Some("SSH".to_string()),
                            os_type: Some("LINUX".to_string()),
                        },
                        IPAddressBasedRemoteInfo {
                            ip_address_configuration_time_stamp: Some(
                                "2026-10-21T10:05:00.000Z".to_string(),
                            ),
                            auth_type: Some("NTLM".to_string()),
                            os_type: Some("WINDOWS".to_string()),
                        },
                    ]),
                    version_control_info_list: Some(vec![VersionControlInfo {
                        version_control_type: Some("GITHUB".to_string()),
                        version_control_configuration_time_stamp: Some(
                            "2026-10-21T11:00:00.000Z".to_string(),
                        ),
                    }]),
                    pipeline_info_list: Some(vec![PipelineInfo {
                        pipeline_type: Some("AZURE_DEVOPS".to_string()),
                        pipeline_configuration_time_stamp: Some(
                            "2026-10-21T11:30:00.000Z".to_string(),
                        ),
                    }]),
                    remote_source_code_analysis_server_info: Some(
                        RemoteSourceCodeAnalysisServerInfo {
                            remote_source_code_analysis_server_configuration_timestamp: Some(
                                "2026-10-21T12:00:00.000Z".to_string(),
                            ),
                        },
                    ),
                    ..ConfigurationSummary::default()
                }),
                ..collector("c2")
            },
            expected: json!({
                "collectorId": "c2",
                "hostName": "collector-host-02",
                "collectorVersion": "1.0.14.0",
                "configurationSummary": {
                    "ipAddressBasedRemoteInfoList": [
                        {
                            "ipAddressConfigurationTimeStamp": "2026-10-21T10:00:00.000Z",
                            "authType": "SSH",
                            "osType": "LINUX",
                        },
                        {
                            "ipAddressConfigurationTimeStamp": "2026-10-21T10:05:00.000Z",
                            "authType": "NTLM",
                            "osType": "WINDOWS",
                        },
                    ],
                    "versionControlInfoList": [{
                        "versionControlType": "GITHUB",
                        "versionControlConfigurationTimeStamp": "2026-10-21T11:00:00.000Z",
                    }],
                    "pipelineInfoList": [{
                        "pipelineType": "AZURE_DEVOPS",
                        "pipelineConfigurationTimeStamp": "2026-10-21T11:30:00.000Z",
                    }],
                    "remoteSourceCodeAnalysisServerInfo": {
                        "remoteSourceCodeAnalysisServerConfigurationTimestamp":
                            "2026-10-21T12:00:00.000Z",
                    },
                },
            }),
        },
        Case {
            what: "a registration time stamp alone, with a configuration holding nothing",
            record: CollectorRecord {
                registered_time_stamp: Some("2026-01-02T03:04:05.000Z".to_string()),
                configuration_summary: Some(ConfigurationSummary::default()),
                ..collector("c4")
            },
            expected: json!({
                "collectorId": "c4",
                "registeredTimeStamp": "2026-01-02T03:04:05.000Z",
                "configurationSummary": {},
            }),
        },
    ]
}

/// The fixture's records, for seeding.
fn registered() -> Vec<CollectorRecord> {
    fixture().into_iter().map(|case| case.record).collect()
}

/// The collectors the whole fixture must report, in ascending id order.
fn expected_in_order() -> Vec<Value> {
    let mut rows: Vec<(String, Value)> = fixture()
        .into_iter()
        .map(|case| (case.record.id.clone(), case.expected))
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows.into_iter().map(|(_, reported)| reported).collect()
}

/// Every registered collector is reported, member for member as its registration recorded
/// it, in the collectors' one total order.
#[tokio::test]
async fn every_registered_collector_is_reported_member_for_member() {
    let router = emulator_with(registered());
    let answer = list(&router, "").await;

    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from an unparameterized call, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        reported(&answer),
        expected_in_order(),
        "the list must report every collector in ascending id order, got {:?}",
        answer.body
    );
    assert_eq!(
        at(&answer.body, "/nextToken"),
        Value::Null,
        "a page that ended the list carries no nextToken, got {:?}",
        answer.body
    );

    for entry in reported(&answer) {
        let object = entry
            .as_object()
            .unwrap_or_else(|| panic!("a collector must be an object, got {entry:?}"));
        for key in object.keys() {
            assert!(
                COLLECTOR_MEMBERS.contains(&key.as_str()),
                "case {entry:?}: Collector declares no member {key:?}"
            );
        }
        if let Some(health) = object.get("collectorHealth").and_then(Value::as_str) {
            assert!(
                COLLECTOR_HEALTH.contains(&health),
                "case {entry:?}: CollectorHealth does not declare {health:?}"
            );
        }
    }
}

/// Each fixture row on its own: the one-collector account reports exactly that row.
#[tokio::test]
async fn one_collector_reports_exactly_what_its_record_holds() {
    for Case {
        what,
        record,
        expected,
    } in fixture()
    {
        let router = emulator_with(vec![record]);
        let answer = list(&router, "").await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer),
            vec![expected.clone()],
            "case {what:?}: the list must report {expected:?}, got {:?}",
            answer.body
        );
    }
}

/// An account no collector registered in reports an empty list and no token — never a
/// collector nothing holds.
#[tokio::test]
async fn an_account_with_no_collector_reports_an_empty_list() {
    let router = emulator_with(Vec::new());

    for query in ["", "maxResults=1", "maxResults=100"] {
        let answer = list(&router, query).await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(&answer.body, "/Collectors"),
            json!([]),
            "case {query:?}: an account with no collector reports an empty list, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/nextToken"),
            Value::Null,
            "case {query:?}: a list that ended carries no nextToken, got {:?}",
            answer.body
        );
    }
}

/// A walk through `nextToken` reports every collector exactly once, in the page size asked
/// for.
///
/// The rows differ in how many collectors are registered (the whole fixture, one, none)
/// and in page size (every size over the fixture, one past its length, and the documented
/// maximum) — the invariant is the same for all of them: the concatenated pages are
/// exactly the single-page answer, no page is longer than `maxResults`, and only the page
/// that ends the list carries no token.
#[tokio::test]
async fn pagination_walks_every_collector_exactly_once() {
    let accounts: Vec<(&str, Vec<CollectorRecord>)> = vec![
        ("the whole fixture", registered()),
        ("one collector", vec![collector("c0")]),
        ("no collector at all", Vec::new()),
    ];

    for (account, collectors) in accounts {
        let router = emulator_with(collectors);
        let whole = reported(&list(&router, "").await);

        let mut sizes: Vec<usize> = (1..=(whole.len() + 1).max(2)).collect();
        sizes.push(usize::try_from(MAX_RESULTS_MAX).unwrap_or(100));
        for size in sizes {
            let case = format!("{account} in pages of {size}");
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let query = match &token {
                    None => format!("maxResults={size}"),
                    Some(token) => format!("maxResults={size}&nextToken={token}"),
                };
                let answer = list(&router, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200 on page {pages}, got {} with {:?}",
                    answer.status, answer.body
                );
                let page = reported(&answer);
                assert!(
                    page.len() <= size,
                    "case {case:?}: a page holds at most {size} collectors, got {page:?}"
                );
                walked.extend(page);
                pages += 1;
                assert!(
                    pages <= whole.len() + 2,
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
                walked, whole,
                "case {case:?}: the walk must report {whole:?} exactly once each, got {walked:?}"
            );
        }
    }
}

/// Every request the operation cannot act on: 400 `ValidationException`, in this service's
/// own wording, and nothing reported.
#[tokio::test]
async fn a_request_the_operation_cannot_act_on_is_refused() {
    let router = emulator_with(registered());

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "maxResults of 0, a page of nothing",
            "maxResults=0".to_string(),
            INVALID_REQUEST_BODY,
        ),
        (
            "a negative maxResults",
            "maxResults=-1".to_string(),
            INVALID_REQUEST_BODY,
        ),
        (
            "maxResults one past the documented maximum",
            format!("maxResults={}", MAX_RESULTS_MAX + 1),
            INVALID_REQUEST_BODY,
        ),
        (
            "maxResults far past the documented maximum",
            "maxResults=1000".to_string(),
            INVALID_REQUEST_BODY,
        ),
        (
            "a nextToken that is not even an encoding this service issues",
            "nextToken=not-a-token-this-service-issued".to_string(),
            INVALID_OR_MISSING_PARAMETERS,
        ),
        (
            "a nextToken that decodes but carries no marker of ours",
            "nextToken=deadbeef".to_string(),
            INVALID_OR_MISSING_PARAMETERS,
        ),
    ];

    for (what, query, message) in cases {
        let answer = list(&router, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400 for {query:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException for {query:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            message,
            "case {what:?}: expected the service's own wording for {query:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/Collectors"),
            Value::Null,
            "case {what:?}: a refused request reports no collector, got {:?}",
            answer.body
        );
    }

    // The documented maximum itself is accepted: the bound is inclusive.
    let answer = list(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await;
    assert_eq!(
        answer.status, 200,
        "the documented maximum must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        reported(&answer),
        expected_in_order(),
        "a page of the documented maximum holds the whole fixture, got {:?}",
        answer.body
    );
}

/// A token names a position in ONE list: one this list issued resumes it, and one issued
/// by a neighbouring list of this service is foreign here — refused rather than resolved
/// against the collectors.
#[tokio::test]
async fn a_token_is_only_good_for_the_list_it_was_issued_for() {
    let router = emulator_with(registered());

    let first = list(&router, "maxResults=1").await;
    let token = first.string("/nextToken");
    assert!(
        !token.is_empty(),
        "the first page of several collectors must carry a nextToken, got {:?}",
        first.body
    );

    // The page size is documented per call, so changing it does not invalidate a token.
    let resumed = list(&router, &format!("maxResults=2&nextToken={token}")).await;
    assert_eq!(
        resumed.status, 200,
        "a token of this list resumes it whatever page size follows, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        reported(&resumed),
        expected_in_order()[1..3].to_vec(),
        "the resumed page must continue after the first collector, got {:?}",
        resumed.body
    );

    // Two import file tasks, so the neighbouring list pages and issues a token of its own.
    for name in ["collector-token-1", "collector-token-2"] {
        let started = start_import(&router, minimal_import(name)).await;
        assert_eq!(
            started.status, 200,
            "the import that sets this case up must succeed, got {} with {:?}",
            started.status, started.body
        );
    }
    let task_page = call(&router, "GET", "/list-import-file-task?maxResults=1", None).await;
    let foreign = task_page.string("/nextToken");
    assert!(
        !foreign.is_empty(),
        "the ListImportFileTask fixture must page, got {:?}",
        task_page.body
    );

    let answer = list(&router, &format!("nextToken={foreign}")).await;
    assert_eq!(
        answer.status, 400,
        "a ListImportFileTask token is foreign to this list, got {} with {:?}",
        answer.status, answer.body
    );
    let answer = call(
        &router,
        "GET",
        &format!("/list-import-file-task?nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "a ListCollectors token is foreign to ListImportFileTask, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The read reports and does not mutate: the same call answers the same thing twice, a
/// paged walk consumes nothing, and the neighbouring read answers the same before and
/// after.
#[tokio::test]
async fn reading_the_list_changes_nothing_a_later_caller_sees() {
    let router = emulator_with(registered());

    let before = call(&router, "GET", "/list-import-file-task", None).await;
    let first = list(&router, "").await;
    for size in [1, 2] {
        let _ = list(&router, &format!("maxResults={size}")).await;
    }
    let second = list(&router, "").await;
    let after = call(&router, "GET", "/list-import-file-task", None).await;

    assert_eq!(
        reported(&second),
        reported(&first),
        "two identical calls must answer the same thing: {:?} then {:?}",
        first.body,
        second.body
    );
    assert_eq!(
        after.body, before.body,
        "reading the collectors must not change what a later caller sees: {:?} then {:?}",
        before.body, after.body
    );
}

/// A collector registered in one Region is reported only by a read signed for that
/// Region: the store is partitioned per Region, and a read of another Region answers as
/// the empty account it is — at every page size, and still refusing a token it never
/// issued.
#[tokio::test]
async fn a_collector_is_reported_only_in_the_region_it_registered_in() {
    let router = emulator_with(registered());

    let here = list_in(&router, TEST_REGION, "").await;
    assert_eq!(
        reported(&here),
        expected_in_order(),
        "the Region the collectors registered in reports them, got {:?}",
        here.body
    );

    for region in ["us-east-1", "ap-southeast-2"] {
        for query in ["", "maxResults=1"] {
            let answer = list_in(&router, region, query).await;
            assert_eq!(
                answer.status, 200,
                "case {region:?} {query:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                at(&answer.body, "/Collectors"),
                json!([]),
                "case {region:?} {query:?}: a Region no collector registered in reports an \
                 empty list, got {:?}",
                answer.body
            );
        }
        let refused = list_in(&router, region, "maxResults=0").await;
        assert_eq!(
            refused.status, 400,
            "case {region:?}: the page rules hold in every Region, got {} with {:?}",
            refused.status, refused.body
        );
    }
}
