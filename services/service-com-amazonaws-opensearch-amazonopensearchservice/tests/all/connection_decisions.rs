// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the other three decisions on a cross-cluster connection:
//! `RejectInboundConnection`, `DeleteInboundConnection` and
//! `DeleteOutboundConnection`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`], the connection request helpers from
//! [`super::cross_cluster_connections`] and the inbound report from
//! [`super::inbound_connections`]; this suite adds no second copy of any of them.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * rejecting a request that waits for a decision moves the ONE stored record both
//!   views report into its refused lifecycle, which it then walks on the clock alone
//!   and comes to rest in — over a table of connections differing in connection mode,
//!   in connection properties and in how fully the source reference names its account
//!   and Region;
//! * a request that is not waiting for a decision cannot be rejected, and an id that
//!   addresses no connection the destination owner has a view of is the modelled
//!   missing-resource refusal that names the id;
//! * a delete puts the one record into its deleting status whichever owner asks for
//!   it, the clock then owes it the deleted status and the store's release of the
//!   record, and afterwards the id addresses nothing from either side — over a table
//!   spanning both delete operations and every status a connection can be deleted
//!   from;
//! * the destination owner can delete only what it has a view of, while the source
//!   owner has a view of its connection from the moment the create returns;
//! * releasing the record releases what it held: the source domain can connect to the
//!   same destination under the same alias again.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONNECTION_MODE_DIRECT, CONNECTION_MODE_VPC_ENDPOINT, CONNECTION_RETENTION,
    CONNECTION_STATUS_ACTIVE, CONNECTION_STATUS_DELETED, CONNECTION_STATUS_DELETING,
    CONNECTION_STATUS_PENDING_ACCEPTANCE, CONNECTION_STATUS_REJECTED, CONNECTION_STATUS_REJECTING,
    CONNECTION_STATUS_VALIDATING,
};

use super::cross_cluster_connections::{
    connections, create_connection, created_domain, describe_connections as describe_outbound,
    domain_info, reported, ACCOUNT, REGION,
};
use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::inbound_connections::describe_inbound;

/// The statuses a refused request holds, in the order its lifecycle visits them:
/// the refusal being carried out, then the refused request at rest. From
/// `InboundConnectionStatusCode`'s own documentation
/// (`smithy_query(query="doc",shape="InboundConnectionStatus",member="StatusCode")`):
/// `REJECTING` — "Inbound connection rejection is in process" — then `REJECTED`.
const REJECTED_CHAIN: [&str; 2] = [CONNECTION_STATUS_REJECTING, CONNECTION_STATUS_REJECTED];

/// How long the suite polls for a change the clock owes before giving up, and how
/// long it waits between polls. The longest span owed here is the store's release of
/// a deleted record, so the budget is well past [`CONNECTION_RETENTION`].
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `RejectInboundConnection`: `PUT /opensearch/cc/inboundConnection/{ConnectionId}/reject`.
async fn reject(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/cc/inboundConnection/{id}/reject"),
            None,
        )
        .await
}

/// `AcceptInboundConnection`, so a case can reach a connection the destination owner
/// has already decided about.
async fn accept(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/cc/inboundConnection/{id}/accept"),
            None,
        )
        .await
}

/// `DeleteInboundConnection`: `DELETE /opensearch/cc/inboundConnection/{ConnectionId}`.
async fn delete_inbound(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/cc/inboundConnection/{id}"),
            None,
        )
        .await
}

/// `DeleteOutboundConnection`: `DELETE /opensearch/cc/outboundConnection/{ConnectionId}`.
async fn delete_outbound(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/cc/outboundConnection/{id}"),
            None,
        )
        .await
}

/// One filter selecting exactly the connection `id` names.
fn by_id(id: &str) -> Value {
    json!({ "Filters": [{ "Name": "connection-id", "Values": [id] }] })
}

/// One reported connection's `ConnectionStatus.StatusCode`.
fn status_code(connection: &Value, case: &str) -> String {
    connection["ConnectionStatus"]["StatusCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a StatusCode, got {connection:?}"))
        .to_owned()
}

