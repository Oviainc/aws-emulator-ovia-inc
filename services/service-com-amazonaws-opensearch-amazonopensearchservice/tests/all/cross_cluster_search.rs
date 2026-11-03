// SPDX-License-Identifier: CC0-1.0

//! CROSS-CLUSTER search through a domain's published REST endpoint, driven over the real
//! wire path: a signed request to the endpoint `DescribeDomain` published, through the
//! multi-service router, into the mounted service.
//!
//! The rule, over the set of index patterns one search may name:
//!
//! > a search that names `<cluster>:<index>` is answered from the REMOTE domain the source
//! > domain's ACTIVE outbound connection carrying that `ConnectionAlias` names — never from
//! > the local cluster's own indexes — and each hit it answers names its index the way a
//! > cross-cluster hit does, `<cluster>:<index>`; a pattern naming an alias the domain has
//! > no active connection for is refused as a remote cluster the local one does not know,
//! > and one naming an index the remote cluster does not hold is refused exactly as a local
//! > index that is absent.
//!
//! The rows span the class: the remote index alone, a local and a remote index in one
//! request, a wildcard across the remote cluster, a query filter applied to the remote
//! documents, a count instead of a search, an index the remote cluster does not hold, an
//! alias nothing connects, and an alias whose connection has not been accepted yet (so it
//! is not `ACTIVE`).
//!
//! The suite also pins the OTHER half of what a client exercising a domain as a search
//! cluster depends on: the documents an index holds stay queryable across a cluster
//! SCALE-OUT (`UpdateDomainConfig` growing the data-node count), both locally and through
//! the connection.
//!
//! The harness is [`super::domain_lifecycle`]'s, and the data-plane request helper is
//! [`super::domain_rest_data_plane`]'s; this suite adds no second copy of either.

use serde_json::{json, Value};

