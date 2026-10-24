// SPDX-License-Identifier: CC0-1.0

//! MediaLive Anywhere's cluster: what `CreateCluster` persists, what
//! `DescribeCluster` reports back and what `ListClusterAlerts` reports out of a
//! cluster's alert log, end to end over the restJson1 wire.
//!
//! The rules each suite below asserts, each over a table that spans its class
//! rather than one recorded request:
//!
//! 1. `ListClusterAlerts` reports the alerts the named cluster's stored log
//!    holds, oldest first, narrowed to the ones its `StateFilter` admits (SET,
//!    CLEARED or ALL — the values
//!    `smithy_query(query="operation",operation="ListClusterAlerts")`'s
//!    `StateFilter` documents; an absent filter narrows nothing), in pages of at
//!    most `MaxResults`, handing out a positional `NextToken` exactly while
//!    admitted alerts are left over. An alert's `State` is SET while it carries
//!    no `ClearedTimestamp` and CLEARED once it does.
//! 2. A cluster is persisted by `CreateCluster` and read back by
//!    `DescribeCluster`: the id and the ARN are minted, `State` is `CREATING`
//!    until the clock makes `ACTIVE` due, `ChannelIds` is derived from the
//!    channels whose `AnywhereSettings$ClusterId` names the cluster, and every
//!    other member is the one the create request supplied.
//! 3. A `ClusterId` naming no stored cluster is a 404 `NotFoundException`, and a
//!    request this operation cannot read (a `StateFilter` outside the documented
//!    set, a `MaxResults` below 1, a `NextToken` that is not a position) is a
//!    400 `BadRequestException`.
//!
//! NOTHING in this emulator raises a cluster alert — the real service raises one
//! from the Nodes and the encoding hardware it runs for a Cluster, and this
//! emulator runs neither — so the alert LOG of a cluster created through the API
//! is empty (asserted below), and the suites that exercise the filtering and the
//! pagination seed the log through the store's own one writer,
//! `raise_cluster_alert`, instead of asking an operation to invent alerts.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    self, ClusterAlertRecord, MAX_LIST_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::{epoch_seconds, member, timestamp, unix_seconds};
use crate::wire::{call, error_code, error_message, medialive, medialive_with_engine, ACCOUNT};

/// The cluster collection URI
/// (`smithy_query(query="operation",operation="CreateCluster")`: POST
/// /prod/clusters; the describe is `<collection>/{ClusterId}` and the alerts are
/// `<collection>/{ClusterId}/alerts`).
const CLUSTERS: &str = "/prod/clusters";

/// One cluster created from `body`, answering the create's response.
async fn create_cluster(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, created) = call(router, "POST", CLUSTERS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCluster to answer 201, got {status}: {created}"
    );
    created
}

/// One `ListClusterAlerts` call, with `query` appended to the alerts URI.
async fn list_alerts(
    router: &MultiServiceRouter,
    cluster_id: &str,
    query: &str,
) -> (u16, Value) {
    let uri = format!("{CLUSTERS}/{cluster_id}/alerts{query}");
    call(router, "GET", &uri, None).await
}

/// An alert raised `set_secs` after the epoch, cleared `cleared_secs` after it
/// when the alert is no longer happening.
///
/// The optional members are filled in so a reported alert pins all of them: the
/// subtype (`ClusterAlert$Id`, "The further subtype of this alert"), the message
/// and the channel the condition is about.
fn alert(alert_type: &str, set_secs: u64, cleared_secs: Option<u64>) -> ClusterAlertRecord {
    let mut record = ClusterAlertRecord::raised(alert_type, at(set_secs));
    record.id = Some(format!("{alert_type}-subtype"));
    record.message = Some(format!("{alert_type} on this cluster"));
    record.channel_id = Some("1000001".to_string());
    record.node_id = Some("8000001".to_string());
    record.cleared_at = cleared_secs.map(at);
    record
}