/// The status the destination owner's report gives `id`, or `None` where that report
/// does not carry the connection.
async fn inbound_status(emulator: &Emulator, case: &str, id: &str) -> Option<String> {
    let answer = describe_inbound(emulator, by_id(id)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeInboundConnections to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    reported(&answer, case, id).map(|connection| status_code(&connection, case))
}

/// The status the source owner's report gives `id`, or `None` where that report does
/// not carry the connection.
async fn outbound_status(emulator: &Emulator, case: &str, id: &str) -> Option<String> {
    let answer = describe_outbound(emulator, by_id(id)).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeOutboundConnections to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    reported(&answer, case, id).map(|connection| status_code(&connection, case))
}

/// A failed answer's `message`.
fn error_message(answer: &Answer) -> String {
    answer.body["message"].as_str().unwrap_or_default().to_owned()
}

/// Create one connection and answer its `ConnectionId`.
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
        .to_owned()
}

/// Create a connection from `source` to `destination` under `alias` and leave it
/// waiting for the destination owner's decision.
async fn pending_connection(
    emulator: &Emulator,
    case: &str,
    alias: &str,
    source: &str,
    destination: &str,
) -> String {
    let id = created_connection(
        emulator,
        case,
        json!({
            "ConnectionAlias": alias,
            "LocalDomainInfo": domain_info(source, None, None),
            "RemoteDomainInfo": domain_info(destination, None, None),
        }),
    )
    .await;
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        inbound_status(emulator, case, &id).await.as_deref(),
        Some(CONNECTION_STATUS_PENDING_ACCEPTANCE),
        "case {case}: the request must be waiting for the destination owner's decision"
    );
    id
}

/// Poll the destination owner's report until the refused request has come to rest,
/// asserting it only ever moves FORWARD along [`REJECTED_CHAIN`].
async fn walks_to_rejected(emulator: &Emulator, case: &str, id: &str) {
    let mut reached = 0usize;
    for _ in 0..60 {
        let status = inbound_status(emulator, case, id)
            .await
            .unwrap_or_else(|| panic!("case {case}: a refused request stays reported"));
        let at = REJECTED_CHAIN
            .iter()
            .position(|step| *step == status)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: a refused request only holds a status of {REJECTED_CHAIN:?}, got \
                     {status:?}"
                )
            });
        assert!(
            at >= reached,
            "case {case}: a refused request never moves back along its lifecycle, got {status:?} \
             after {:?}",
            REJECTED_CHAIN[reached]
        );
        reached = at;
        if status == CONNECTION_STATUS_REJECTED {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case}: a refused request must come to rest at {CONNECTION_STATUS_REJECTED}");
}

