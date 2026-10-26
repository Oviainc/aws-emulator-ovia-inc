// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeInboundConnections` — the destination (remote)
//! domain owner's view of the cross-cluster connections `CreateOutboundConnection`
//! mints.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the connection request helpers from
//! [`super::cross_cluster_connections`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the inbound report is the SAME stored record the outbound report describes,
//!   projected onto `InboundConnection` — same id, endpoints, mode and status, and
//!   none of the members that shape does not model — over a table of creates
//!   differing in connection mode, in connection properties and in how fully the
//!   source reference names its account and Region;
//! * a connection has an inbound view exactly while its status is one
//!   `InboundConnectionStatusCode` carries, over a table spanning a request being
//!   validated, one that was validated, and one whose validation failed because
//!   either reference resolved to no domain;
//! * the report is narrowed by EVERY documented filter name, with the values of one
//!   filter as alternatives and two filters cumulative, and a filter on a property
//!   this operation does not filter on selects nothing;
//! * paging walks the whole set exactly once over every page size from one to past
//!   the set, and a token that names no place in the set is refused with the modelled
//!   `InvalidPaginationTokenException`.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONNECTION_MODE_DIRECT, CONNECTION_MODE_VPC_ENDPOINT, CONNECTION_STATUS_PENDING_ACCEPTANCE,
    CONNECTION_STATUS_VALIDATING, CONNECTION_STATUS_VALIDATION_FAILED,
};

use super::cross_cluster_connections::{
    connections, create_connection, created_domain, delete_domain,
    describe_connections as describe_outbound, domain_info, reported, reported_ids, ACCOUNT, REGION,
};
use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// `DescribeInboundConnections`: `POST /opensearch/cc/inboundConnection/search`.
async fn describe_inbound(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/cc/inboundConnection/search"),
            Some(body),
        )
        .await
}

/// The members `InboundConnection` does NOT model, so an answer that carried one
/// would be reporting a member the shape has no place for. The alias and the
/// connection properties belong to the SOURCE owner's view of the connection
/// (`smithy_query(query="shape",shape="InboundConnection")` lists exactly
/// `LocalDomainInfo`, `RemoteDomainInfo`, `ConnectionId`, `ConnectionStatus` and
/// `ConnectionMode`).
const UNMODELLED_MEMBERS: [&str; 2] = ["ConnectionAlias", "ConnectionProperties"];

/// One connection to create: what makes this row differ from its siblings, the
/// `ConnectionMode` to send (absent where `None`) with the mode expected back, the
/// `ConnectionProperties` to send, and the source reference to send with the one
/// expected back.
struct Row {
    case: &'static str,
    alias: &'static str,
    local: Value,
    expected_local: Value,
    mode: Option<&'static str>,
    expected_mode: &'static str,
    properties: Option<Value>,
}

/// The table of connections whose inbound view is asserted: the mode omitted and
/// named either way, no properties / an endpoint / a cross-cluster-search property,
/// and a source reference naming its account and Region in full, partly or not at
/// all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source reference naming only the domain, and no mode",
            alias: "inbound-plain",
            local: domain_info("in-source-a", None, None),
            expected_local: domain_info("in-source-a", Some(ACCOUNT), Some(REGION)),
            mode: None,
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: None,
        },
        Row {
            case: "a source in another account and Region, mode named",
            alias: "inbound-cross-account",
            local: domain_info("in-source-b", Some("210987654321"), Some("eu-west-1")),
            expected_local: domain_info("in-source-b", Some("210987654321"), Some("eu-west-1")),
            mode: Some(CONNECTION_MODE_DIRECT),
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "ENABLED" } })),
        },
        Row {
            case: "the other connection mode, with the endpoint property",
            alias: "inbound-vpc",
            local: domain_info("in-source-c", None, Some("ap-south-1")),
            expected_local: domain_info("in-source-c", Some(ACCOUNT), Some("ap-south-1")),
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            expected_mode: CONNECTION_MODE_VPC_ENDPOINT,
            properties: Some(json!({ "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com" })),
        },
        Row {
            case: "a source naming the owner but not the Region",
            alias: "inbound-owner-only",
            local: domain_info("in-source-d", Some("210987654321"), None),
            expected_local: domain_info("in-source-d", Some("210987654321"), Some(REGION)),
            mode: Some(CONNECTION_MODE_DIRECT),
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "DISABLED" } })),
        },
    ]
}

