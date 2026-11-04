// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHO a status-based refusal about a cross-cluster connection is
//! about: the rule that a refusal reporting a connection's current status as the reason
//! names the connection the request addressed, by the `ConnectionId` the request
//! carried, before the status clause.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`], the connection request
//! helpers from [`super::cross_cluster_connections`] and the connection decisions from
//! [`super::connection_decisions`]; this suite adds no second copy of any of them.
//!
//! The table spans what the rule generalises over rather than one recorded call:
//! several refused connections coexist (so a refusal that named a fixed connection, or
//! the wrong one, fails), they differ in connection mode, properties, how fully the
//! source reference names its account and Region, and in whether the SOURCE owner has
//! since deleted its own view of the connection; and each refusal is issued more than
//! once, because a refusal that is a report of the record says the same thing every
//! time.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONNECTION_MODE_DIRECT, CONNECTION_MODE_VPC_ENDPOINT, CONNECTION_STATUS_REJECTED,
};

use super::connection_decisions::{
    connection_change_refusal, created_connection, delete_inbound, delete_outbound, error_message,
    inbound_status, reject, rests_at_deleted, walks_to_rejected,
};
use super::cross_cluster_connections::{created_domain, domain_info, ACCOUNT, REGION};
use super::domain_lifecycle::{error_code, Emulator, SETTLE_WAIT};

/// How this service words a change asked of a connection whose status does not allow
/// it: the connection the request addressed, then the status that is the reason. Stated
/// once for both suites that assert it, in
/// [`super::connection_decisions::connection_change_refusal`].
fn expected_message(connection_id: &str, status: &str) -> String {
    connection_change_refusal(connection_id, status)
}

/// One refused connection: what makes this row differ, the alias to connect under, the
/// source domain and the reference sent for it, the `ConnectionMode` sent (absent where
/// `None`), the `ConnectionProperties` sent, and whether the source owner deletes its
/// own view of the connection before the destination owner's delete is issued.
struct Row {
    case: &'static str,
    alias: &'static str,
    source: &'static str,
    local: Value,
    mode: Option<&'static str>,
    properties: Option<Value>,
    source_deleted: bool,
}

/// The table: the mode omitted and named either way, no properties / a cross-cluster
/// search property / an endpoint, a source reference naming its account and Region in
/// full or not at all, and the connection's source side still held or already deleted.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source naming only the domain, no mode, source side still held",
            alias: "refusal-plain",
            source: "refusal-source-a",
            local: domain_info("refusal-source-a", None, None),
            mode: None,
            properties: None,
            source_deleted: false,
        },
        Row {
            case: "a source naming its account and Region, mode named, source side deleted",
            alias: "refusal-cross-account",
            source: "refusal-source-b",
            local: domain_info("refusal-source-b", Some(ACCOUNT), Some(REGION)),
            mode: Some(CONNECTION_MODE_DIRECT),
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "ENABLED" } })),
            source_deleted: true,
        },
        Row {
            case: "the other connection mode, with the endpoint property",
            alias: "refusal-vpc-endpoint",
            source: "refusal-source-c",
            local: domain_info("refusal-source-c", None, Some("eu-west-1")),
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            properties: Some(json!({ "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com" })),
            source_deleted: false,
        },
        Row {
            case: "a fourth connection, so three others could be named by mistake",
            alias: "refusal-fourth",
            source: "refusal-source-d",
            local: domain_info("refusal-source-d", Some(ACCOUNT), None),
            mode: Some(CONNECTION_MODE_DIRECT),
            properties: None,
            source_deleted: true,
        },
    ]
}

#[tokio::test]
async fn a_status_refusal_names_the_connection_the_request_addressed() {
    let emulator = Emulator::new();
    created_domain(&emulator, "refusal-destination").await;
    let table = rows();
    for row in &table {
        created_domain(&emulator, row.source).await;
    }

    // Four connections the destination owner refuses, so every refusal below has three
    // other connections it could have named instead.
    let mut ids: Vec<String> = Vec::new();
    for row in &table {
        let mut body = json!({
            "ConnectionAlias": row.alias,
            "LocalDomainInfo": row.local,
            "RemoteDomainInfo": domain_info("refusal-destination", None, None),
        });
        if let Some(mode) = row.mode {
            body["ConnectionMode"] = json!(mode);
        }
        if let Some(properties) = &row.properties {
            body["ConnectionProperties"] = properties.clone();
        }
        ids.push(created_connection(&emulator, row.case, body).await);
    }

    // Nothing but the clock brings a validated request to the point of being decided.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (row, id) in table.iter().zip(ids.iter()) {
        let answer = reject(&emulator, id).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected the refusal to be taken, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        walks_to_rejected(&emulator, row.case, id).await;
    }

    // Where the row says so, the SOURCE owner deletes its own view first: that is the
    // sequence a client walking the whole lifecycle takes, and it must not change who
    // the destination owner's refusal is about.
    for (row, id) in table.iter().zip(ids.iter()) {
        if row.source_deleted {
            let answer = delete_outbound(&emulator, id).await;
            assert_eq!(
                answer.status, 200,
                "case {}: expected DeleteOutboundConnection to answer 200, got {} with {:?}",
                row.case, answer.status, answer.body
            );
            rests_at_deleted(&emulator, row.case, id).await;
        }
    }

    for (row, id) in table.iter().zip(ids.iter()) {
        let case = row.case;

        // The status clause is read from the record, so it is asserted from the record:
        // the destination owner's report is what says where the connection is.
        assert_eq!(
            inbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_REJECTED),
            "case {case}: the connection must be the destination owner's record of its \
             refusal before its delete is refused"
        );

        // Twice over: a refusal that reports the record says the same thing every time.
        for round in 1..=2 {
            let refused = delete_inbound(&emulator, id).await;
            assert_eq!(
                refused.status, 400,
                "case {case} (round {round}): expected DeleteInboundConnection to answer 400, \
                 got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case} (round {round}): expected ValidationException, got {:?}",
                refused.body
            );
            let message = error_message(&refused);
            assert_eq!(
                message,
                expected_message(id, CONNECTION_STATUS_REJECTED),
                "case {case} (round {round}): expected the refusal to name the connection the \
                 request addressed and the status it is in, got {message:?}"
            );
            // And it names THIS connection: no other connection's id appears in it.
            for other in ids.iter().filter(|other| *other != id) {
                assert!(
                    !message.contains(other.as_str()),
                    "case {case} (round {round}): expected the refusal to name only {id:?}, it \
                     also names {other:?}: {message:?}"
                );
            }
        }

        // The refused delete left the record exactly where the refusal did.
        assert_eq!(
            inbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_REJECTED),
            "case {case}: a refused delete leaves the record where the refusal left it"
        );
    }
}
