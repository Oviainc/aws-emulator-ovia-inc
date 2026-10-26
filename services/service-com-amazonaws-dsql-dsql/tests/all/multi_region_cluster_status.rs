// SPDX-License-Identifier: CC0-1.0

//! The status a cluster reports while its multi-Region configuration is
//! outstanding, over the wire.
//!
//! RULE under test: a cluster's status reports the state of its multi-Region
//! configuration. A cluster created or reconfigured with a `multiRegionProperties`
//! configuration is not usable until its peering is established on both sides, so
//! it settles into `PENDING_SETUP` and reaches `ACTIVE` only once every peer it
//! names names it back; and the deletion of such a peered cluster unwinds that
//! peering first, reporting `PENDING_DELETE` before the removal itself
//! (`DELETING`) and before the cluster goes away.
//!
//! `ClusterStatus` carries both values (`smithy_query(query="shape",
//! shape="ClusterStatus")`: ACTIVE | CREATING | DELETED | DELETING | FAILED | IDLE
//! | INACTIVE | PENDING_DELETE | PENDING_SETUP | UPDATING), and `CreateCluster`
//! documents multi-Region creation — a witness Region and peer clusters, each
//! separately permission-gated — as a flow of its own
//! (`smithy_query(query="operation", operation="CreateCluster")`).
//!
//! The tables span the class rather than one request: a configuration naming a
//! witness Region alone, peers alone, both, several peers, a peer this store does
//! hold which does not name it back, and no configuration at all; and the peering
//! is driven to completion across two Regions of one store the way a client drives
//! it, since a peering is a relationship both clusters carry.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs` — this crate's one test client.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::CLUSTER_STATUS_VALUES;
use service_com_amazonaws_dsql_dsql::state::{
    MAX_MULTI_REGION_PEERS, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETING, STATUS_PENDING_DELETE,
    STATUS_PENDING_SETUP, STATUS_UPDATING,
};

use crate::cluster_lifecycle::cluster_id_for;
use crate::harness::{expected_arn, wait_for_the_clock, Answer, Dsql};

/// The `ValidationExceptionReason` a member-value refusal carries. Source: the
/// service's own model, `smithy_query(query="shape",
/// shape="ValidationExceptionReason")` — one of `unknownOperation`, `cannotParse`,
/// `fieldValidationFailed`, `deletionProtectionEnabled`, `other`.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The member name the peer-list refusal must name in `fieldList`, spelled with
/// its path from the request root the way both operations' documentation spells
/// it.
const PEER_CLUSTERS_MEMBER: &str = "multiRegionProperties.clusters";

/// The Region the first cluster of a multi-Region configuration belongs to.
const FIRST_REGION: &str = "us-east-1";
/// The Region its peer belongs to — a cluster of a multi-Region configuration is
/// peered with a cluster in ANOTHER Region.
const SECOND_REGION: &str = "us-east-2";
/// The witness Region both clusters name: a third Region, which the operation's
/// own constraint requires (it can be neither cluster's own Region).
const WITNESS_REGION: &str = "us-west-2";

/// A peer ARN naming a cluster this store does not hold, as `ClusterArn`'s pattern
/// requires one (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`).
fn absent_peer_arn(case: usize) -> String {
    expected_arn(SECOND_REGION, &cluster_id_for(900 + case))
}

/// The `multiRegionProperties` a case sends, built from the members it names.
fn properties(witness: Option<&str>, peers: &[String]) -> Value {
    let mut properties = serde_json::Map::new();
    if let Some(witness) = witness {
        properties.insert("witnessRegion".to_string(), json!(witness));
    }
    if !peers.is_empty() {
        properties.insert("clusters".to_string(), json!(peers));
    }
    Value::Object(properties)
}

/// One `CreateCluster` configuration and the status the clock must settle the
/// created cluster into.
struct ConfigurationCase {
    name: &'static str,
    /// Whether the request carries a `multiRegionProperties` member at all.
    configured: bool,
    witness: Option<&'static str>,
    /// How many peer ARNs naming clusters this store does NOT hold to send.
    absent_peers: usize,
    /// Whether to also name a peer cluster this store DOES hold — one that does
    /// not name the created cluster back.
    held_peer: bool,
    settles_to: &'static str,
}