/// The body of a create built from a row, connecting the row's source domain to
/// `remote`.
fn create_body(row: &Row, remote: &str) -> Value {
    let mut body = json!({
        "ConnectionAlias": row.alias,
        "LocalDomainInfo": row.local,
        "RemoteDomainInfo": domain_info(remote, None, None),
    });
    if let Some(mode) = row.mode {
        body["ConnectionMode"] = json!(mode);
    }
    if let Some(properties) = &row.properties {
        body["ConnectionProperties"] = properties.clone();
    }
    body
}

/// Create one connection and answer its `ConnectionId`, failing the named case when
/// the create itself is refused.
async fn created_connection(emulator: &Emulator, case: &str, body: Value) -> String {
    let answer = create_connection(emulator, body).await;
    assert_eq!(
        answer.status, 200,
        "case CreateOutboundConnection({case}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ConnectionId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateOutboundConnection({case}): expected a ConnectionId, got {:?}",
                answer.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn the_inbound_report_is_the_destination_owners_view_of_the_stored_connection() {
    let emulator = Emulator::new();
    created_domain(&emulator, "in-destination").await;
    for source in ["in-source-a", "in-source-b", "in-source-c", "in-source-d"] {
        created_domain(&emulator, source).await;
    }

    let table = rows();
    let mut created: Vec<(&str, String)> = Vec::new();
    for row in &table {
        let id = created_connection(&emulator, row.case, create_body(row, "in-destination")).await;
        created.push((row.case, id));
    }

    // Nothing but the clock brings a validated request into the destination owner's
    // view; the suite asks for no acceptance and no second create.
    tokio::time::sleep(SETTLE_WAIT).await;

    let report = describe_inbound(&emulator, json!({})).await;
    assert_eq!(
        report.status, 200,
        "case DescribeInboundConnections(unfiltered): expected 200, got {} with {:?}",
        report.status, report.body
    );
    let outbound = describe_outbound(&emulator, json!({})).await;

    for (row, (case, id)) in table.iter().zip(created.iter()) {
        let described = reported(&report, case, id).unwrap_or_else(|| {
            panic!(
                "case {case}: the connection {id:?} must be reported inbound, got {:?}",
                report.body
            )
        });

        assert_eq!(
            described["LocalDomainInfo"], row.expected_local,
            "case {case}: expected the source reference {:?}, got {:?}",
            row.expected_local, described["LocalDomainInfo"]
        );
        assert_eq!(
            described["RemoteDomainInfo"],
            domain_info("in-destination", Some(ACCOUNT), Some(REGION)),
            "case {case}: expected the destination reference resolved in full, got {:?}",
            described["RemoteDomainInfo"]
        );
        assert_eq!(
            described["ConnectionMode"],
            json!(row.expected_mode),
            "case {case}: expected ConnectionMode {:?}, got {:?}",
            row.expected_mode,
            described["ConnectionMode"]
        );
        assert_eq!(
            described["ConnectionStatus"]["StatusCode"],
            json!(CONNECTION_STATUS_PENDING_ACCEPTANCE),
            "case {case}: a validated request waits for the destination owner's decision, got {:?}",
            described["ConnectionStatus"]
        );
        assert!(
            described["ConnectionStatus"]["Message"].is_null(),
            "case {case}: a validated request has nothing to add to its status, got {:?}",
            described["ConnectionStatus"]["Message"]
        );
        for member in UNMODELLED_MEMBERS {
            assert!(
                described[member].is_null(),
                "case {case}: InboundConnection models no {member}, got {:?}",
                described[member]
            );
        }

        // The two views are ONE record: whatever the source owner's report says about
        // the connection's identity, endpoints, mode and status, the destination
        // owner's report says too.
        let other = reported(&outbound, case, id)
            .unwrap_or_else(|| panic!("case {case}: {id:?} must be reported outbound too"));
        for member in [
            "ConnectionId",
            "LocalDomainInfo",
            "RemoteDomainInfo",
            "ConnectionMode",
        ] {
            assert_eq!(
                described[member], other[member],
                "case {case}: the two views of one connection must agree on {member}, got {:?} \
                 inbound and {:?} outbound",
                described[member], other[member]
            );
        }
        assert_eq!(
            described["ConnectionStatus"]["StatusCode"], other["ConnectionStatus"]["StatusCode"],
            "case {case}: the two views of one connection must agree on its status, got {:?} \
             inbound and {:?} outbound",
            described["ConnectionStatus"], other["ConnectionStatus"]
        );
    }

    assert_eq!(
        connections(&report, "unfiltered").len(),
        table.len(),
        "case DescribeInboundConnections(unfiltered): expected exactly the created connections, \
         got {:?}",
        reported_ids(&report, "unfiltered")
    );
    assert!(
        report.body["NextToken"].is_null(),
        "case DescribeInboundConnections(unfiltered): a page that ended the set carries no token, \
         got {:?}",
        report.body["NextToken"]
    );
}

#[tokio::test]
async fn a_connection_has_an_inbound_view_exactly_while_its_status_is_an_inbound_one() {
    let emulator = Emulator::new();
    created_domain(&emulator, "view-source").await;
    created_domain(&emulator, "view-destination").await;
    // A domain that existed and was deleted resolves to nothing, exactly like one
    // that never existed.
    created_domain(&emulator, "view-deleted").await;
    delete_domain(&emulator, "view-deleted").await;

    // One row per way the two references can resolve, which is what decides the
    // status the clock owes the request — and so whether the destination owner ever
    // has anything to decide about it.
    let table: Vec<(&str, &str, &str, &str, bool)> = vec![
        (
            "both references resolve",
            "view-source",
            "view-destination",
            CONNECTION_STATUS_PENDING_ACCEPTANCE,
            true,
        ),
        (
            "the destination resolves to nothing",
            "view-source",
            "view-absent",
            CONNECTION_STATUS_VALIDATION_FAILED,
            false,
        ),
        (
            "the source resolves to nothing",
            "view-nowhere",
            "view-destination",
            CONNECTION_STATUS_VALIDATION_FAILED,
            false,
        ),
        (
            "the destination was deleted",
            "view-source",
            "view-deleted",
            CONNECTION_STATUS_VALIDATION_FAILED,
            false,
        ),
    ];

    let mut created: Vec<(&str, String, &str, bool)> = Vec::new();
    for (index, (case, local, remote, expected_status, has_inbound_view)) in
        table.into_iter().enumerate()
    {
        let id = created_connection(
            &emulator,
            case,
            json!({
                "ConnectionAlias": format!("view-{index}"),
                "LocalDomainInfo": domain_info(local, None, None),
                "RemoteDomainInfo": domain_info(remote, None, None),
            }),
        )
        .await;

        // The transient status is observable, and `VALIDATING` is not one
        // `InboundConnectionStatusCode` carries: a request still being validated is
        // the source owner's alone, so the destination owner's report does not carry
        // it.
        let outbound = describe_outbound(
            &emulator,
            json!({ "Filters": [{ "Name": "connection-id", "Values": [id] }] }),
        )
        .await;
        assert_eq!(
            outbound.body["Connections"][0]["ConnectionStatus"]["StatusCode"],
            json!(CONNECTION_STATUS_VALIDATING),
            "case {case}: expected the request to still be validating, got {:?}",
            outbound.body
        );
        let pending = describe_inbound(&emulator, json!({})).await;
        assert!(
            reported(&pending, case, &id).is_none(),
            "case {case}: a request being validated has no inbound view yet, got {:?}",
            pending.body
        );

        created.push((case, id, expected_status, has_inbound_view));
    }

    // Nothing but the clock moves the outcome in.
    tokio::time::sleep(SETTLE_WAIT).await;
    let report = describe_inbound(&emulator, json!({})).await;
    let outbound = describe_outbound(&emulator, json!({})).await;

    for (case, id, expected_status, has_inbound_view) in created {
        let settled = reported(&outbound, case, &id)
            .unwrap_or_else(|| panic!("case {case}: {id:?} must still be reported outbound"));
        assert_eq!(
            settled["ConnectionStatus"]["StatusCode"],
            json!(expected_status),
            "case {case}: expected the request to settle at {expected_status}, got {:?}",
            settled["ConnectionStatus"]
        );

        let described = reported(&report, case, &id);
        assert_eq!(
            described.is_some(),
            has_inbound_view,
            "case {case}: a connection at {expected_status} must {} an inbound view, got {:?}",
            if has_inbound_view { "have" } else { "not have" },
            report.body
        );
        if let Some(described) = described {
            assert_eq!(
                described["ConnectionStatus"]["StatusCode"],
                json!(expected_status),
                "case {case}: the inbound view reports the status the record is in, got {:?}",
                described["ConnectionStatus"]
            );
        }
    }
}

#[tokio::test]
async fn the_inbound_report_is_narrowed_by_every_documented_filter_name() {
    let emulator = Emulator::new();
    for domain in ["dst-one", "dst-two", "flt-source-a"] {
        created_domain(&emulator, domain).await;
    }

    // Three connections differing in source domain, source account, source Region and
    // destination domain, so each filter name below picks a different subset. Every
    // source reference names a domain the registry resolves, so each connection
    // reaches the status that gives it an inbound view.
    let fixtures: Vec<(&str, Value, &str)> = vec![
        ("to-one", domain_info("flt-source-a", None, None), "dst-one"),
        (
            "to-two",
            domain_info("flt-source-a", Some("210987654321"), Some("eu-west-1")),
            "dst-two",
        ),
        (
            "to-one-again",
            domain_info("dst-two", None, Some("ap-south-1")),
            "dst-one",
        ),
    ];
    let mut ids: Vec<String> = Vec::new();
    for (alias, local, remote) in &fixtures {
        let id = created_connection(
            &emulator,
            alias,
            json!({
                "ConnectionAlias": alias,
                "LocalDomainInfo": local,
                "RemoteDomainInfo": domain_info(remote, None, None),
            }),
        )
        .await;
        ids.push(id);
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    // The filter names AWS documents as available for this operation, each on a value
    // the fixtures above make selective, plus the cases that fix the combining rules.
    let cases: Vec<(&str, Value, Vec<usize>)> = vec![
        ("no filters", json!({}), vec![0, 1, 2]),
        (
            "connection-id",
            json!({ "Filters": [{ "Name": "connection-id", "Values": [ids[1]] }] }),
            vec![1],
        ),
        (
            "local-domain-info.domain-name",
            json!({ "Filters": [{ "Name": "local-domain-info.domain-name", "Values": ["flt-source-a"] }] }),
            vec![0, 1],
        ),
        (
            "local-domain-info.owner-id",
            json!({ "Filters": [{ "Name": "local-domain-info.owner-id", "Values": ["210987654321"] }] }),
            vec![1],
        ),
        (
            "local-domain-info.region",
            json!({ "Filters": [{ "Name": "local-domain-info.region", "Values": ["eu-west-1"] }] }),
            vec![1],
        ),
        (
            "remote-domain-info.domain-name",
            json!({ "Filters": [{ "Name": "remote-domain-info.domain-name", "Values": ["dst-one"] }] }),
            vec![0, 2],
        ),
        // "A cross-cluster connection must match at least one of the specified
        // values": the values of ONE filter are alternatives.
        (
            "two values of one filter",
            json!({ "Filters": [{
                "Name": "local-domain-info.region",
                "Values": [REGION, "ap-south-1"],
            }] }),
            vec![0, 2],
        ),
        // Two filters are cumulative: a connection must match both.
        (
            "two filters",
            json!({ "Filters": [
                { "Name": "local-domain-info.domain-name", "Values": ["flt-source-a"] },
                { "Name": "remote-domain-info.domain-name", "Values": ["dst-one"] },
            ] }),
            vec![0],
        ),
        // A filter no connection matches selects nothing rather than everything.
        (
            "a value nothing carries",
            json!({ "Filters": [{ "Name": "connection-id", "Values": ["cc-nosuchconnection"] }] }),
            vec![],
        ),
        // A name this operation does not filter on is a property no connection has a
        // value for, so it selects nothing. The alias and the destination account are
        // among them: the inbound report narrows by the SOURCE side's account and
        // Region, not the destination's.
        (
            "an undocumented filter name",
            json!({ "Filters": [{ "Name": "connection-alias", "Values": ["to-one"] }] }),
            vec![],
        ),
        (
            "a filter name of the outbound report only",
            json!({ "Filters": [{ "Name": "remote-domain-info.owner-id", "Values": [ACCOUNT] }] }),
            vec![],
        ),
    ];

    for (case, request, expected_rows) in cases {
        let answer = describe_inbound(&emulator, request).await;
        assert_eq!(
            answer.status, 200,
            "case DescribeInboundConnections({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let mut got = reported_ids(&answer, case);
        got.sort();
        let mut expected: Vec<String> = expected_rows.iter().map(|row| ids[*row].clone()).collect();
        expected.sort();
        assert_eq!(
            got, expected,
            "case DescribeInboundConnections({case}): expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn inbound_paging_walks_the_whole_set_exactly_once() {
    let emulator = Emulator::new();
    created_domain(&emulator, "paged-in-source").await;
    created_domain(&emulator, "paged-in-destination").await;

    for index in 0..5 {
        created_connection(
            &emulator,
            &format!("page-{index}"),
            json!({
                "ConnectionAlias": format!("in-page-{index}"),
                "LocalDomainInfo": domain_info("paged-in-source", None, None),
                "RemoteDomainInfo": domain_info("paged-in-destination", None, None),
            }),
        )
        .await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    let whole = describe_inbound(&emulator, json!({})).await;
    let everything = reported_ids(&whole, "unpaged");
    assert_eq!(
        everything.len(),
        5,
        "case DescribeInboundConnections(unpaged): expected five connections, got {everything:?}"
    );

    // Every page size from one to past the whole set, plus the model's own maximum:
    // the pages concatenate to the unpaged answer exactly once however the set is cut.
    for size in [1, 2, 3, 4, 5, 6, 100] {
        let case = format!("MaxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = describe_inbound(&emulator, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {pages}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = reported_ids(&answer, &case);
            assert!(
                page.len() <= size,
                "case {case} page {pages}: a page is never longer than the size asked for, got \
                 {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= everything.len() + 1,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            match answer.body["NextToken"].as_str() {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            walked, everything,
            "case {case}: the pages must walk the whole set exactly once, got {walked:?}"
        );
    }

    // A token that names no place in the set — one this service never minted — is
    // refused rather than silently answered from the start.
    for token in ["cc-nosuchconnection", "cc-000000000000"] {
        let case = format!("NextToken={token:?}");
        let answer = describe_inbound(&emulator, json!({ "NextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "InvalidPaginationTokenException",
            "case {case}: expected InvalidPaginationTokenException, got {:?}",
            answer.body
        );
    }
}