/// Poll both reports until neither carries `id` any more, which is the store having
/// released the record a delete asked it to.
async fn walks_to_released(emulator: &Emulator, case: &str, id: &str) {
    let budget = CONNECTION_RETENTION + SETTLE_WAIT + SETTLE_WAIT;
    let started = std::time::Instant::now();
    while started.elapsed() < budget {
        let inbound = inbound_status(emulator, case, id).await;
        let outbound = outbound_status(emulator, case, id).await;
        if inbound.is_none() && outbound.is_none() {
            return;
        }
        // While the record is still held, the two views of it cannot disagree about
        // where it is.
        if let (Some(inbound), Some(outbound)) = (&inbound, &outbound) {
            assert_eq!(
                inbound, outbound,
                "case {case}: the two views of one connection must agree while it is held"
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case}: a deleted connection must be released by the store within {budget:?}, it is \
         still reported as {:?}",
        outbound_status(emulator, case, id).await
    );
}

/// One connection to reject: what makes this row differ from its siblings, the source
/// domain and the reference sent for it with the one expected back, the
/// `ConnectionMode` sent (absent where `None`) with the mode expected back, and the
/// `ConnectionProperties` sent.
struct Row {
    case: &'static str,
    alias: &'static str,
    source: &'static str,
    local: Value,
    expected_local: Value,
    mode: Option<&'static str>,
    expected_mode: &'static str,
    properties: Option<Value>,
}

/// The table of connections refused: the mode omitted and named either way, no
/// properties / an endpoint / a cross-cluster-search property, and a source reference
/// naming its account and Region in full, partly or not at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source naming only the domain, and no mode",
            alias: "reject-plain",
            source: "rej-source-a",
            local: domain_info("rej-source-a", None, None),
            expected_local: domain_info("rej-source-a", Some(ACCOUNT), Some(REGION)),
            mode: None,
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: None,
        },
        Row {
            case: "a source in another account and Region, mode named",
            alias: "reject-cross-account",
            source: "rej-source-b",
            local: domain_info("rej-source-b", Some("210987654321"), Some("eu-west-1")),
            expected_local: domain_info("rej-source-b", Some("210987654321"), Some("eu-west-1")),
            mode: Some(CONNECTION_MODE_DIRECT),
            expected_mode: CONNECTION_MODE_DIRECT,
            properties: Some(json!({ "CrossClusterSearch": { "SkipUnavailable": "DISABLED" } })),
        },
        Row {
            case: "the other connection mode, with the endpoint property",
            alias: "reject-vpc",
            source: "rej-source-c",
            local: domain_info("rej-source-c", None, Some("ap-south-1")),
            expected_local: domain_info("rej-source-c", Some(ACCOUNT), Some("ap-south-1")),
            mode: Some(CONNECTION_MODE_VPC_ENDPOINT),
            expected_mode: CONNECTION_MODE_VPC_ENDPOINT,
            properties: Some(json!({ "Endpoint": "vpc-remote.eu-west-1.es.amazonaws.com" })),
        },
    ]
}

