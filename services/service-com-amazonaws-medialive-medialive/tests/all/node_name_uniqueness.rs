// SPDX-License-Identifier: CC0-1.0

//! The uniqueness of a MediaLive Anywhere node's `Name` inside its cluster,
//! driven end to end over the restJson1 wire through BOTH writes that set it —
//! `CreateNode` and `UpdateNode`.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! RULE: a node's `Name` is "unique in the Cluster … Names are case-sensitive"
//! (`smithy_query(query="doc", shape="UpdateNodeRequest", member="Name")`), a
//! property of the NODE and so of whichever write set it: a request asking for a
//! name another live node of that cluster already holds is refused through the
//! operation's own modelled error channel — 422 `UnprocessableEntityException`
//! for the create, which models no `ConflictException`, and 409
//! `ConflictException` for the update — and writes nothing.
//!
//! The suites are TABLES: the issue's two creates under one name are ONE row.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::NAME_ALREADY_IN_USE_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::node_listing::{list, summaries};
// The node request helpers are the node lifecycle and settings suites' own, so
// a create or an update driven here goes through the SAME harness as the one
// driven there.
use crate::node_lifecycle::{cluster, create_node, created, delete_node, described, node_id};
use crate::node_settings_update::update_node;
use crate::wire::{error_code, error_message, medialive};

/// A `CreateNode` body carrying the `Name` the row names — or, where it names
/// none, a body with no `name` member at all, which the model allows.
fn create_body(name: Option<&str>) -> Value {
    match name {
        Some(name) => json!({ "name": name }),
        None => json!({}),
    }
}

/// The names reported by `ListNodes` for one cluster, in the order the listing
/// reports them, with the member each node that carries none left out.
async fn listed_names(router: &MultiServiceRouter, case: &str, cluster_id: &str) -> Vec<Value> {
    let (status, body) = list(router, cluster_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListNodes: expected 200, got {status}: {body}"
    );
    summaries(case, &body)
        .iter()
        .map(|node| node["name"].clone())
        .collect()
}

/// Assert one refusal is the create's name-collision answer: 422, the modelled
/// client error, the service's one wording for a name already in use, and the
/// single `ValidationErrors` entry naming the member it was found at.
fn assert_name_collision_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 422,
        "case {case:?}: CreateNode: expected 422, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body}"
    );
    assert_eq!(
        error_message(body),
        NAME_ALREADY_IN_USE_MESSAGE,
        "case {case:?}: expected the shared name-collision wording, got {:?}",
        error_message(body)
    );
    assert_eq!(
        body["validationErrors"][0]["elementPath"],
        json!("name"),
        "case {case:?}: expected the validation error to name the member, got {body}"
    );
    assert_eq!(
        body["validationErrors"][0]["errorMessage"],
        json!(NAME_ALREADY_IN_USE_MESSAGE),
        "case {case:?}: expected the entry to carry the same wording, got {body}"
    );
}

/// One row of the create table: what the cluster already holds, what the second
/// create asks for, which cluster it is addressed to, whether the holder's
/// delete is accepted first, and whether the create is accepted.
struct Row {
    case: &'static str,
    held: Option<&'static str>,
    asked: Option<&'static str>,
    /// 0 is the cluster the holder is in, 1 the other cluster of the account.
    cluster_index: usize,
    free_the_name: bool,
    accepted: bool,
}

