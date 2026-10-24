// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service cross-cluster connection surface:
//! `CreateOutboundConnection` and `DescribeOutboundConnections`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the
//! connection paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a create persists the connection and the report of it is the SAME record, over a
//!   table of five creates differing in alias length and character class, in the
//!   connection mode (named either way and omitted), in the connection properties
//!   (absent, an endpoint, and each `SkipUnavailable` value) and in how fully the
//!   remote domain reference names its account and Region;
//! * a created connection is `VALIDATING`, and the clock owes it the outcome of
//!   validating its references against the domain registry: `PENDING_ACCEPTANCE`
//!   where both resolve and `VALIDATION_FAILED` where either does not, over a table
//!   spanning which reference is unresolvable and how;
//! * every identifier minted satisfies `ConnectionId`'s own modelled constraint, and
//!   no two connections share one;
//! * an alias identifies the remote cluster within ONE source domain: a second create
//!   under an alias that domain already uses is refused and changes nothing, while the
//!   same alias from another source domain is accepted;
//! * `ConnectionAlias` is accepted exactly where its modelled constraint allows it —
//!   each published bound and one past it, plus one row per character class its
//!   pattern refuses;
//! * a reference that names no domain at all is refused rather than stored
//!   half-formed, over every member that carries one;
//! * the report is narrowed by EVERY documented filter name, with the values of one
//!   filter as alternatives and two filters cumulative, and a filter on a property
//!   this operation does not filter on selects nothing;
//! * paging walks the whole set exactly once over every page size from one to past
//!   the set, and a token that names no place in the set is refused with the modelled
//!   `InvalidPaginationTokenException`;
//! * the store is bounded per source domain: creates are accepted up to the quota and
//!   the one past it is refused with the modelled `LimitExceededException`, with the
//!   stored set left at the quota and another source domain unaffected;
//! * and the store is bounded in the AGGREGATE however the caller varies the request:
//!   filling it with a fresh source domain name every time still ends in that same
//!   refusal, with the stored set left at its bound.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONNECTION_MODE_DIRECT, CONNECTION_MODE_VPC_ENDPOINT, CONNECTION_QUOTA,
    CONNECTION_STATUS_PENDING_ACCEPTANCE, CONNECTION_STATUS_VALIDATING,
    CONNECTION_STATUS_VALIDATION_FAILED, CONNECTION_STORE_QUOTA,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// The account every emulated resource belongs to, which is what a domain reference
/// that names no `OwnerId` resolves to.
const ACCOUNT: &str = "000000000000";

/// The Region the test harness signs its requests for, which is what a domain
/// reference that names no `Region` resolves to.
const REGION: &str = "us-east-1";

/// `CreateOutboundConnection`: `POST /opensearch/cc/outboundConnection`.
async fn create_connection(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/cc/outboundConnection"),
            Some(body),
        )
        .await
}

/// `DescribeOutboundConnections`: `POST /opensearch/cc/outboundConnection/search`.
async fn describe_connections(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/cc/outboundConnection/search"),
            Some(body),
        )
        .await
}

/// A domain to connect, created through `CreateDomain` so the test connects domains
/// that exist.
async fn created_domain(emulator: &Emulator, name: &str) {
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
}

/// `DeleteDomain`, so a test can reach a domain reference the registry no longer
/// resolves.
async fn delete_domain(emulator: &Emulator, name: &str) {
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain({name:?}): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
}

/// The `Connections` list of a `DescribeOutboundConnections` answer, with the case
/// named on failure.
fn connections(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["Connections"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a Connections array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `ConnectionId`s of a report, in the order it reported them.
fn reported_ids(answer: &Answer, case: &str) -> Vec<String> {
    connections(answer, case)
        .iter()
        .map(|connection| {
            connection["ConnectionId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a ConnectionId, got {connection:?}")
                })
                .to_string()
        })
        .collect()
}