#[tokio::test]
async fn rejecting_a_pending_request_moves_the_one_record_both_views_report() {
    let emulator = Emulator::new();
    created_domain(&emulator, "rej-destination").await;
    let table = rows();
    for row in &table {
        created_domain(&emulator, row.source).await;
    }

    let mut created: Vec<String> = Vec::new();
    for row in &table {
        let mut body = json!({
            "ConnectionAlias": row.alias,
            "LocalDomainInfo": row.local,
            "RemoteDomainInfo": domain_info("rej-destination", None, None),
        });
        if let Some(mode) = row.mode {
            body["ConnectionMode"] = json!(mode);
        }
        if let Some(properties) = &row.properties {
            body["ConnectionProperties"] = properties.clone();
        }
        created.push(created_connection(&emulator, row.case, body).await);
    }

    // Nothing but the clock brings a validated request to the point of being decided.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (row, id) in table.iter().zip(created.iter()) {
        let case = row.case;
        let answer = reject(&emulator, id).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected RejectInboundConnection to answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let refused = &answer.body["Connection"];
        assert_eq!(
            refused["ConnectionId"],
            json!(id),
            "case {case}: expected the answer to carry the refused connection {id:?}, got {:?}",
            answer.body
        );
        // Carrying the refusal out is work the service does, and the span is never
        // zero: a caller acting straight after the reject sees it in process.
        assert_eq!(
            status_code(refused, case),
            CONNECTION_STATUS_REJECTING,
            "case {case}: expected a refused request at {CONNECTION_STATUS_REJECTING}, got {:?}",
            refused["ConnectionStatus"]
        );
        assert_eq!(
            refused["LocalDomainInfo"], row.expected_local,
            "case {case}: expected the source reference {:?}, got {:?}",
            row.expected_local, refused["LocalDomainInfo"]
        );
        assert_eq!(
            refused["ConnectionMode"],
            json!(row.expected_mode),
            "case {case}: expected ConnectionMode {:?}, got {:?}",
            row.expected_mode,
            refused["ConnectionMode"]
        );
        assert_eq!(
            outbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_REJECTING),
            "case {case}: the source owner's report must carry the same decision"
        );
    }

    for (row, id) in table.iter().zip(created.iter()) {
        walks_to_rejected(&emulator, row.case, id).await;
        assert_eq!(
            outbound_status(&emulator, row.case, id).await.as_deref(),
            Some(CONNECTION_STATUS_REJECTED),
            "case {}: the two views of one connection must agree it was refused",
            row.case
        );
    }

    // A refusal is a decision, not a removal: the record of it stays reported, and
    // the decision cannot be taken again.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (row, id) in table.iter().zip(created.iter()) {
        let case = row.case;
        assert_eq!(
            inbound_status(&emulator, case, id).await.as_deref(),
            Some(CONNECTION_STATUS_REJECTED),
            "case {case}: a refused request stays reported at rest"
        );
        for (operation, answer) in [
            ("RejectInboundConnection", reject(&emulator, id).await),
            ("AcceptInboundConnection", accept(&emulator, id).await),
        ] {
            assert_eq!(
                answer.status, 409,
                "case {case}: expected {operation} on a refused request to answer 409, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "DisabledOperationException",
                "case {case}: expected {operation} to answer DisabledOperationException, got {:?}",
                answer.body
            );
            assert_eq!(
                error_message(&answer),
                format!(
                    "Connection {id} is in {CONNECTION_STATUS_REJECTED} state, not \
                     {CONNECTION_STATUS_PENDING_ACCEPTANCE}"
                ),
                "case {case}: expected {operation} to name the status the connection is in, got \
                 {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn an_id_addressing_no_pending_inbound_request_cannot_be_rejected() {
    let emulator = Emulator::new();
    created_domain(&emulator, "rjm-source").await;
    created_domain(&emulator, "rjm-destination").await;

    // A request still being validated and one whose validation failed: neither has
    // reached the destination owner, so neither is a request it can refuse.
    let validating = created_connection(
        &emulator,
        "a request still being validated",
        json!({
            "ConnectionAlias": "rjm-validating",
            "LocalDomainInfo": domain_info("rjm-source", None, None),
            "RemoteDomainInfo": domain_info("rjm-destination", None, None),
        }),
    )
    .await;
    let failed = created_connection(
        &emulator,
        "a request whose validation failed",
        json!({
            "ConnectionAlias": "rjm-failed",
            "LocalDomainInfo": domain_info("rjm-source", None, None),
            "RemoteDomainInfo": domain_info("rjm-absent", None, None),
        }),
    )
    .await;

    // Well-formed ids addressing nothing — `ConnectionId` is `@length(min: 10, max:
    // 256)` with pattern `^[a-z][a-z0-9\-]+$`, and the last is at the published
    // minimum length — plus the two requests above.
    let mut table: Vec<(String, String)> = ["cc-nosuchconnection", "cc-000000000000", "cc-1234567"]
        .iter()
        .map(|id| {
            (
                format!("an id this service never minted ({id})"),
                (*id).to_owned(),
            )
        })
        .collect();
    table.push((
        "a request still being validated".to_owned(),
        validating.clone(),
    ));
    table.push(("a request whose validation failed".to_owned(), failed));

    for (case, id) in &table {
        let answer = reject(&emulator, id).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            error_message(&answer),
            format!("Connection ID {id} does not exist"),
            "case {case}: expected the refusal to name the id that resolved to nothing, got {:?}",
            answer.body
        );
    }

    // The refusal is about the STATUS, not about the id: the same request is refusable
    // once the clock has validated it.
    tokio::time::sleep(SETTLE_WAIT).await;
    let answer = reject(&emulator, &validating).await;
    assert_eq!(
        answer.status, 200,
        "case a validated request under the same id: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// One delete to make: what makes this row differ, the alias to connect under, the
/// decision to take before deleting (none where `None`), the status the connection is
/// expected to be deleted FROM, and whether the destination owner or the source owner
/// asks for the delete.
struct DeleteRow {
    case: &'static str,
    alias: &'static str,
    decide: Option<&'static str>,
    deleted_from: &'static str,
    inbound: bool,
}

/// Both delete operations, over every status a connection can be deleted from: one
/// waiting for a decision, one in use and one that was refused.
fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "the source owner deletes a request waiting for a decision",
            alias: "del-pending-outbound",
            decide: None,
            deleted_from: CONNECTION_STATUS_PENDING_ACCEPTANCE,
            inbound: false,
        },
        DeleteRow {
            case: "the destination owner deletes a request waiting for its decision",
            alias: "del-pending-inbound",
            decide: None,
            deleted_from: CONNECTION_STATUS_PENDING_ACCEPTANCE,
            inbound: true,
        },
        DeleteRow {
            case: "the source owner deletes a connection in use",
            alias: "del-active-outbound",
            decide: Some("accept"),
            deleted_from: CONNECTION_STATUS_ACTIVE,
            inbound: false,
        },
        DeleteRow {
            case: "the destination owner deletes a connection in use",
            alias: "del-active-inbound",
            decide: Some("accept"),
            deleted_from: CONNECTION_STATUS_ACTIVE,
            inbound: true,
        },
        DeleteRow {
            case: "the destination owner deletes a request it refused",
            alias: "del-rejected-inbound",
            decide: Some("reject"),
            deleted_from: CONNECTION_STATUS_REJECTED,
            inbound: true,
        },
        DeleteRow {
            case: "the source owner deletes a request that was refused",
            alias: "del-rejected-outbound",
            decide: Some("reject"),
            deleted_from: CONNECTION_STATUS_REJECTED,
            inbound: false,
        },
    ]
}