/// An instant a fixed number of seconds after the Unix epoch, so an asserted
/// timestamp names an instant the test chose rather than one it has to guess.
fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// Seed `alerts` into the stored alert log of `cluster_id`, in the order given.
///
/// The store's own writer, so the log the operation reads is the log something
/// raised: there is no second copy for the two to disagree about.
fn raise(
    engine: &engine::InMemoryEngine,
    cluster_id: &str,
    alerts: impl IntoIterator<Item = ClusterAlertRecord>,
) {
    let store = state::service_state(engine).expect("the medialive store");
    let mut store = store.lock();
    for alert in alerts {
        assert!(
            store.raise_cluster_alert(cluster_id, alert),
            "setup: expected cluster {cluster_id} to take the alert"
        );
    }
}

/// The `alertType` of every alert one page reports, in the order it reported
/// them.
fn reported_types(case: &str, page: &Value) -> Vec<String> {
    let alerts = page["alerts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an alerts array, got {page}"));
    alerts
        .iter()
        .map(|alert| {
            alert["alertType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an alertType, got {alert}"))
                .to_string()
        })
        .collect()
}

/// The log every filtering and pagination row below is read from: six alerts,
/// three still happening and three cleared, raised in this order.
fn seeded_log() -> Vec<ClusterAlertRecord> {
    vec![
        alert("NODE_UNREACHABLE", 1_700_000_000, None),
        alert("INPUT_SIGNAL_LOST", 1_700_000_060, Some(1_700_000_600)),
        alert("DISK_PRESSURE", 1_700_000_120, None),
        alert("CLOCK_DRIFT", 1_700_000_180, Some(1_700_000_700)),
        alert("INTERFACE_DOWN", 1_700_000_240, None),
        alert("CERTIFICATE_EXPIRING", 1_700_000_300, Some(1_700_000_800)),
    ]
}

/// The `alertType`s of [`seeded_log`] that each `StateFilter` admits.
const SET_TYPES: [&str; 3] = ["NODE_UNREACHABLE", "DISK_PRESSURE", "INTERFACE_DOWN"];
const CLEARED_TYPES: [&str; 3] = [
    "INPUT_SIGNAL_LOST",
    "CLOCK_DRIFT",
    "CERTIFICATE_EXPIRING",
];
const ALL_TYPES: [&str; 6] = [
    "NODE_UNREACHABLE",
    "INPUT_SIGNAL_LOST",
    "DISK_PRESSURE",
    "CLOCK_DRIFT",
    "INTERFACE_DOWN",
    "CERTIFICATE_EXPIRING",
];

/// A router carrying one cluster with [`seeded_log`] raised against it,
/// answering the cluster's id.
async fn cluster_with_alerts() -> (MultiServiceRouter, String) {
    let (router, engine) = medialive_with_engine();
    let created = create_cluster(
        &router,
        "seeded cluster",
        json!({ "name": "alerting-cluster", "clusterType": "ON_PREMISES" }),
    )
    .await;
    let id = created["id"]
        .as_str()
        .expect("the created cluster reports an id")
        .to_string();
    raise(&engine, &id, seeded_log());
    (router, id)
}