/// The one reported connection carrying `id`, or `None` when the report does not
/// carry it.
fn reported(answer: &Answer, case: &str, id: &str) -> Option<Value> {
    connections(answer, case)
        .into_iter()
        .find(|connection| connection["ConnectionId"] == json!(id))
}

/// A `DomainInformationContainer` naming `domain`, and optionally the account that
/// owns it and the Region it is in.
fn domain_info(domain: &str, owner: Option<&str>, region: Option<&str>) -> Value {
    let mut info = json!({ "DomainName": domain });
    if let Some(owner) = owner {
        info["OwnerId"] = json!(owner);
    }
    if let Some(region) = region {
        info["Region"] = json!(region);
    }
    json!({ "AWSDomainInformation": info })
}

/// One create to make: the alias, the `ConnectionMode` to send (absent where `None`)
/// and the mode expected back, the `ConnectionProperties` to send, and the remote
/// reference to send with the one expected back.
struct Row {
    alias: String,
    mode: Option<&'static str>,
    expected_mode: &'static str,
    properties: Option<Value>,
    remote: Value,
    expected_remote: Value,
}

impl Row {
    fn body(&self, local: &str) -> Value {
        let mut body = json!({
            "ConnectionAlias": self.alias,
            "LocalDomainInfo": domain_info(local, None, None),
            "RemoteDomainInfo": self.remote,
        });
        if let Some(mode) = self.mode {
            body["ConnectionMode"] = json!(mode);
        }
        if let Some(properties) = &self.properties {
            body["ConnectionProperties"] = properties.clone();
        }
        body
    }
}

/// The create table: one row per way an accepted create differs from another — the
/// alias at each published bound and spanning its character classes, the mode named
/// either way and omitted, no properties / an endpoint / each `SkipUnavailable`
/// value, and a remote reference naming its account and Region in full, partly, or
/// not at all.
///
/// `ConnectionAlias` is `@length(min: 2, max: 100)` with pattern
/// `^[a-zA-Z][a-zA-Z0-9\-\_]+$`, `ConnectionMode` is `DIRECT | VPC_ENDPOINT` and
/// `SkipUnavailableStatus` is `ENABLED | DISABLED` — all from this service's Smithy
/// model (`smithy_query(query="operation",operation="CreateOutboundConnection")`).
fn create_rows() -> Vec<Row> {
    let longest = format!("A{}", "b".repeat(99));
    vec![
        // The shortest alias the constraint allows, no mode (so the connection is
        // created in the mode cross-cluster search uses), no properties, and a remote
        // reference naming only the domain.
        Row {
            alias: "cc".to_string(),
            mode: None,
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: None,
            remote: domain_info("remote-plain", None, None),
            expected_remote: domain_info("remote-plain", Some(ACCOUNT), Some(REGION)),
        },
        // An alias spanning every class the pattern allows, the mode named
        // explicitly, and a cross-account remote in another Region.
        Row {
            alias: "Cross_Cluster-01".to_string(),
            mode: Some(CONNECTION_MODE_DIRECT),
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "ENABLED" } })),
            remote: domain_info("remote-other", Some("210987654321"), Some("eu-west-1")),
            expected_remote: domain_info("remote-other", Some("210987654321"), Some("eu-west-1")),
        },
        // The longest alias the constraint allows, the other connection mode, and the
        // endpoint property that mode is the one for.
        Row {
            alias: longest,
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            expected_mode: CONNECTION_MODE_VPC_ENDPOINT,
            properties: Some(json!({ "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com" })),
            remote: domain_info("remote-vpc", None, Some("ap-south-1")),
            expected_remote: domain_info("remote-vpc", Some(ACCOUNT), Some("ap-south-1")),
        },
        // The other `SkipUnavailable` value, and a remote reference naming the owner
        // but not the Region.
        Row {
            alias: "skip-disabled".to_string(),
            mode: Some(CONNECTION_MODE_DIRECT),
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "DISABLED" } })),
            remote: domain_info("remote-same", Some("210987654321"), None),
            expected_remote: domain_info("remote-same", Some("210987654321"), Some(REGION)),
        },
        // Both properties at once, on the same remote domain a sibling row already
        // connects to: a connection is identified by its alias, not by the pair.
        Row {
            alias: "both_properties".to_string(),
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            expected_mode: CONNECTION_MODE_VPC_ENDPOINT,
            properties: Some(json!({
                "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com",
                "CrossClusterSearch": { "SkipUnavailable": "ENABLED" },
            })),
            remote: domain_info("remote-plain", None, None),
            expected_remote: domain_info("remote-plain", Some(ACCOUNT), Some(REGION)),
        },
    ]
}