/// Rule: a node's `Name` is unique among the LIVE nodes of its cluster and is
/// compared case-sensitively, so a create asking for a name one of them already
/// holds stores nothing and is refused — while a name no live node of that
/// cluster holds is created: a different name, the same name differing in case or
/// by a trailing space, the same name in another cluster, the name of a node
/// whose deregistration has been accepted, and no name at all (which is not a
/// name two nodes share).
///
/// The rows differ along WHERE the name is already held, along how the asked-for
/// name differs from the held one, along the FORM of the held name (one
/// character, non-ASCII, long) and along whether either side names a name at
/// all, because that is what the uniqueness generalises over.
#[tokio::test]
async fn create_node_keeps_a_name_unique_in_its_cluster() {
    let rows = [
        Row {
            case: "the name a live node of the cluster holds",
            held: Some("worker"),
            asked: Some("worker"),
            cluster_index: 0,
            free_the_name: false,
            accepted: false,
        },
        Row {
            case: "a held name of one character",
            held: Some("a"),
            asked: Some("a"),
            cluster_index: 0,
            free_the_name: false,
            accepted: false,
        },
        Row {
            case: "a held non-ASCII name",
            held: Some("nœud-à-dix"),
            asked: Some("nœud-à-dix"),
            cluster_index: 0,
            free_the_name: false,
            accepted: false,
        },
        Row {
            case: "a held long name",
            held: Some(
                "worker-worker-worker-worker-worker-worker-worker-worker-worker-\
                 worker-worker-worker-worker-worker-worker-worker-worker-worker",
            ),
            asked: Some(
                "worker-worker-worker-worker-worker-worker-worker-worker-worker-\
                 worker-worker-worker-worker-worker-worker-worker-worker-worker",
            ),
            cluster_index: 0,
            free_the_name: false,
            accepted: false,
        },
        Row {
            case: "that name in another case",
            held: Some("worker"),
            asked: Some("WORKER"),
            cluster_index: 0,
            free_the_name: false,
            accepted: true,
        },
        Row {
            case: "that name with a trailing space",
            held: Some("worker"),
            asked: Some("worker "),
            cluster_index: 0,
            free_the_name: false,
            accepted: true,
        },
        Row {
            case: "a name nothing holds",
            held: Some("worker"),
            asked: Some("spare"),
            cluster_index: 0,
            free_the_name: false,
            accepted: true,
        },
        Row {
            case: "the same name in another cluster",
            held: Some("worker"),
            asked: Some("worker"),
            cluster_index: 1,
            free_the_name: false,
            accepted: true,
        },
        Row {
            case: "the name of a node whose delete was accepted",
            held: Some("worker"),
            asked: Some("worker"),
            cluster_index: 0,
            free_the_name: true,
            accepted: true,
        },
        Row {
            case: "no name at all beside a named node",
            held: Some("worker"),
            asked: None,
            cluster_index: 0,
            free_the_name: false,
            accepted: true,
        },
        Row {
            case: "no name at all beside a node that has none",
            held: None,
            asked: None,
            cluster_index: 0,
            free_the_name: false,
            accepted: true,
        },
    ];

    for Row {
        case,
        held,
        asked,
        cluster_index,
        free_the_name,
        accepted,
    } in rows
    {
        let router = medialive();
        let clusters = [
            cluster(&router, "unique-node-name").await,
            cluster(&router, "other-unique-node-name").await,
        ];
        let holder = created(&router, case, &clusters[0], create_body(held)).await;
        let holder_id = node_id(case, &holder);
        if free_the_name {
            let (status, release) = delete_node(&router, &clusters[0], &holder_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteNode: expected 202, got {status}: {release}"
            );
        }

        let (status, body) = create_node(&router, &clusters[cluster_index], create_body(asked)).await;

        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: CreateNode(name={asked:?}): expected 201, got {status}: {body}"
            );
            let created_id = node_id(case, &body);
            assert_ne!(
                created_id, holder_id,
                "case {case:?}: expected a second node, got the holder's id again"
            );
            assert_eq!(
                body["name"],
                asked.map_or(Value::Null, |name| json!(name)),
                "case {case:?}: expected the name the request asked for, got {:?}",
                body["name"]
            );
            // The create is read back through the operation that reads ONE node,
            // so what was stored is what the create reported.
            let read = described(&router, case, &clusters[cluster_index], &created_id).await;
            assert_eq!(
                read, body,
                "case {case:?}: expected the created node read back unchanged, got {read}"
            );
            continue;
        }

        assert_name_collision_refusal(case, status, &body);
        // The refusal stored nothing: the holder is exactly as it was and its
        // cluster still holds the one node.
        let read = described(&router, case, &clusters[0], &holder_id).await;
        assert_eq!(
            read, holder,
            "case {case:?}: expected the refused create to change nothing, got {read}"
        );
        assert_eq!(
            listed_names(&router, case, &clusters[0]).await,
            vec![held.map_or(Value::Null, |name| json!(name))],
            "case {case:?}: expected the cluster to hold one node under that name"
        );
    }
}