fn configuration_cases() -> Vec<ConfigurationCase> {
    vec![
        ConfigurationCase {
            name: "no multiRegionProperties at all",
            configured: false,
            witness: None,
            absent_peers: 0,
            held_peer: false,
            settles_to: STATUS_ACTIVE,
        },
        ConfigurationCase {
            name: "a multiRegionProperties carrying neither member configures nothing",
            configured: true,
            witness: None,
            absent_peers: 0,
            held_peer: false,
            settles_to: STATUS_ACTIVE,
        },
        ConfigurationCase {
            name: "a witness Region alone, so no peering is established yet",
            configured: true,
            witness: Some(WITNESS_REGION),
            absent_peers: 0,
            held_peer: false,
            settles_to: STATUS_PENDING_SETUP,
        },
        ConfigurationCase {
            name: "one peer cluster this store does not hold",
            configured: true,
            witness: None,
            absent_peers: 1,
            held_peer: false,
            settles_to: STATUS_PENDING_SETUP,
        },
        ConfigurationCase {
            name: "a witness Region and one peer",
            configured: true,
            witness: Some(WITNESS_REGION),
            absent_peers: 1,
            held_peer: false,
            settles_to: STATUS_PENDING_SETUP,
        },
        ConfigurationCase {
            name: "a witness Region and several peers",
            configured: true,
            witness: Some(WITNESS_REGION),
            absent_peers: 3,
            held_peer: false,
            settles_to: STATUS_PENDING_SETUP,
        },
        ConfigurationCase {
            name: "a peer this store holds, which does not name it back",
            configured: true,
            witness: Some(WITNESS_REGION),
            absent_peers: 0,
            held_peer: true,
            settles_to: STATUS_PENDING_SETUP,
        },
        ConfigurationCase {
            name: "a peer this store holds beside one it does not",
            configured: true,
            witness: Some(WITNESS_REGION),
            absent_peers: 1,
            held_peer: true,
            settles_to: STATUS_PENDING_SETUP,
        },
    ]
}

/// RULE: `CreateCluster` reports `CREATING`, and the clock settles the cluster into
/// the status its configuration asks for — `ACTIVE` for a cluster with no
/// multi-Region configuration, `PENDING_SETUP` for one whose peering is not
/// established on both sides — and a cluster waiting in `PENDING_SETUP` stays
/// there, rather than drifting to `ACTIVE` on its own.
#[tokio::test]
async fn the_clock_settles_a_created_cluster_into_the_status_its_configuration_asks_for() {
    for (number, case) in configuration_cases().into_iter().enumerate() {
        let dsql = Dsql::in_region(FIRST_REGION);

        let mut peers: Vec<String> = (0..case.absent_peers).map(absent_peer_arn).collect();
        if case.held_peer {
            let held = dsql.create(json!({"deletionProtectionEnabled": false})).await;
            assert_eq!(
                held.status, 200,
                "case {:?}: the peer cluster must be created, got {} {:?}",
                case.name, held.status, held.body
            );
            peers.push(held.str("arn"));
        }

        let mut request = json!({"deletionProtectionEnabled": false});
        if case.configured {
            request["multiRegionProperties"] = properties(case.witness, &peers);
        }
        let created = dsql.create(request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCluster must answer 200 for {request}, got {} {:?}",
            case.name, created.status, created.body
        );
        assert_eq!(
            created.str("status"),
            STATUS_CREATING,
            "case {:?}: a freshly minted cluster reports {STATUS_CREATING}, got {:?}",
            case.name,
            created.str("status")
        );

        let identifier = created.str("identifier");
        wait_for_the_clock().await;
        let settled = dsql.get(&identifier).await;
        assert_eq!(
            settled.str("status"),
            case.settles_to,
            "case {:?} (#{number}): the clock must settle the cluster into {:?}, got {:?}",
            case.name,
            case.settles_to,
            settled.str("status")
        );
        assert!(
            CLUSTER_STATUS_VALUES.contains(&settled.str("status").as_str()),
            "case {:?}: the status must be a modelled ClusterStatus value, got {:?}",
            case.name,
            settled.str("status")
        );

        // The wait is for the OTHER side of the peering to be configured, which no
        // amount of elapsed time supplies.
        wait_for_the_clock().await;
        let again = dsql.get(&identifier).await;
        assert_eq!(
            again.str("status"),
            case.settles_to,
            "case {:?}: the settled status must not drift, got {:?}",
            case.name,
            again.str("status")
        );
    }
}