/// Whether `id` satisfies `ConnectionId`'s own modelled constraint:
/// `@length(min: 10, max: 256)` with pattern `^[a-z][a-z0-9\-]+$`.
fn is_connection_id(id: &str) -> bool {
    (10..=256).contains(&id.len())
        && id.starts_with(|first: char| first.is_ascii_lowercase())
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

#[tokio::test]
async fn a_created_connection_is_the_connection_the_report_describes() {
    let emulator = Emulator::new();
    created_domain(&emulator, "local-source").await;
    // Every domain the table's rows reference, so each request's references resolve
    // and the connections reach the status a validated request waits in.
    for remote in ["remote-plain", "remote-other", "remote-vpc", "remote-same"] {
        created_domain(&emulator, remote).await;
    }

    let rows = create_rows();
    let mut created: Vec<(String, Value)> = Vec::new();
    for row in &rows {
        let case = format!("CreateOutboundConnection({:?})", row.alias);
        let answer = create_connection(&emulator, row.body("local-source")).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );

        let id = answer.body["ConnectionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ConnectionId, got {:?}", answer.body))
            .to_string();
        assert!(
            is_connection_id(&id),
            "case {case}: the minted ConnectionId must satisfy its modelled constraint, got {id:?}"
        );
        assert!(
            created.iter().all(|(earlier, _)| *earlier != id),
            "case {case}: every connection gets its own id, got {id:?} again"
        );

        assert_eq!(
            answer.body["ConnectionAlias"],
            json!(row.alias),
            "case {case}: expected the alias back, got {:?}",
            answer.body["ConnectionAlias"]
        );
        assert_eq!(
            answer.body["ConnectionMode"],
            json!(row.expected_mode),
            "case {case}: expected ConnectionMode {:?}, got {:?}",
            row.expected_mode,
            answer.body["ConnectionMode"]
        );
        assert_eq!(
            answer.body["ConnectionProperties"],
            row.properties.clone().unwrap_or(Value::Null),
            "case {case}: expected the properties the create carried, got {:?}",
            answer.body["ConnectionProperties"]
        );
        assert_eq!(
            answer.body["LocalDomainInfo"],
            domain_info("local-source", Some(ACCOUNT), Some(REGION)),
            "case {case}: the source reference is resolved in full, got {:?}",
            answer.body["LocalDomainInfo"]
        );
        assert_eq!(
            answer.body["RemoteDomainInfo"], row.expected_remote,
            "case {case}: expected the destination reference {:?}, got {:?}",
            row.expected_remote, answer.body["RemoteDomainInfo"]
        );
        assert_eq!(
            answer.body["ConnectionStatus"]["StatusCode"],
            json!(CONNECTION_STATUS_VALIDATING),
            "case {case}: a connection request starts out being validated, got {:?}",
            answer.body["ConnectionStatus"]
        );

        created.push((id, answer.body.clone()));
    }

    // The clock owes each request the next status once the validation span has
    // passed; nothing but time is asked to move it.
    tokio::time::sleep(SETTLE_WAIT).await;

    let report = describe_connections(&emulator, json!({})).await;
    assert_eq!(
        report.status, 200,
        "case DescribeOutboundConnections(no filters): expected 200, got {} with {:?}",
        report.status, report.body
    );
    assert_eq!(
        connections(&report, "no filters").len(),
        rows.len(),
        "case DescribeOutboundConnections(no filters): expected every created connection, got {:?}",
        report.body
    );

    for (id, answered) in &created {
        let case = format!("DescribeOutboundConnections({id:?})");
        let described = reported(&report, &case, id)
            .unwrap_or_else(|| panic!("case {case}: the created connection must be reported"));

        // The report is the SAME record the create answered with, member for member,
        // with only the status moved on by the clock.
        let mut expected = answered.clone();
        expected["ConnectionStatus"] = json!({ "StatusCode": CONNECTION_STATUS_PENDING_ACCEPTANCE });
        assert_eq!(
            described, expected,
            "case {case}: expected the created record {expected:?}, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_connection_request_is_validated_against_the_domain_registry() {
    let emulator = Emulator::new();
    created_domain(&emulator, "val-source").await;
    created_domain(&emulator, "val-remote").await;
    // A domain that existed and was deleted resolves to nothing, exactly like one
    // that never existed.
    created_domain(&emulator, "val-deleted").await;
    delete_domain(&emulator, "val-deleted").await;

    // One row per way the two references can resolve: both, each one alone, neither,
    // and one naming a domain the registry no longer holds.
    let rows: Vec<(&str, &str, &str, &str, Vec<&str>)> = vec![
        (
            "both resolve",
            "val-source",
            "val-remote",
            CONNECTION_STATUS_PENDING_ACCEPTANCE,
            vec![],
        ),
        (
            "the destination resolves to nothing",
            "val-source",
            "val-absent",
            CONNECTION_STATUS_VALIDATION_FAILED,
            vec!["val-absent"],
        ),
        (
            "the source resolves to nothing",
            "val-nowhere",
            "val-remote",
            CONNECTION_STATUS_VALIDATION_FAILED,
            vec!["val-nowhere"],
        ),
        (
            "neither resolves",
            "val-nowhere",
            "val-absent",
            CONNECTION_STATUS_VALIDATION_FAILED,
            vec!["val-nowhere", "val-absent"],
        ),
        (
            "the destination was deleted",
            "val-source",
            "val-deleted",
            CONNECTION_STATUS_VALIDATION_FAILED,
            vec!["val-deleted"],
        ),
    ];

    let mut created: Vec<(String, &str, Vec<&str>)> = Vec::new();
    for (index, (case, local, remote, expected_status, unresolved)) in rows.into_iter().enumerate() {
        let answer = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": format!("validated-{index}"),
                "LocalDomainInfo": domain_info(local, None, None),
                "RemoteDomainInfo": domain_info(remote, None, None),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case CreateOutboundConnection({case}): a request is accepted and then validated, \
             got {} with {:?}",
            answer.status, answer.body
        );
        // The outcome is not reported before it is due: every create answers that the
        // request is being validated, whatever the outcome will be.
        assert_eq!(
            answer.body["ConnectionStatus"]["StatusCode"],
            json!(CONNECTION_STATUS_VALIDATING),
            "case CreateOutboundConnection({case}): expected the request to be validating, got \
             {:?}",
            answer.body["ConnectionStatus"]
        );
        let id = answer.body["ConnectionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The transient status is observable: a caller reading straight after the
        // create still sees the request being validated.
        let pending = describe_connections(
            &emulator,
            json!({ "Filters": [{ "Name": "connection-id", "Values": [id] }] }),
        )
        .await;
        assert_eq!(
            pending.body["Connections"][0]["ConnectionStatus"]["StatusCode"],
            json!(CONNECTION_STATUS_VALIDATING),
            "case DescribeOutboundConnections({case}, before the outcome is due): expected the \
             request to still be validating, got {:?}",
            pending.body
        );

        created.push((id, expected_status, unresolved));
    }

    // Nothing but the clock moves the outcome in.
    tokio::time::sleep(SETTLE_WAIT).await;
    let report = describe_connections(&emulator, json!({})).await;

    for (id, expected_status, unresolved) in created {
        let case = format!("DescribeOutboundConnections({id:?})");
        let described = reported(&report, &case, &id)
            .unwrap_or_else(|| panic!("case {case}: the created connection must be reported"));
        assert_eq!(
            described["ConnectionStatus"]["StatusCode"],
            json!(expected_status),
            "case {case}: expected {expected_status}, got {:?}",
            described["ConnectionStatus"]
        );
        let message = described["ConnectionStatus"]["Message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for domain in &unresolved {
            assert!(
                message.contains(domain),
                "case {case}: a failed validation must name what did not resolve ({domain}), got \
                 {message:?}"
            );
        }
        if unresolved.is_empty() {
            assert!(
                described["ConnectionStatus"]["Message"].is_null(),
                "case {case}: a validated request has nothing to add to its status, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_alias_identifies_one_connection_of_one_source_domain() {
    let emulator = Emulator::new();
    created_domain(&emulator, "alias-source").await;
    created_domain(&emulator, "alias-other").await;

    // Rows differing in alias character class and length: the rule is about the
    // (source domain, alias) pair, not about any one alias.
    for alias in ["dup", "Dup_Alias-2", &"z".repeat(100)] {
        let case = format!("CreateOutboundConnection(alias-source, {alias:?})");
        let body = json!({
            "ConnectionAlias": alias,
            "LocalDomainInfo": domain_info("alias-source", None, None),
            "RemoteDomainInfo": domain_info("remote-dup", None, None),
        });
        let first = create_connection(&emulator, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            first.status, first.body
        );
        let id = first.body["ConnectionId"].clone();

        // A second create under the same alias from the same source domain resolves
        // to the connection that already exists, so it is refused.
        let again = create_connection(&emulator, body).await;
        assert_eq!(
            again.status, 409,
            "case {case} twice: expected 409 for an alias already used, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            error_code(&again),
            "ResourceAlreadyExistsException",
            "case {case} twice: expected ResourceAlreadyExistsException, got {:?}",
            again.body
        );

        // The refused create added nothing and left the stored record as the first
        // one wrote it.
        let report = describe_connections(
            &emulator,
            json!({ "Filters": [{ "Name": "local-domain-info.domain-name", "Values": ["alias-source"] }] }),
        )
        .await;
        let held: Vec<Value> = connections(&report, &case)
            .into_iter()
            .filter(|connection| connection["ConnectionAlias"] == json!(alias))
            .collect();
        assert_eq!(
            held.len(),
            1,
            "case {case} twice: expected exactly one record under the alias, got {held:?}"
        );
        assert_eq!(
            held[0]["ConnectionId"], id,
            "case {case} twice: expected the first record's id {id:?}, got {:?}",
            held[0]["ConnectionId"]
        );

        // The SAME alias from a different source domain names a different connection,
        // so it is accepted.
        let elsewhere = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": alias,
                "LocalDomainInfo": domain_info("alias-other", None, None),
                "RemoteDomainInfo": domain_info("remote-dup", None, None),
            }),
        )
        .await;
        assert_eq!(
            elsewhere.status, 200,
            "case CreateOutboundConnection(alias-other, {alias:?}): another source domain may use \
             the alias, got {} with {:?}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.body["ConnectionId"], id,
            "case CreateOutboundConnection(alias-other, {alias:?}): expected its own id, got {id:?}"
        );
    }
}

#[tokio::test]
async fn a_connection_alias_is_accepted_exactly_where_the_modelled_constraint_allows_it() {
    let emulator = Emulator::new();
    created_domain(&emulator, "alias-rules").await;

    // `ConnectionAlias` is `@length(min: 2, max: 100)` with
    // `@pattern("^[a-zA-Z][a-zA-Z0-9\-\_]+$")`: each published bound and one past it,
    // plus one row per character class the pattern refuses.
    let hundred = format!("a{}", "9".repeat(99));
    let hundred_one = format!("a{}", "9".repeat(100));
    let cases: Vec<(&str, bool)> = vec![
        ("ab", true),
        ("A_1", true),
        ("link-to_remote-01", true),
        (hundred.as_str(), true),
        // One past each published length bound.
        ("a", false),
        (hundred_one.as_str(), false),
        // The pattern refuses a leading digit, a leading hyphen, a dot and a space.
        ("0link", false),
        ("-link", false),
        ("link.remote", false),
        ("link remote", false),
    ];

    for (alias, accepted) in cases {
        let case = format!("CreateOutboundConnection({alias:?})");
        let answer = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": alias,
                "LocalDomainInfo": domain_info("alias-rules", None, None),
                "RemoteDomainInfo": domain_info("remote-rules", None, None),
            }),
        )
        .await;
        let report = describe_connections(&emulator, json!({})).await;
        let stored = connections(&report, &case);
        let present = stored
            .iter()
            .any(|connection| connection["ConnectionAlias"] == json!(alias));
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 inside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                present,
                "case {case}: an accepted alias must be persisted, got {stored:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 outside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                answer.body
            );
            assert!(
                !present,
                "case {case}: a refused create must persist nothing, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_reference_that_names_no_domain_is_refused() {
    let emulator = Emulator::new();
    created_domain(&emulator, "ref-source").await;

    // `DomainInformationContainer$AWSDomainInformation` is optional in the model, so
    // the boundary validator passes a reference that names no domain at all. One
    // row per member that carries a reference, and one with both empty.
    let named = domain_info("ref-remote", None, None);
    let rows: Vec<(&str, Value, Value)> = vec![
        ("LocalDomainInfo", json!({}), named.clone()),
        ("RemoteDomainInfo", domain_info("ref-source", None, None), json!({})),
        ("both", json!({}), json!({})),
    ];

    for (member, local, remote) in rows {
        let case = format!("CreateOutboundConnection(empty {member})");
        let answer = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": "empty-reference",
                "LocalDomainInfo": local,
                "RemoteDomainInfo": remote,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
        let report = describe_connections(&emulator, json!({})).await;
        assert!(
            connections(&report, &case).is_empty(),
            "case {case}: a refused create must persist nothing, got {:?}",
            report.body
        );
    }
}