#[tokio::test]
async fn a_delete_puts_the_one_record_on_its_way_out_of_the_store() {
    let emulator = Emulator::new();
    created_domain(&emulator, "del-source").await;
    created_domain(&emulator, "del-destination").await;

    for row in delete_rows() {
        let case = row.case;
        let id = pending_connection(&emulator, case, row.alias, "del-source", "del-destination")
            .await;

        // Reach the status this row deletes the connection from, through the operation
        // that takes that decision and the clock that carries it out.
        match row.decide {
            None => {}
            Some("accept") => {
                let answer = accept(&emulator, &id).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the acceptance to be taken, got {} with {:?}",
                    answer.status, answer.body
                );
            }
            Some(_) => {
                let answer = reject(&emulator, &id).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the refusal to be taken, got {} with {:?}",
                    answer.status, answer.body
                );
                walks_to_rejected(&emulator, case, &id).await;
            }
        }
        if row.deleted_from == CONNECTION_STATUS_ACTIVE {
            // The accepted connection walks to the status this row deletes it from.
            for _ in 0..60 {
                if outbound_status(&emulator, case, &id).await.as_deref()
                    == Some(CONNECTION_STATUS_ACTIVE)
                {
                    break;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
        assert_eq!(
            outbound_status(&emulator, case, &id).await.as_deref(),
            Some(row.deleted_from),
            "case {case}: the connection must be at {} before it is deleted",
            row.deleted_from
        );

        let answer = if row.inbound {
            delete_inbound(&emulator, &id).await
        } else {
            delete_outbound(&emulator, &id).await
        };
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the delete to answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let deleted = &answer.body["Connection"];
        assert_eq!(
            deleted["ConnectionId"],
            json!(id),
            "case {case}: expected the answer to carry the deleted connection {id:?}, got {:?}",
            answer.body
        );
        // Taking the connection down is work the service does, so the answer reports
        // the deletion in progress rather than a record already gone.
        assert_eq!(
            status_code(deleted, case),
            CONNECTION_STATUS_DELETING,
            "case {case}: expected a deleted connection at {CONNECTION_STATUS_DELETING}, got {:?}",
            deleted["ConnectionStatus"]
        );
        // The source owner's view is the same record: a delete by either owner is the
        // one connection going away.
        assert_eq!(
            outbound_status(&emulator, case, &id).await.as_deref(),
            Some(CONNECTION_STATUS_DELETING),
            "case {case}: both views must report the one connection on its way out"
        );

        // The clock owes the record the deleted status it comes to rest in, and then
        // the store's release of it.
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_eq!(
            inbound_status(&emulator, case, &id).await.as_deref(),
            Some(CONNECTION_STATUS_DELETED),
            "case {case}: a deleted connection is reported at {CONNECTION_STATUS_DELETED} before \
             the store releases it"
        );
        walks_to_released(&emulator, case, &id).await;

        // Once the record is released the id addresses nothing, from either side.
        for (operation, answer) in [
            ("DeleteInboundConnection", delete_inbound(&emulator, &id).await),
            (
                "DeleteOutboundConnection",
                delete_outbound(&emulator, &id).await,
            ),
            ("AcceptInboundConnection", accept(&emulator, &id).await),
            ("RejectInboundConnection", reject(&emulator, &id).await),
        ] {
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case}: expected {operation} on a released connection to answer \
                 ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                error_message(&answer),
                format!("Connection ID {id} does not exist"),
                "case {case}: expected {operation} to name the id that resolved to nothing, got \
                 {:?}",
                answer.body
            );
        }

        // Releasing the record releases what it held: the same source domain can
        // connect to the same destination under the same alias again, which it could
        // not while the record was in the store.
        let again = created_connection(
            &emulator,
            case,
            json!({
                "ConnectionAlias": row.alias,
                "LocalDomainInfo": domain_info("del-source", None, None),
                "RemoteDomainInfo": domain_info("del-destination", None, None),
            }),
        )
        .await;
        assert_ne!(
            again, id,
            "case {case}: a connection created after the release is a new record, not the old id"
        );
        let answer = delete_outbound(&emulator, &again).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the replacement connection to be deletable, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn each_owner_deletes_only_the_connections_it_has_a_view_of() {
    let emulator = Emulator::new();
    created_domain(&emulator, "view-del-source").await;
    created_domain(&emulator, "view-del-destination").await;

    // A request still being validated: the source owner asked for it, so it is in
    // that owner's report from the moment the create returned, while the destination
    // owner has nothing to see or delete yet.
    let id = created_connection(
        &emulator,
        "a request still being validated",
        json!({
            "ConnectionAlias": "view-del-validating",
            "LocalDomainInfo": domain_info("view-del-source", None, None),
            "RemoteDomainInfo": domain_info("view-del-destination", None, None),
        }),
    )
    .await;
    let case = "a request still being validated";
    assert_eq!(
        outbound_status(&emulator, case, &id).await.as_deref(),
        Some(CONNECTION_STATUS_VALIDATING),
        "case {case}: the source owner sees its own request while it is validated"
    );
    assert!(
        inbound_status(&emulator, case, &id).await.is_none(),
        "case {case}: the destination owner has no view of a request being validated"
    );

    let refused = delete_inbound(&emulator, &id).await;
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case {case}: expected DeleteInboundConnection to answer ResourceNotFoundException, got \
         {:?}",
        refused.body
    );
    assert_eq!(
        error_message(&refused),
        format!("Connection ID {id} does not exist"),
        "case {case}: expected the refusal to name the id, got {:?}",
        refused.body
    );
    // The refused delete changed nothing.
    assert_eq!(
        outbound_status(&emulator, case, &id).await.as_deref(),
        Some(CONNECTION_STATUS_VALIDATING),
        "case {case}: a refused delete leaves the record where it was"
    );

    // The source owner's delete of the same request is served.
    let answer = delete_outbound(&emulator, &id).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DeleteOutboundConnection to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        status_code(&answer.body["Connection"], case),
        CONNECTION_STATUS_DELETING,
        "case {case}: expected the deletion to be in progress, got {:?}",
        answer.body
    );
    walks_to_released(&emulator, case, &id).await;

    // A request whose validation failed is the source owner's alone too, and the
    // report of the whole set is left empty once it is released.
    let failed = created_connection(
        &emulator,
        "a request whose validation failed",
        json!({
            "ConnectionAlias": "view-del-failed",
            "LocalDomainInfo": domain_info("view-del-source", None, None),
            "RemoteDomainInfo": domain_info("view-del-absent", None, None),
        }),
    )
    .await;
    let case = "a request whose validation failed";
    tokio::time::sleep(SETTLE_WAIT).await;
    let refused = delete_inbound(&emulator, &failed).await;
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case {case}: expected DeleteInboundConnection to answer ResourceNotFoundException, got \
         {:?}",
        refused.body
    );
    let answer = delete_outbound(&emulator, &failed).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DeleteOutboundConnection to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    walks_to_released(&emulator, case, &failed).await;

    let report = describe_outbound(&emulator, json!({})).await;
    assert!(
        connections(&report, case).is_empty(),
        "case {case}: every connection of this account was released, got {:?}",
        report.body
    );
}