/// Two clusters in different Regions of ONE store, peered the way a client peers
/// them: the first is created with the witness Region, the second names the first,
/// and the first is then reconfigured to name the second. Returns a client per
/// Region and each cluster's identifier, with both clusters `ACTIVE`.
async fn peered_pair(engine: &Arc<engine::InMemoryEngine>) -> (Dsql, String, Dsql, String) {
    let first = Dsql::sharing(engine, FIRST_REGION);
    let second = Dsql::sharing(engine, SECOND_REGION);

    let one = first
        .create(json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {"witnessRegion": WITNESS_REGION},
        }))
        .await;
    assert_eq!(
        one.status, 200,
        "the first cluster must be created, got {} {:?}",
        one.status, one.body
    );
    let (one_id, one_arn) = (one.str("identifier"), one.str("arn"));

    let two = second
        .create(json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {
                "witnessRegion": WITNESS_REGION,
                "clusters": [one_arn],
            },
        }))
        .await;
    assert_eq!(
        two.status, 200,
        "the second cluster must be created, got {} {:?}",
        two.status, two.body
    );
    let (two_id, two_arn) = (two.str("identifier"), two.str("arn"));

    wait_for_the_clock().await;
    for (case, client, identifier) in [
        ("the cluster its peer does not name yet", &first, &one_id),
        ("the cluster that names a peer not naming it", &second, &two_id),
    ] {
        assert_eq!(
            client.get(identifier).await.str("status"),
            STATUS_PENDING_SETUP,
            "case {case:?}: a one-sided peering leaves the cluster in {STATUS_PENDING_SETUP}"
        );
    }

    // The call that configures the other side: the peering is now named by both.
    let completed = first
        .update(
            &one_id,
            json!({
                "multiRegionProperties": {
                    "witnessRegion": WITNESS_REGION,
                    "clusters": [two_arn],
                },
            }),
        )
        .await;
    assert_eq!(
        (completed.status, completed.str("status")),
        (200, STATUS_UPDATING.to_string()),
        "the configuration change completing the peering must be accepted, got {} {:?}",
        completed.status,
        completed.body
    );

    wait_for_the_clock().await;
    for (case, client, identifier) in [
        ("the reconfigured cluster", &first, &one_id),
        ("the cluster that was waiting for it", &second, &two_id),
    ] {
        assert_eq!(
            client.get(identifier).await.str("status"),
            STATUS_ACTIVE,
            "case {case:?}: an established peering leaves the cluster {STATUS_ACTIVE}"
        );
    }

    (first, one_id, second, two_id)
}

/// RULE: a cluster waiting for its peering reaches `ACTIVE` only once every peer it
/// names names it back — and while it waits it takes the configuration change that
/// completes the peering, but no other mutation.
#[tokio::test]
async fn a_peering_reaches_active_only_once_both_clusters_name_each_other() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let (first, one_id, _second, _two_id) = peered_pair(&engine).await;

    // The pair is peered, so each cluster reports its configuration back.
    let fetched = first.get(&one_id).await;
    assert_eq!(
        fetched
            .get("multiRegionProperties")
            .get("witnessRegion")
            .cloned()
            .unwrap_or(Value::Null),
        json!(WITNESS_REGION),
        "the established configuration must be reported back, got {:?}",
        fetched.get("multiRegionProperties")
    );
}