use super::domain_lifecycle::{Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::domain_rest_data_plane::{error_type, rest_prefixed};

/// The alias the accepted connection carries — the name the source domain knows the remote
/// cluster by, and so the prefix a cross-cluster pattern names it with.
const ALIAS: &str = "far";

/// The alias of the connection that is created but never accepted, so it never reaches
/// `ACTIVE`.
const PENDING_ALIAS: &str = "pending";

/// An alias no connection of the source domain carries at all.
const UNCONNECTED_ALIAS: &str = "nowhere";

/// The index both clusters hold under the same name — which is what makes a cross-cluster
/// query that must answer the REMOTE documents observable at all.
const SHARED_INDEX: &str = "catalog";

/// A second index only the remote cluster holds, so a wildcard across that cluster has more
/// than one index to resolve.
const REMOTE_ONLY_INDEX: &str = "catalog-archive";

/// The multi-shard, one-replica index the scale-out case watches: the copies it asks for are
/// what another node has to hold.
const SHARDED_INDEX: &str = "ledger";

/// `CreateDomain` for a domain this suite searches, waited out by the caller.
async fn created_domain(emulator: &Emulator, name: &str) {
    let created = emulator
        .create(json!({
            "DomainName": name,
            "ClusterConfig": { "InstanceType": "m5.large.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
}

/// `CreateOutboundConnection` between two domains of this store, answering with the
/// `ConnectionId` the service minted.
async fn connect(emulator: &Emulator, alias: &str, local: &str, remote: &str) -> String {
    let created = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/cc/outboundConnection"),
            Some(json!({
                "ConnectionAlias": alias,
                "LocalDomainInfo": { "AWSDomainInformation": { "DomainName": local } },
                "RemoteDomainInfo": { "AWSDomainInformation": { "DomainName": remote } },
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateOutboundConnection({alias:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    created.body["ConnectionId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateOutboundConnection({alias:?}): expected a ConnectionId, got {:?}",
                created.body
            )
        })
        .to_string()
}

/// `AcceptInboundConnection`, the destination owner's decision that puts the connection in
/// place.
async fn accept(emulator: &Emulator, connection_id: &str) {
    let accepted = emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/cc/inboundConnection/{connection_id}/accept"),
            None,
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case AcceptInboundConnection({connection_id:?}): expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
}

/// Write one document into `index` of `domain` through that domain's own data plane.
async fn index_document(emulator: &Emulator, domain: &str, index: &str, id: &str, source: Value) {
    let written = rest_prefixed(
        emulator,
        domain,
        "PUT",
        &format!("/{index}/_doc/{id}"),
        Some(source),
    )
    .await;
    assert!(
        written.status == 200 || written.status == 201,
        "case write {domain}/{index}/{id}: expected 200 or 201, got {} with {:?}",
        written.status,
        written.body
    );
}

/// One search to make, and what it must answer.
struct Row {
    case: &'static str,
    /// The index pattern the request names.
    pattern: &'static str,
    /// The query body, or `None` for a search that carries none.
    query: Option<Value>,
    /// The `(index, id)` pairs the answer must carry, in answer order.
    hits: Vec<(&'static str, &'static str)>,
}

/// The search table: one row per way a cross-cluster pattern differs from another.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the remote index alone",
            pattern: "far:catalog",
            query: None,
            hits: vec![
                ("far:catalog", "r1"),
                ("far:catalog", "r2"),
                ("far:catalog", "r3"),
            ],
        },
        Row {
            case: "a local and a remote index in one request",
            pattern: "catalog,far:catalog",
            query: None,
            hits: vec![
                ("catalog", "l1"),
                ("far:catalog", "r1"),
                ("far:catalog", "r2"),
                ("far:catalog", "r3"),
            ],
        },
        Row {
            case: "a wildcard across the remote cluster",
            pattern: "far:catalog*",
            query: None,
            hits: vec![
                ("far:catalog", "r1"),
                ("far:catalog", "r2"),
                ("far:catalog", "r3"),
                ("far:catalog-archive", "a1"),
            ],
        },
        Row {
            case: "a query filter applied to the remote documents",
            pattern: "far:catalog",
            query: Some(json!({ "query": { "term": { "shelf": "east" } } })),
            hits: vec![("far:catalog", "r2")],
        },
        Row {
            case: "a query filter that selects nothing on the remote cluster",
            pattern: "far:catalog",
            query: Some(json!({ "query": { "term": { "shelf": "nowhere" } } })),
            hits: Vec::new(),
        },
    ]
}

/// The `(index, id)` pairs a search answered, in answer order.
fn answered_hits(answer: &Answer, case: &str) -> Vec<(String, String)> {
    answer.body["hits"]["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a hits array, got {:?}", answer.body))
        .iter()
        .map(|hit| {
            (
                hit["_index"].as_str().unwrap_or_default().to_string(),
                hit["_id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Two domains, connected and loaded: the source domain holding one document, the remote
/// one holding three in the shared index and one in an index of its own, plus a second
/// connection left un-accepted.
async fn given_connected_clusters(emulator: &Emulator, local: &str, remote: &str) {
    created_domain(emulator, local).await;
    created_domain(emulator, remote).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let accepted = connect(emulator, ALIAS, local, remote).await;
    let left_pending = connect(emulator, PENDING_ALIAS, local, remote).await;
    // The validation the create owes is materialised by the read the accept takes, so the
    // request is PENDING_ACCEPTANCE by the time the owner decides about it.
    tokio::time::sleep(SETTLE_WAIT).await;
    accept(emulator, &accepted).await;
    // The connection the owner accepted is being put in place; the other one was never
    // decided about, so it stays where the validation left it.
    tokio::time::sleep(SETTLE_WAIT).await;
    let _ = left_pending;

    index_document(
        emulator,
        local,
        SHARED_INDEX,
        "l1",
        json!({ "title": "local", "shelf": "west" }),
    )
    .await;
    for (id, shelf) in [("r1", "west"), ("r2", "east"), ("r3", "north")] {
        index_document(
            emulator,
            remote,
            SHARED_INDEX,
            id,
            json!({ "title": "remote", "shelf": shelf }),
        )
        .await;
    }
    index_document(
        emulator,
        remote,
        REMOTE_ONLY_INDEX,
        "a1",
        json!({ "title": "archived", "shelf": "south" }),
    )
    .await;
}

/// RULE: a search naming `<cluster>:<index>` is answered from the remote domain the source
/// domain's active connection names, and never from the local cluster's own indexes.
#[tokio::test]
async fn a_cross_cluster_search_answers_the_remote_clusters_documents() {
    let emulator = Emulator::new();
    let local = "search-source";
    let remote = "search-remote";
    given_connected_clusters(&emulator, local, remote).await;

    for row in rows() {
        let answer = rest_prefixed(
            &emulator,
            local,
            "POST",
            &format!("/{}/_search", row.pattern),
            row.query.clone(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {} with {:?}",
            row.case, answer.status, answer.body
        );
        let hits = answered_hits(&answer, row.case);
        let expected: Vec<(String, String)> = row
            .hits
            .iter()
            .map(|(index, id)| ((*index).to_string(), (*id).to_string()))
            .collect();
        assert_eq!(
            hits, expected,
            "case {:?}: expected {expected:?}, got {hits:?}",
            row.case
        );
        assert_eq!(
            answer.body["hits"]["total"]["value"].as_u64(),
            Some(expected.len() as u64),
            "case {:?}: expected a total of {}, got {:?}",
            row.case,
            expected.len(),
            answer.body
        );
    }

    // A COUNT over the same pattern counts the remote documents, which is the same
    // resolution read through the other operation that scans them.
    let counted = rest_prefixed(
        &emulator,
        local,
        "POST",
        &format!("/{ALIAS}:{SHARED_INDEX}/_count"),
        None,
    )
    .await;
    assert_eq!(
        counted.body["count"].as_u64(),
        Some(3),
        "case a cross-cluster count: expected 3, got {:?}",
        counted.body
    );
}

/// RULE (the refusals of the same rule): a pattern naming an alias the domain has no
/// ACTIVE connection for is refused as a remote cluster the local one does not know, and
/// one naming an index the remote cluster does not hold is refused exactly as an absent
/// local index is.
#[tokio::test]
async fn a_cross_cluster_search_refuses_a_cluster_or_an_index_it_cannot_resolve() {
    let emulator = Emulator::new();
    let local = "refusal-source";
    let remote = "refusal-remote";
    given_connected_clusters(&emulator, local, remote).await;

    // (case, the pattern, the status, the engine's error type, the reason it states)
    let rows: [(&str, String, u16, &str, String); 4] = [
        (
            "an index the remote cluster does not hold",
            format!("{ALIAS}:absent"),
            404,
            "index_not_found_exception",
            format!("no such index [{ALIAS}:absent]"),
        ),
        (
            "an alias no connection of this domain carries",
            format!("{UNCONNECTED_ALIAS}:{SHARED_INDEX}"),
            400,
            "no_such_remote_cluster_exception",
            format!("no such remote cluster: [{UNCONNECTED_ALIAS}]"),
        ),
        (
            "an alias whose connection has not been accepted",
            format!("{PENDING_ALIAS}:{SHARED_INDEX}"),
            400,
            "no_such_remote_cluster_exception",
            format!("no such remote cluster: [{PENDING_ALIAS}]"),
        ),
        (
            "a local index beside an unresolvable cluster",
            format!("{SHARED_INDEX},{UNCONNECTED_ALIAS}:{SHARED_INDEX}"),
            400,
            "no_such_remote_cluster_exception",
            format!("no such remote cluster: [{UNCONNECTED_ALIAS}]"),
        ),
    ];

    for (case, pattern, status, error, reason) in rows {
        let answer =
            rest_prefixed(&emulator, local, "POST", &format!("/{pattern}/_search"), None).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        let answered = error_type(&answer);
        assert_eq!(
            answered, error,
            "case {case:?}: expected error type {error:?}, got {answered:?}"
        );
        assert_eq!(
            answer.body["error"]["reason"].as_str(),
            Some(reason.as_str()),
            "case {case:?}: expected the reason {reason:?}, got {:?}",
            answer.body
        );
    }
}

/// RULE: a cluster SCALE-OUT is a configuration change and not a data migration, so every
/// document an index held before it is still queryable afterwards — locally and through the
/// connection — and the copies the index asked for are held by the nodes the cluster was
/// scaled to.
#[tokio::test]
async fn documents_stay_queryable_across_a_cluster_scale_out() {
    let emulator = Emulator::new();
    let local = "scale-out-source";
    let remote = "scale-out-remote";
    given_connected_clusters(&emulator, local, remote).await;

    // A multi-shard index with one replica copy per shard: on the single node the domain
    // was created with, every replica is unassigned, which is the health such a cluster
    // really reports.
    let created = rest_prefixed(
        &emulator,
        local,
        "PUT",
        &format!("/{SHARDED_INDEX}"),
        Some(json!({
            "settings": { "index": { "number_of_shards": 3, "number_of_replicas": 1 } },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the sharded index: expected 200 from the create, got {} with {:?}",
        created.status, created.body
    );
    index_document(
        &emulator,
        local,
        SHARDED_INDEX,
        "s1",
        json!({ "title": "sharded", "shelf": "west" }),
    )
    .await;
    let health = rest_prefixed(
        &emulator,
        local,
        "GET",
        &format!("/_cluster/health/{SHARDED_INDEX}"),
        None,
    )
    .await;
    assert_eq!(
        (
            health.body["status"].as_str(),
            health.body["active_shards"].as_u64(),
            health.body["unassigned_shards"].as_u64(),
        ),
        (Some("yellow"), Some(3), Some(3)),
        "case one node: expected the replica copies to be unassigned, got {:?}",
        health.body
    );

    // (case, the data-node count to scale each domain to)
    let rows: [(&str, u64); 2] = [
        ("the source cluster scaled out to three nodes", 3),
        ("the source cluster scaled out again to five", 5),
    ];
    for (case, nodes) in rows {
        let updated = emulator
            .call(
                "POST",
                &format!("{PREFIX}/opensearch/domain/{local}/config"),
                Some(json!({
                    "ClusterConfig": { "InstanceType": "m5.large.search", "InstanceCount": nodes },
                })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateDomainConfig, got {} with {:?}",
            updated.status, updated.body
        );
        tokio::time::sleep(SETTLE_WAIT).await;

        // The nodes the scaled-out cluster reports, read through the engine's own node API.
        let nodes_answer = rest_prefixed(&emulator, local, "GET", "/_nodes", None).await;
        let reported = nodes_answer.body["nodes"]
            .as_object()
            .map(serde_json::Map::len)
            .unwrap_or_default() as u64;
        assert_eq!(
            reported, nodes,
            "case {case:?}: expected {nodes} nodes, got {reported} with {:?}",
            nodes_answer.body
        );

        // The copies the index asked for are held now that there is another node to hold
        // them: nothing is unassigned, so the cluster is green.
        let health = rest_prefixed(
            &emulator,
            local,
            "GET",
            &format!("/_cluster/health/{SHARDED_INDEX}"),
            None,
        )
        .await;
        assert_eq!(
            (
                health.body["status"].as_str(),
                health.body["active_shards"].as_u64(),
                health.body["unassigned_shards"].as_u64(),
            ),
            (Some("green"), Some(6), Some(0)),
            "case {case:?}: expected every copy assigned, got {:?}",
            health.body
        );

        // The documents the indexes held before the change are still answered, and so are
        // the remote cluster's through the connection.
        let local_hits = rest_prefixed(
            &emulator,
            local,
            "POST",
            &format!("/{SHARDED_INDEX},{SHARED_INDEX}/_search"),
            None,
        )
        .await;
        assert_eq!(
            answered_hits(&local_hits, case),
            vec![
                (SHARED_INDEX.to_string(), "l1".to_string()),
                (SHARDED_INDEX.to_string(), "s1".to_string()),
            ],
            "case {case:?}: expected both local documents to survive the change, got {:?}",
            local_hits.body
        );
        let across = rest_prefixed(
            &emulator,
            local,
            "POST",
            &format!("/{ALIAS}:{SHARED_INDEX}/_search"),
            None,
        )
        .await;
        assert_eq!(
            answered_hits(&across, case).len(),
            3,
            "case {case:?}: expected the remote cluster's three documents, got {:?}",
            across.body
        );
    }
}