/// Rule 1: the alerts a page reports are the stored log narrowed to what the
/// request's `StateFilter` admits, oldest first, and an alert's `State` and
/// `ClearedTimestamp` agree.
///
/// The rows span the documented filter values (SET | CLEARED | ALL) plus the
/// absent filter, which narrows nothing.
#[tokio::test]
async fn list_cluster_alerts_reports_the_alerts_the_state_filter_admits() {
    // (case, the StateFilter the request carried, the alertTypes it admits)
    let table: [(&str, Option<&str>, &[&str]); 4] = [
        ("no filter", None, &ALL_TYPES),
        ("ALL", Some("ALL"), &ALL_TYPES),
        ("SET", Some("SET"), &SET_TYPES),
        ("CLEARED", Some("CLEARED"), &CLEARED_TYPES),
    ];

    for (case, filter, admitted) in table {
        let (router, id) = cluster_with_alerts().await;
        let query = filter.map_or_else(String::new, |value| format!("?stateFilter={value}"));
        let (status, page) = list_alerts(&router, &id, &query).await;

        assert_eq!(
            status, 200,
            "case {case:?}: expected ListClusterAlerts to answer 200, got {status}: {page}"
        );
        assert_eq!(
            reported_types(case, &page),
            admitted.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: expected exactly the admitted alerts, oldest first, got {page}"
        );
        assert_eq!(
            member(&page, "nextToken"),
            Value::Null,
            "case {case:?}: a page that reported every admitted alert hands out no token, got {:?}",
            member(&page, "nextToken")
        );

        for alert in page["alerts"].as_array().expect("an alerts array") {
            let alert_type = alert["alertType"].as_str().unwrap_or_default();
            let cleared = member(alert, "clearedTimestamp");
            let expected_state = if cleared == Value::Null { "SET" } else { "CLEARED" };
            assert_eq!(
                alert["state"],
                json!(expected_state),
                "case {case:?}: alert {alert_type} carries clearedTimestamp {cleared:?}, so its \
                 state must be {expected_state}, got {alert}"
            );
            // The stored instants, in the modelled `__timestampIso8601` form.
            let set_at = epoch_seconds(timestamp(case, "setTimestamp", alert));
            assert!(
                (1_700_000_000.0..=1_700_000_300.0).contains(&set_at),
                "case {case:?}: alert {alert_type} must report the instant it was raised, got {set_at}"
            );
            if cleared != Value::Null {
                let cleared_at = epoch_seconds(timestamp(case, "clearedTimestamp", alert));
                assert!(
                    cleared_at > set_at,
                    "case {case:?}: alert {alert_type} was cleared after it was set, got \
                     cleared {cleared_at} and set {set_at}"
                );
            }
            assert_eq!(
                alert["id"],
                json!(format!("{alert_type}-subtype")),
                "case {case:?}: expected the stored subtype of {alert_type}, got {alert}"
            );
            assert_eq!(
                alert["message"],
                json!(format!("{alert_type} on this cluster")),
                "case {case:?}: expected the stored message of {alert_type}, got {alert}"
            );
            assert_eq!(
                alert["channelId"],
                json!("1000001"),
                "case {case:?}: expected the stored channelId of {alert_type}, got {alert}"
            );
            assert_eq!(
                alert["nodeId"],
                json!("8000001"),
                "case {case:?}: expected the stored nodeId of {alert_type}, got {alert}"
            );
        }
    }
}

/// One pagination row: the page size and filter a walk uses, and the alerts it
/// must report.
struct PageRow {
    case: &'static str,
    /// The `MaxResults` the request carried, or none at all.
    max_results: Option<i32>,
    /// The `StateFilter` the request carried, or none at all.
    filter: Option<&'static str>,
    /// The `alertType`s the walk must report, in log order.
    admitted: &'static [&'static str],
}