/// RULE: while a cluster waits in `PENDING_SETUP` it accepts the configuration
/// change that completes its peering — the call the wait is for — and refuses every
/// other mutation with the status gate's 409.
#[tokio::test]
async fn a_waiting_cluster_takes_the_configuration_change_and_nothing_else() {
    let dsql = Dsql::in_region(FIRST_REGION);
    let created = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {"witnessRegion": WITNESS_REGION},
        }))
        .await;
    let identifier = created.str("identifier");
    wait_for_the_clock().await;
    assert_eq!(
        dsql.get(&identifier).await.str("status"),
        STATUS_PENDING_SETUP,
        "the fixture cluster must be waiting for its peering"
    );

    for (case, request) in [
        ("deletionProtectionEnabled", json!({"deletionProtectionEnabled": true})),
        ("kmsEncryptionKey", json!({"kmsEncryptionKey": "alias/other"})),
        ("an empty request", json!({})),
    ] {
        let refused = dsql.update(&identifier, request).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: a cluster waiting for its peering refuses a mutation that does \
             not configure it, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(STATUS_PENDING_SETUP),
            "case {case:?}: the refusal must name the status that refused it, got {:?}",
            refused.message()
        );
    }

    let accepted = dsql
        .update(
            &identifier,
            json!({"multiRegionProperties": {"witnessRegion": WITNESS_REGION}}),
        )
        .await;
    assert_eq!(
        (accepted.status, accepted.str("status")),
        (200, STATUS_UPDATING.to_string()),
        "a configuration change must be accepted while the cluster waits, got {} {:?}",
        accepted.status,
        accepted.body
    );
}

/// RULE: the peer clusters one multi-Region configuration may name are bounded —
/// a configuration at the ceiling is accepted and persisted, one past it is
/// refused with the modelled 400 `ValidationException` naming the member, and the
/// refusal leaves the store exactly as it was. Asserted for both operations that
/// write the member.
#[tokio::test]
async fn a_configuration_naming_more_peers_than_the_ceiling_is_refused() {
    // The ceiling itself comes from the service (`state::MAX_MULTI_REGION_PEERS`),
    // so the table is stated against it rather than against a copy of its value:
    // the bound, one inside it, one past it, and well past it.
    let counts = [
        0,
        1,
        MAX_MULTI_REGION_PEERS - 1,
        MAX_MULTI_REGION_PEERS,
        MAX_MULTI_REGION_PEERS + 1,
        MAX_MULTI_REGION_PEERS * 4,
    ];

    for count in counts {
        let accepted = count <= MAX_MULTI_REGION_PEERS;
        let peers: Vec<String> = (0..count).map(absent_peer_arn).collect();
        let configuration = properties(Some(WITNESS_REGION), &peers);

        // CreateCluster: the member is written as the cluster is minted.
        let dsql = Dsql::in_region(FIRST_REGION);
        let created = dsql
            .create(json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": configuration.clone(),
            }))
            .await;
        if accepted {
            assert_eq!(
                created.status, 200,
                "case CreateCluster with {count} peers: at or inside the ceiling must be \
                 accepted, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.get("multiRegionProperties"),
                configuration,
                "case CreateCluster with {count} peers: the accepted configuration must be \
                 reported back, got {:?}",
                created.get("multiRegionProperties")
            );
        } else {
            assert_refused_peer_list(&format!("CreateCluster with {count} peers"), &created);
            let listed = dsql.list("").await;
            assert_eq!(
                listed.get("clusters").as_array().map(Vec::len).unwrap_or_default(),
                0,
                "case CreateCluster with {count} peers: a refused create must create nothing, \
                 got {:?}",
                listed.get("clusters")
            );
        }

        // UpdateCluster: the same member, written onto a cluster that exists.
        let dsql = Dsql::in_region(FIRST_REGION);
        let identifier = dsql.create_settled().await;
        let updated = dsql
            .update(&identifier, json!({"multiRegionProperties": configuration.clone()}))
            .await;
        if accepted {
            assert_eq!(
                updated.status, 200,
                "case UpdateCluster with {count} peers: at or inside the ceiling must be \
                 accepted, got {} {:?}",
                updated.status, updated.body
            );
        } else {
            assert_refused_peer_list(&format!("UpdateCluster with {count} peers"), &updated);
            let fetched = dsql.get(&identifier).await;
            assert_eq!(
                fetched.get("multiRegionProperties"),
                Value::Null,
                "case UpdateCluster with {count} peers: a refused update must persist no \
                 configuration, got {:?}",
                fetched.get("multiRegionProperties")
            );
            assert_eq!(
                fetched.str("status"),
                STATUS_ACTIVE,
                "case UpdateCluster with {count} peers: a refused update must not move the \
                 cluster's status, got {:?}",
                fetched.str("status")
            );
        }
    }
}