#[tokio::test]
async fn the_report_is_narrowed_by_every_documented_filter_name() {
    let emulator = Emulator::new();
    created_domain(&emulator, "src-alpha").await;
    created_domain(&emulator, "src-beta").await;

    // Three connections differing in source domain, destination domain, owning
    // account and Region, so each filter name below picks a different subset.
    let fixtures: Vec<(&str, &str, Value)> = vec![
        (
            "src-alpha",
            "to-one",
            domain_info("dst-one", None, None),
        ),
        (
            "src-alpha",
            "to-two",
            domain_info("dst-two", Some("210987654321"), Some("eu-west-1")),
        ),
        (
            "src-beta",
            "to-one",
            domain_info("dst-one", None, Some("ap-south-1")),
        ),
    ];
    let mut ids: Vec<String> = Vec::new();
    for (local, alias, remote) in &fixtures {
        let answer = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": alias,
                "LocalDomainInfo": domain_info(local, None, None),
                "RemoteDomainInfo": remote,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case CreateOutboundConnection({local}, {alias}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        ids.push(
            answer.body["ConnectionId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

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
            json!({ "Filters": [{ "Name": "local-domain-info.domain-name", "Values": ["src-alpha"] }] }),
            vec![0, 1],
        ),
        (
            "remote-domain-info.domain-name",
            json!({ "Filters": [{ "Name": "remote-domain-info.domain-name", "Values": ["dst-one"] }] }),
            vec![0, 2],
        ),
        (
            "remote-domain-info.owner-id",
            json!({ "Filters": [{ "Name": "remote-domain-info.owner-id", "Values": ["210987654321"] }] }),
            vec![1],
        ),
        (
            "remote-domain-info.region",
            json!({ "Filters": [{ "Name": "remote-domain-info.region", "Values": ["eu-west-1"] }] }),
            vec![1],
        ),
        // "A cross-cluster connection must match at least one of the specified
        // values": the values of ONE filter are alternatives.
        (
            "two values of one filter",
            json!({ "Filters": [{
                "Name": "remote-domain-info.region",
                "Values": [REGION, "ap-south-1"],
            }] }),
            vec![0, 2],
        ),
        // Two filters are cumulative: a connection must match both.
        (
            "two filters",
            json!({ "Filters": [
                { "Name": "local-domain-info.domain-name", "Values": ["src-alpha"] },
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
        // value for, so it selects nothing.
        (
            "an undocumented filter name",
            json!({ "Filters": [{ "Name": "connection-alias", "Values": ["to-one"] }] }),
            vec![],
        ),
    ];

    for (case, request, expected_rows) in cases {
        let answer = describe_connections(&emulator, request).await;
        assert_eq!(
            answer.status, 200,
            "case DescribeOutboundConnections({case}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let mut reported = reported_ids(&answer, case);
        reported.sort();
        let mut expected: Vec<String> = expected_rows
            .iter()
            .map(|index| ids[*index].clone())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case DescribeOutboundConnections({case}): expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn paging_walks_the_whole_set_exactly_once() {
    let emulator = Emulator::new();
    created_domain(&emulator, "paged-source").await;

    for index in 0..5 {
        let answer = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": format!("page-{index}"),
                "LocalDomainInfo": domain_info("paged-source", None, None),
                "RemoteDomainInfo": domain_info("paged-remote", None, None),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case CreateOutboundConnection(page-{index}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let whole = describe_connections(&emulator, json!({})).await;
    let everything = reported_ids(&whole, "unpaged");
    assert_eq!(
        everything.len(),
        5,
        "case DescribeOutboundConnections(unpaged): expected five connections, got {everything:?}"
    );
    assert!(
        whole.body["NextToken"].is_null(),
        "case DescribeOutboundConnections(unpaged): a page that ended the set carries no token, \
         got {:?}",
        whole.body["NextToken"]
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
            let answer = describe_connections(&emulator, request).await;
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
        let answer = describe_connections(&emulator, json!({ "NextToken": token })).await;
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

#[tokio::test]
async fn the_whole_store_is_bounded_however_the_caller_varies_the_request() {
    let emulator = Emulator::new();

    // The per-source-domain quota partitions on a name the REQUEST carries, and a
    // source domain need not exist for its request to be accepted — so the aggregate
    // bound is the one that has to hold here. This fills the store exactly the way a
    // caller varying the source domain name would: a fresh name every time, none of
    // them a domain the registry holds.
    let per_domain = CONNECTION_QUOTA;
    let sources = CONNECTION_STORE_QUOTA / per_domain;
    for source in 0..sources {
        for index in 0..per_domain {
            let answer = create_connection(
                &emulator,
                json!({
                    "ConnectionAlias": format!("fill-{index}"),
                    "LocalDomainInfo": domain_info(&format!("filler-{source}"), None, None),
                    "RemoteDomainInfo": domain_info("filler-remote", None, None),
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case CreateOutboundConnection(filler-{source}, fill-{index}): expected 200 \
                 inside the store bound, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }

    // One more create, on a source domain name the store holds no connection for at
    // all, is refused: the bound is on the whole store, not on any one name.
    let over = create_connection(
        &emulator,
        json!({
            "ConnectionAlias": "one-too-many",
            "LocalDomainInfo": domain_info("filler-fresh-name", None, None),
            "RemoteDomainInfo": domain_info("filler-remote", None, None),
        }),
    )
    .await;
    assert_eq!(
        over.status, 409,
        "case CreateOutboundConnection(filler-fresh-name): expected 409 past the store bound, \
         got {} with {:?}",
        over.status, over.body
    );
    assert_eq!(
        error_code(&over),
        "LimitExceededException",
        "case CreateOutboundConnection(filler-fresh-name): expected LimitExceededException, got \
         {:?}",
        over.body
    );

    // The refused create added nothing: the store is left at its bound.
    let report = describe_connections(&emulator, json!({ "MaxResults": 100 })).await;
    assert_eq!(
        report.status, 200,
        "case DescribeOutboundConnections(at the store bound): expected 200, got {} with {:?}",
        report.status, report.body
    );
    let mut counted = 0;
    let mut token = report.body["NextToken"].as_str().map(str::to_string);
    counted += reported_ids(&report, "at the store bound").len();
    while let Some(next) = token {
        let page = describe_connections(
            &emulator,
            json!({ "MaxResults": 100, "NextToken": next }),
        )
        .await;
        counted += reported_ids(&page, "at the store bound").len();
        token = page.body["NextToken"].as_str().map(str::to_string);
    }
    assert_eq!(
        counted, CONNECTION_STORE_QUOTA,
        "case DescribeOutboundConnections(at the store bound): expected the store left at its \
         bound of {CONNECTION_STORE_QUOTA}, got {counted}"
    );
}

#[tokio::test]
async fn the_per_source_domain_store_is_bounded_by_its_quota() {
    let emulator = Emulator::new();
    created_domain(&emulator, "bounded-source").await;
    created_domain(&emulator, "neighbour-source").await;

    // Every create up to the quota is accepted — the bound is on the COUNT, so the
    // aliases here differ only in their index.
    for index in 0..CONNECTION_QUOTA {
        let answer = create_connection(
            &emulator,
            json!({
                "ConnectionAlias": format!("quota-{index}"),
                "LocalDomainInfo": domain_info("bounded-source", None, None),
                "RemoteDomainInfo": domain_info("quota-remote", None, None),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case CreateOutboundConnection(quota-{index}): expected 200 inside the quota, got {} \
             with {:?}",
            answer.status, answer.body
        );
    }

    let over = create_connection(
        &emulator,
        json!({
            "ConnectionAlias": "over-the-quota",
            "LocalDomainInfo": domain_info("bounded-source", None, None),
            "RemoteDomainInfo": domain_info("quota-remote", None, None),
        }),
    )
    .await;
    assert_eq!(
        over.status, 409,
        "case CreateOutboundConnection(over-the-quota): expected 409 past the quota, got {} with \
         {:?}",
        over.status, over.body
    );
    assert_eq!(
        error_code(&over),
        "LimitExceededException",
        "case CreateOutboundConnection(over-the-quota): expected LimitExceededException, got {:?}",
        over.body
    );

    // The refused create left the stored set at the quota, and the bound is per
    // source domain: another domain still has its own room.
    let bounded = describe_connections(
        &emulator,
        json!({ "Filters": [{ "Name": "local-domain-info.domain-name", "Values": ["bounded-source"] }] }),
    )
    .await;
    assert_eq!(
        reported_ids(&bounded, "at the quota").len(),
        CONNECTION_QUOTA,
        "case DescribeOutboundConnections(bounded-source): expected the set left at the quota, \
         got {:?}",
        bounded.body
    );

    let neighbour = create_connection(
        &emulator,
        json!({
            "ConnectionAlias": "quota-0",
            "LocalDomainInfo": domain_info("neighbour-source", None, None),
            "RemoteDomainInfo": domain_info("quota-remote", None, None),
        }),
    )
    .await;
    assert_eq!(
        neighbour.status, 200,
        "case CreateOutboundConnection(neighbour-source): another source domain has its own room, \
         got {} with {:?}",
        neighbour.status, neighbour.body
    );
}