/// Rule: because at most one live node of a cluster holds a given name, a node
/// re-sending its OWN name through `UpdateNode` collides with nothing and is
/// accepted, while asking for a name a live sibling holds is refused with 409 and
/// writes nothing — which is exactly what two nodes stored under one name would
/// make impossible.
///
/// The rows are every node of a cluster whose names were asked for by separate
/// creates, each re-sending its own name and then asking for each sibling's,
/// because the uniqueness has to hold for every node of the cluster rather than
/// for the one created first.
#[tokio::test]
async fn one_live_node_per_name_lets_every_node_re_send_its_own_name() {
    // The names the cluster's nodes are created under: distinct, and spanning
    // the forms a name takes (plain, one character, differing only in case).
    let names = ["worker", "w", "Worker"];

    let router = medialive();
    let cluster_id = cluster(&router, "one-node-per-name").await;
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        let node = created(&router, name, &cluster_id, json!({ "name": name })).await;
        ids.push(node_id(name, &node));
    }

    // Every name the cluster holds is held ONCE, however many creates asked for
    // one of them.
    assert_eq!(
        listed_names(&router, "the whole cluster", &cluster_id).await,
        names.iter().map(|name| json!(name)).collect::<Vec<Value>>(),
        "expected one node per name in the order their ids were minted"
    );

    for (index, name) in names.iter().enumerate() {
        let case = format!("{name} re-sending its own name");
        let before = described(&router, &case, &cluster_id, &ids[index]).await;
        let (status, body) = update_node(&router, &cluster_id, &ids[index], json!({ "name": name })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: UpdateNode(name={name:?}): expected 201, got {status}: {body}"
        );
        assert_eq!(
            body["name"],
            json!(name),
            "case {case:?}: expected the node to keep its own name, got {:?}",
            body["name"]
        );
        assert_eq!(
            described(&router, &case, &cluster_id, &ids[index]).await,
            before,
            "case {case:?}: expected re-sending the node's own name to change nothing"
        );

        // A create asking for a name this node holds is refused, and so is a
        // rename of a SIBLING onto it: one name, one live node.
        let (status, refusal) = create_node(&router, &cluster_id, json!({ "name": name })).await;
        assert_name_collision_refusal(&format!("a second create of {name:?}"), status, &refusal);

        for (other, other_id) in ids.iter().enumerate().filter(|(other, _)| *other != index) {
            let case = format!("{} asking for {name:?}", names[other]);
            let (status, refusal) =
                update_node(&router, &cluster_id, other_id, json!({ "name": name })).await;
            assert_eq!(
                status, 409,
                "case {case:?}: UpdateNode: expected 409, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(status, &refusal),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {refusal}"
            );
            assert_eq!(
                error_message(&refusal),
                NAME_ALREADY_IN_USE_MESSAGE,
                "case {case:?}: expected the shared name-collision wording, got {refusal}"
            );
        }
    }

    // After every refusal the cluster still holds exactly the nodes it started
    // with, each under the name its own create asked for.
    assert_eq!(
        listed_names(&router, "after the refusals", &cluster_id).await,
        names.iter().map(|name| json!(name)).collect::<Vec<Value>>(),
        "expected the refused writes to leave every name where it was"
    );
}

/// Rule: a create RETRIED with the `RequestId` (`@idempotencyToken`) of an
/// earlier create for the same cluster answers with the node that one created
/// rather than colliding with it, while two creates under one name that do not
/// share a token are a second node under a name already held — and so refused.
///
/// The rows differ along which token each of the two creates presents, because
/// that is what decides whether the second create is a retry of the first or a
/// new request for a name the cluster already holds.
#[tokio::test]
async fn a_retried_create_answers_with_the_node_it_already_made() {
    // (case, the first create's token, the second's, whether one node answers both)
    let rows: [(&str, Option<&str>, Option<&str>, bool); 5] = [
        ("the same token", Some("token-a"), Some("token-a"), true),
        ("two tokens", Some("token-a"), Some("token-b"), false),
        ("no token at all", None, None, false),
        ("only the first names one", Some("token-a"), None, false),
        ("only the second names one", None, Some("token-b"), false),
    ];

    for (case, first_token, second_token, same) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "retried-node-create").await;

        let mut first = json!({ "name": "worker" });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let holder = created(&router, case, &cluster_id, first).await;
        let holder_id = node_id(case, &holder);

        let mut second = json!({ "name": "worker" });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }
        let (status, body) = create_node(&router, &cluster_id, second).await;

        if same {
            assert_eq!(
                status, 201,
                "case {case:?}: the retry: expected 201, got {status}: {body}"
            );
            assert_eq!(
                node_id(case, &body),
                holder_id,
                "case {case:?}: expected the retry to answer with the node the first create made"
            );
            assert_eq!(
                body, holder,
                "case {case:?}: expected the retry to report that node unchanged, got {body}"
            );
        } else {
            assert_name_collision_refusal(case, status, &body);
        }

        // Either way the cluster holds ONE node under that name.
        assert_eq!(
            listed_names(&router, case, &cluster_id).await,
            vec![json!("worker")],
            "case {case:?}: expected the cluster to hold one node named \"worker\""
        );
    }
}