/// Assert `answer` is the modelled 400 `ValidationException` naming the peer-list
/// member, with the `@required` `reason` filled and `fieldList` saying why.
fn assert_refused_peer_list(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a configuration past the peer ceiling must be refused with 400, \
         got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .error
            .as_deref()
            .is_some_and(|code| code.contains("ValidationException")),
        "case {case:?}: the wire code must be ValidationException, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.str("reason"),
        REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: a member-value refusal reports {REASON_FIELD_VALIDATION_FAILED}, \
         got {:?}",
        answer.body
    );
    let field_list = answer.get("fieldList");
    let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        fields.iter().any(|field| {
            field.get("name").and_then(Value::as_str) == Some(PEER_CLUSTERS_MEMBER)
                && field
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
        }),
        "case {case:?}: fieldList must name {PEER_CLUSTERS_MEMBER} with a message saying why, \
         got {field_list:?}"
    );
}

/// One `DeleteCluster` and the statuses the cluster reports on its way out.
struct DeleteCase {
    name: &'static str,
    /// Whether the deleted cluster is peered with another cluster.
    peered: bool,
    /// The status the delete itself answers.
    reports: &'static str,
    /// The statuses the clock carries it through afterwards, in order, before the
    /// cluster goes away.
    then: &'static [&'static str],
}

/// RULE: a deletion that has a multi-Region peering to unwind reports
/// `PENDING_DELETE` until the removal itself starts; one that has none starts the
/// removal straight away. Either way the clock, not a caller, finishes it.
#[tokio::test]
async fn a_peered_cluster_reports_pending_delete_before_its_removal() {
    let cases = [
        DeleteCase {
            name: "a cluster with no multi-Region configuration",
            peered: false,
            reports: STATUS_DELETING,
            then: &[],
        },
        DeleteCase {
            name: "a cluster peered with another Region's cluster",
            peered: true,
            reports: STATUS_PENDING_DELETE,
            then: &[STATUS_DELETING],
        },
    ];

    for case in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let (client, identifier) = if case.peered {
            let (first, one_id, _second, _two_id) = peered_pair(&engine).await;
            (first, one_id)
        } else {
            let dsql = Dsql::sharing(&engine, FIRST_REGION);
            let identifier = dsql.create_settled().await;
            (dsql, identifier)
        };

        let deleted = client.delete(&identifier).await;
        assert_eq!(
            (deleted.status, deleted.str("status")),
            (200, case.reports.to_string()),
            "case {:?}: the delete must answer {:?}, got {} {:?}",
            case.name,
            case.reports,
            deleted.status,
            deleted.body
        );
        assert_eq!(
            client.get(&identifier).await.str("status"),
            case.reports,
            "case {:?}: a read must report the same status the delete answered",
            case.name
        );

        for expected in case.then {
            wait_for_the_clock().await;
            assert_eq!(
                client.get(&identifier).await.str("status"),
                *expected,
                "case {:?}: the clock must carry the deletion on to {expected:?}",
                case.name
            );
        }

        wait_for_the_clock().await;
        let gone = client.get(&identifier).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: the clock must remove the cluster, got {} {:?}",
            case.name, gone.status, gone.body
        );
    }
}