/// Rule 1, the pagination half: a walk that follows `NextToken` reports every
/// admitted alert exactly once, in log order, in pages of at most `MaxResults`,
/// and the last page carries no token.
///
/// The rows span the page size (1, 2, 3, every alert at once, more than the log
/// holds, and the absent one that uses the service's ceiling) crossed with the
/// filter, so the pagination is asserted over a narrowed sequence as well as the
/// whole log.
#[tokio::test]
async fn list_cluster_alerts_pages_the_admitted_alerts() {
    let table: [PageRow; 7] = [
        PageRow { case: "one at a time", max_results: Some(1), filter: None, admitted: &ALL_TYPES },
        PageRow { case: "two at a time", max_results: Some(2), filter: None, admitted: &ALL_TYPES },
        PageRow {
            case: "three at a time",
            max_results: Some(3),
            filter: Some("ALL"),
            admitted: &ALL_TYPES,
        },
        PageRow {
            case: "the whole log in one page",
            max_results: Some(6),
            filter: None,
            admitted: &ALL_TYPES,
        },
        PageRow {
            case: "a page larger than the log",
            max_results: Some(100),
            filter: None,
            admitted: &ALL_TYPES,
        },
        PageRow {
            case: "one SET at a time",
            max_results: Some(1),
            filter: Some("SET"),
            admitted: &SET_TYPES,
        },
        PageRow {
            case: "every CLEARED, unpaged",
            max_results: None,
            filter: Some("CLEARED"),
            admitted: &CLEARED_TYPES,
        },
    ];

    for PageRow {
        case,
        max_results,
        filter,
        admitted,
    } in table
    {
        let (router, id) = cluster_with_alerts().await;
        let page_cap = max_results
            .map_or(MAX_LIST_PAGE_SIZE, |size| {
                (size as usize).min(MAX_LIST_PAGE_SIZE)
            });

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = max_results {
                query.push(format!("maxResults={size}"));
            }
            if let Some(value) = filter {
                query.push(format!("stateFilter={value}"));
            }
            if let Some(value) = &token {
                query.push(format!("nextToken={value}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let (status, page) = list_alerts(&router, &id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} expected 200, got {status}: {page}"
            );
            let reported = reported_types(case, &page);
            assert!(
                reported.len() <= page_cap,
                "case {case:?}: page {pages} must hold at most {page_cap} alerts, got {}",
                reported.len()
            );
            walked.extend(reported);
            pages += 1;
            assert!(
                pages <= admitted.len() + 1,
                "case {case:?}: the walk must terminate, but took {pages} pages for {} alerts",
                admitted.len()
            );

            match member(&page, "nextToken") {
                Value::Null => break,
                Value::String(next) => token = Some(next),
                other => panic!("case {case:?}: nextToken must be a string, got {other:?}"),
            }
        }

        assert_eq!(
            walked,
            admitted.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: the walk must report every admitted alert exactly once, in log order"
        );
    }
}

/// Rule 1, the empty half: a cluster created through the API carries no alerts,
/// because nothing in this emulator raises one — so every filter and every page
/// size reports an empty page with no token, rather than an invented alert.
#[tokio::test]
async fn a_created_cluster_reports_an_empty_alert_log() {
    // (case, the query the request carried)
    let table: [(&str, &str); 6] = [
        ("no query", ""),
        ("ALL", "?stateFilter=ALL"),
        ("SET", "?stateFilter=SET"),
        ("CLEARED", "?stateFilter=CLEARED"),
        ("one per page", "?maxResults=1"),
        ("one SET per page", "?maxResults=1&stateFilter=SET"),
    ];

    let router = medialive();
    let created = create_cluster(
        &router,
        "quiet cluster",
        json!({ "name": "quiet-cluster", "clusterType": "ON_PREMISES" }),
    )
    .await;
    let id = created["id"].as_str().expect("an id").to_string();

    for (case, query) in table {
        let (status, page) = list_alerts(&router, &id, query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {page}"
        );
        assert_eq!(
            page["alerts"],
            json!([]),
            "case {case:?}: a cluster nothing raised an alert against reports the empty list, got {page}"
        );
        assert_eq!(
            member(&page, "nextToken"),
            Value::Null,
            "case {case:?}: an empty page hands out no token, got {page}"
        );
    }
}

/// Rule 2: `CreateCluster` persists the cluster and `DescribeCluster` reports
/// the same one back out of the store — every member the request supplied, the
/// minted id and ARN, the `CREATING` state the clock settles to `ACTIVE`, and
/// the derived `ChannelIds`.
///
/// The rows span what the request supplies: a fully specified cluster, one with
/// no network settings, one with several interface mappings and one with no name
/// at all (the member is optional, so a value is never invented for it).
#[tokio::test]
async fn a_created_cluster_round_trips_through_describe_cluster() {
    // (case, the CreateCluster request body)
    let table: [(&str, Value); 4] = [
        (
            "fully specified",
            json!({
                "name": "cluster-full",
                "clusterType": "ON_PREMISES",
                "instanceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaLiveAnywhere"),
                "networkSettings": {
                    "defaultRoute": "net-1",
                    "interfaceMappings": [
                        { "logicalInterfaceName": "primary", "networkId": "net-1" }
                    ]
                },
                "tags": { "team": "video" }
            }),
        ),
        (
            "no network settings",
            json!({ "name": "cluster-bare", "clusterType": "ON_PREMISES" }),
        ),
        (
            "several interface mappings",
            json!({
                "name": "cluster-mapped",
                "clusterType": "ON_PREMISES",
                "networkSettings": {
                    "defaultRoute": "net-a",
                    "interfaceMappings": [
                        { "logicalInterfaceName": "primary", "networkId": "net-a" },
                        { "logicalInterfaceName": "backup", "networkId": "net-b" },
                        { "logicalInterfaceName": "management", "networkId": "net-c" }
                    ]
                }
            }),
        ),
        ("no name", json!({ "clusterType": "ON_PREMISES" })),
    ];

    for (case, body) in table {
        let router = medialive();
        let created = create_cluster(&router, case, body.clone()).await;

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric cluster id, got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:cluster:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["state"],
            json!("CREATING"),
            "case {case:?}: expected a new cluster to be CREATING, got {:?}",
            created["state"]
        );
        assert_eq!(
            created["channelIds"],
            json!([]),
            "case {case:?}: no channel runs on a cluster that was just created, got {:?}",
            created["channelIds"]
        );
        assert_eq!(
            member(&created, "name"),
            member(&body, "name"),
            "case {case:?}: expected the name the request supplied, got {created}"
        );
        assert_eq!(
            member(&created, "clusterType"),
            member(&body, "clusterType"),
            "case {case:?}: expected the clusterType the request supplied, got {created}"
        );
        assert_eq!(
            member(&created, "instanceRoleArn"),
            member(&body, "instanceRoleArn"),
            "case {case:?}: expected the instanceRoleArn the request supplied, got {created}"
        );
        assert_eq!(
            member(&created, "networkSettings"),
            member(&body, "networkSettings"),
            "case {case:?}: expected the networkSettings the request supplied, got {created}"
        );

        // The read reports the stored cluster: the same body, out of state.
        let (status, described) = call(&router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCluster to answer 200, got {status}: {described}"
        );
        assert_eq!(
            described, created,
            "case {case:?}: expected the read to report the cluster the create persisted"
        );

        // A channel whose AnywhereSettings name this cluster is what ChannelIds
        // reports: derived from the channel store on every read.
        let (status, channel) = call(
            &router,
            "POST",
            "/prod/channels",
            Some(json!({
                "name": format!("channel-{case}"),
                "anywhereSettings": { "clusterId": id, "channelPlacementGroupId": "pg-1" }
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateChannel to answer 201, got {status}: {channel}"
        );
        let channel_id = channel["channel"]["id"].as_str().unwrap_or_default().to_string();

        wait_for_the_clock();
        let (status, settled) = call(&router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCluster to answer 200, got {status}: {settled}"
        );
        assert_eq!(
            settled["state"],
            json!("ACTIVE"),
            "case {case:?}: expected the cluster to have settled to ACTIVE, got {:?}",
            settled["state"]
        );
        assert_eq!(
            settled["channelIds"],
            json!([channel_id]),
            "case {case:?}: expected the channel that names this cluster, got {:?}",
            settled["channelIds"]
        );
    }
}

/// Rule 2, the idempotency and uniqueness half: a retried create (the same
/// `@idempotencyToken` `RequestId`) answers with the cluster the first call
/// created, a second cluster under a name the account already holds is refused
/// as a conflict, and a name differing only in case is a different name
/// ("Names are case-sensitive").
#[tokio::test]
async fn create_cluster_is_idempotent_and_keeps_names_unique() {
    let router = medialive();
    let body = json!({
        "name": "unique-cluster",
        "clusterType": "ON_PREMISES",
        "requestId": "token-1"
    });

    let first = create_cluster(&router, "first create", body.clone()).await;
    let retried = create_cluster(&router, "retried create", body.clone()).await;
    assert_eq!(
        retried, first,
        "a retried create must answer with the cluster the first one created"
    );

    // A different token with the same name is a second cluster, which the
    // account's name uniqueness refuses.
    let (status, refusal) = call(
        &router,
        "POST",
        CLUSTERS,
        Some(json!({
            "name": "unique-cluster",
            "clusterType": "ON_PREMISES",
            "requestId": "token-2"
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "a second cluster under a name the account holds must conflict, got {status}: {refusal}"
    );
    assert_eq!(
        error_code(status, &refusal),
        "ConflictException",
        "expected ConflictException, got {refusal}"
    );

    // Case-sensitive: the same letters in another case name another cluster.
    let other_case = create_cluster(
        &router,
        "name in another case",
        json!({
            "name": "UNIQUE-CLUSTER",
            "clusterType": "ON_PREMISES",
            "requestId": "token-3"
        }),
    )
    .await;
    assert_ne!(
        other_case["id"], first["id"],
        "expected a name differing in case to name a different cluster, got {other_case}"
    );

    // Exactly two clusters exist, so neither the retry nor the refusal minted
    // one: the reads are what prove it.
    for (case, id, expected_name) in [
        ("the first cluster", &first["id"], "unique-cluster"),
        ("the other-case cluster", &other_case["id"], "UNIQUE-CLUSTER"),
    ] {
        let id = id.as_str().unwrap_or_default();
        let (status, described) = call(&router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCluster to answer 200, got {status}: {described}"
        );
        assert_eq!(
            described["name"],
            json!(expected_name),
            "case {case:?}: expected the stored name, got {described}"
        );
    }
}

/// Rule 3: a `ClusterId` naming no stored cluster is a 404 `NotFoundException`,
/// on the read and on the listing alike.
///
/// The rows span the ways an id can name nothing: an id in the cluster range
/// that was never minted, one in another resource type's range, and one that is
/// not a MediaLive id at all.
#[tokio::test]
async fn an_unknown_cluster_is_not_found() {
    // (case, the ClusterId the request named)
    let table: [(&str, &str); 3] = [
        ("never minted", "8009999"),
        ("a channel-range id", "1000000"),
        ("not an id at all", "no-such-cluster"),
    ];

    let router = medialive();
    // One cluster exists, so a 404 is about the id the request named rather than
    // about an empty store.
    create_cluster(
        &router,
        "setup",
        json!({ "name": "present-cluster", "clusterType": "ON_PREMISES" }),
    )
    .await;

    for (case, cluster_id) in table {
        for (operation, uri) in [
            ("ListClusterAlerts", format!("{CLUSTERS}/{cluster_id}/alerts")),
            ("DescribeCluster", format!("{CLUSTERS}/{cluster_id}")),
        ] {
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected {operation} to answer 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {case:?}: expected {operation} to answer NotFoundException, got {body}"
            );
            assert!(
                error_message(&body).contains(cluster_id),
                "case {case:?}: expected {operation}'s message to name the id, got {:?}",
                error_message(&body)
            );
        }
    }
}

/// Rule 3, the bad-request half: a request `ListClusterAlerts` cannot read is a
/// 400 `BadRequestException` naming what it could not read, and it changes
/// nothing.
///
/// The rows span the three members a request can get wrong: a `StateFilter`
/// outside the documented SET | CLEARED | ALL set (including one that differs
/// only in case), a `MaxResults` below the 1 a page size must be, and a
/// `NextToken` that is not a position in the log.
#[tokio::test]
async fn list_cluster_alerts_refuses_a_request_it_cannot_read() {
    // (case, the query the request carried, the text its message must name)
    let table: [(&str, &str, &str); 7] = [
        ("an unknown state", "?stateFilter=OPEN", "OPEN"),
        ("a lowercase state", "?stateFilter=set", "set"),
        ("two states at once", "?stateFilter=SET,CLEARED", "SET,CLEARED"),
        ("a zero page size", "?maxResults=0", "0"),
        ("a negative page size", "?maxResults=-3", "-3"),
        ("a token that is not a position", "?nextToken=later", "later"),
        (
            "a negative position",
            "?nextToken=-1",
            "-1",
        ),
    ];

    let (router, id) = cluster_with_alerts().await;

    for (case, query, named) in table {
        let (status, body) = list_alerts(&router, &id, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert!(
            error_message(&body).contains(named),
            "case {case:?}: expected the message to name {named:?}, got {:?}",
            error_message(&body)
        );
    }

    // The refusals changed nothing: the log still reports every alert it held.
    let (status, page) = list_alerts(&router, &id, "").await;
    assert_eq!(status, 200, "expected 200 after the refusals, got {status}: {page}");
    assert_eq!(
        reported_types("after the refusals", &page),
        ALL_TYPES.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "expected the refused requests to have left the log alone, got {page}"
    );
}

/// Rule 1, the resumed-walk edge: a `NextToken` for a position the log no longer
/// reaches resumes at the end of the log — an empty page with no token — rather
/// than failing a walk that was valid when it began.
#[tokio::test]
async fn a_token_past_the_end_of_the_log_reports_an_empty_page() {
    // (case, the position the token named)
    let table: [(&str, usize); 3] = [
        ("exactly the end", ALL_TYPES.len()),
        ("one past the end", ALL_TYPES.len() + 1),
        ("far past the end", 10_000),
    ];

    let (router, id) = cluster_with_alerts().await;

    for (case, position) in table {
        let (status, page) = list_alerts(&router, &id, &format!("?nextToken={position}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {page}"
        );
        assert_eq!(
            page["alerts"],
            json!([]),
            "case {case:?}: expected no alerts past the end of the log, got {page}"
        );
        assert_eq!(
            member(&page, "nextToken"),
            Value::Null,
            "case {case:?}: expected no token past the end of the log, got {page}"
        );
    }
}

/// The alert log is bounded: raising more than the cap keeps the most recent
/// alerts and drops the oldest, so a cluster's history cannot grow the
/// emulator's heap without bound.
#[tokio::test]
async fn the_alert_log_keeps_the_most_recent_alerts() {
    let (router, engine) = medialive_with_engine();
    let created = create_cluster(
        &router,
        "bounded log",
        json!({ "name": "bounded-cluster", "clusterType": "ON_PREMISES" }),
    )
    .await;
    let id = created["id"].as_str().expect("an id").to_string();

    let cap = state::MAX_CLUSTER_ALERTS;
    let raised = cap + 10;
    raise(
        &engine,
        &id,
        (0..raised).map(|index| {
            alert(
                &format!("ALERT_{index:04}"),
                1_700_000_000 + index as u64,
                None,
            )
        }),
    );

    // Walk every page, so the whole log is read through the wire.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(value) => format!("?maxResults={MAX_LIST_PAGE_SIZE}&nextToken={value}"),
            None => format!("?maxResults={MAX_LIST_PAGE_SIZE}"),
        };
        let (status, page) = list_alerts(&router, &id, &query).await;
        assert_eq!(status, 200, "expected 200 walking the log, got {status}: {page}");
        walked.extend(reported_types("bounded log", &page));
        match member(&page, "nextToken") {
            Value::Null => break,
            Value::String(next) => token = Some(next),
            other => panic!("nextToken must be a string, got {other:?}"),
        }
    }

    assert_eq!(
        walked.len(),
        cap,
        "expected the log to hold at most {cap} alerts after raising {raised}, got {}",
        walked.len()
    );
    assert_eq!(
        walked.first().map(String::as_str),
        Some(format!("ALERT_{:04}", raised - cap).as_str()),
        "expected the oldest alerts to have been dropped, got {:?}",
        walked.first()
    );
    assert_eq!(
        walked.last().map(String::as_str),
        Some(format!("ALERT_{:04}", raised - 1).as_str()),
        "expected the most recent alert to be kept, got {:?}",
        walked.last()
    );
}

/// The alert instants a response reports are the instants that were stored, read
/// back as seconds since the epoch — the clock never rewrites a raised alert.
#[tokio::test]
async fn an_alert_reports_the_instants_it_was_raised_and_cleared_at() {
    let (router, engine) = medialive_with_engine();
    let created = create_cluster(
        &router,
        "timestamps",
        json!({ "name": "timestamped-cluster", "clusterType": "ON_PREMISES" }),
    )
    .await;
    let id = created["id"].as_str().expect("an id").to_string();

    // (case, the instant it was raised, the instant it was cleared)
    let table: [(&str, u64, Option<u64>); 3] = [
        ("still happening", 1_600_000_000, None),
        ("cleared a minute later", 1_600_000_100, Some(1_600_000_160)),
        ("cleared a day later", 1_600_000_200, Some(1_600_086_600)),
    ];

    raise(
        &engine,
        &id,
        table
            .iter()
            .map(|(case, set_secs, cleared)| alert(case, *set_secs, *cleared)),
    );

    let (status, page) = list_alerts(&router, &id, "").await;
    assert_eq!(status, 200, "expected 200, got {status}: {page}");
    let alerts = page["alerts"].as_array().expect("an alerts array");
    assert_eq!(
        alerts.len(),
        table.len(),
        "expected every raised alert to be reported, got {page}"
    );

    for ((case, set_secs, cleared), reported) in table.iter().zip(alerts) {
        assert_eq!(
            epoch_seconds(timestamp(case, "setTimestamp", reported)),
            unix_seconds(at(*set_secs)),
            "case {case:?}: expected the stored set instant, got {reported}"
        );
        match cleared {
            Some(secs) => assert_eq!(
                epoch_seconds(timestamp(case, "clearedTimestamp", reported)),
                unix_seconds(at(*secs)),
                "case {case:?}: expected the stored cleared instant, got {reported}"
            ),
            None => assert_eq!(
                member(reported, "clearedTimestamp"),
                Value::Null,
                "case {case:?}: an alert that is still happening carries no clearedTimestamp, got {reported}"
            ),
        }
    }
}

/// The tag operations reach a cluster by its ARN, like every other taggable
/// resource this service holds: a tag written through `CreateTags` is the tag the
/// cluster's own describe reports.
#[tokio::test]
async fn a_cluster_is_tagged_through_its_arn() {
    let router = medialive();
    let created = create_cluster(
        &router,
        "tagged",
        json!({ "name": "tagged-cluster", "clusterType": "ON_PREMISES", "tags": { "team": "video" } }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    let id = created["id"].as_str().expect("an id").to_string();

    let encoded = crate::support::percent_encoded(&arn);
    let (status, body) = call(
        &router,
        "POST",
        &format!("/prod/tags/{encoded}"),
        Some(json!({ "tags": { "stage": "prod" } })),
    )
    .await;
    assert_eq!(status, 204, "expected CreateTags to answer 204, got {status}: {body}");

    let (status, described) = call(&router, "GET", &format!("{CLUSTERS}/{id}"), None).await;
    assert_eq!(status, 200, "expected 200, got {status}: {described}");
    let (status, listed) = call(&router, "GET", &format!("/prod/tags/{encoded}"), None).await;
    assert_eq!(status, 200, "expected ListTagsForResource 200, got {status}: {listed}");
    assert_eq!(
        listed["tags"],
        json!({ "team": "video", "stage": "prod" }),
        "expected the create's tag beside the one CreateTags wrote, got {listed}"
    );
}
