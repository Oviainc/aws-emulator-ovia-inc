// SPDX-License-Identifier: CC0-1.0

//! `CreateNodeRegistrationScript` — the script MediaLive Anywhere issues for one
//! cluster — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.
//!
//! The suites are tables: the stub issue's single call is ONE row.
//!
//! The rules asserted here:
//!
//! 1. The script is issued for a cluster the account HOLDS and declares exactly
//!    the parameters the request carried — the cluster, the node a
//!    re-registration names, the name pattern, the role and the interface
//!    mappings — running `create-node` for a unit that has no node yet and only
//!    the activation for a node that already exists.
//! 2. Issuing a script creates NO node: one script registers every unit of the
//!    cluster, and it is the script, run on a unit, that creates that unit's node.
//! 3. Every value the script carries is one shell word, whatever characters it
//!    contains, so what the script declares reads back exactly as the request
//!    spelled it.
//! 4. A retried request — the same `RequestId` on the same cluster — answers with
//!    the script the first attempt issued, and a request that carries another
//!    token (or none) is a new script.
//! 5. A cluster the account does not hold, an `Id` naming no live node of it and
//!    an interface mapping list past the ceiling are refused with
//!    `BadRequestException` (400), and nothing is issued.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_NODE_INTERFACE_MAPPINGS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
// The cluster/node request helpers, the `NodeRole` and `NetworkInterfaceMode`
// value lists and the create/describe/delete calls are the node lifecycle
// suite's, so a script and the node it registers are driven through ONE harness.
use crate::node_lifecycle::{
    cluster, created, delete_node, described, mapping, node_id, INTERFACE_MODES, ROLES,
};
use crate::node_listing::{list, summaries};
use crate::wire::{call, error_code, error_message, medialive, DEFAULT_REGION};

/// The registration-script sub-resource of one cluster, which
/// `CreateNodeRegistrationScript` is bound to (`smithy_query(query="operation",
/// operation="CreateNodeRegistrationScript")`: `POST
/// /prod/clusters/{ClusterId}/nodeRegistrationScript`).
fn script_uri(cluster_id: &str) -> String {
    format!("/prod/clusters/{cluster_id}/nodeRegistrationScript")
}

async fn create_script(
    router: &MultiServiceRouter,
    cluster_id: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &script_uri(cluster_id), Some(body)).await
}

/// One issued script, with the call itself asserted to have succeeded.
///
/// `200` is the status the model gives this operation
/// (`smithy_query(query="operation", operation="CreateNodeRegistrationScript")`:
/// code 200), unlike the `201` the resource creates answer with — nothing is
/// created here.
pub(crate) async fn issued(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    body: Value,
) -> String {
    let (status, response) = create_script(router, cluster_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateNodeRegistrationScript: expected 200, got {status}: {response}"
    );
    response["nodeRegistrationScript"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a nodeRegistrationScript, got {response}")
        })
        .to_string()
}

/// The value one `VARIABLE='…'` assignment of the script declares, read the way a
/// POSIX shell reads a single-quoted word: everything between the quotes is
/// literal, and a `'\''` splice is one quote inside the same word.
///
/// Written here rather than reused from the emulator on purpose — the suite
/// DECODES what the script declares instead of re-running the encoder, so rule 3
/// is a round trip through two independent implementations.
fn declared(script: &str, variable: &str) -> Option<String> {
    let opening = format!("\n{variable}='");
    let start = script.find(&opening)? + opening.len();
    let mut rest = &script[start..];
    let mut value = String::new();
    loop {
        let end = rest.find('\'')?;
        value.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        match rest.strip_prefix(r"\''") {
            Some(next) => {
                value.push('\'');
                rest = next;
            }
            None => return Some(value),
        }
    }
}

/// Whether the script runs `aws medialive <operation>` anywhere.
fn runs(script: &str, operation: &str) -> bool {
    script.contains(&format!("aws medialive {operation}"))
}

/// What one row of the shape table expects the issued script to declare.
#[derive(Debug, Clone, Default)]
struct Expected {
    /// The node the script re-registers, when the request named one.
    node: Option<String>,
    name_pattern: Option<String>,
    role: Option<String>,
    /// The interface mappings the script carries, as the request's own JSON
    /// entries; empty when the script must declare none.
    mappings: Vec<Value>,
}

/// Rule 1: the script is issued against the cluster the path names and declares
/// exactly the parameters the request carried.
///
/// The rows differ along WHICH members the request carries (none, each on its
/// own, all of them), along the `NodeRole` ([`ROLES`]) and
/// `NetworkInterfaceMode` ([`INTERFACE_MODES`]) values, along the LENGTH of the
/// interface mapping list, and along whether the request asks for a
/// re-registration of an existing node — which is what the operation's `Id`
/// member generalises over.
#[tokio::test]
async fn the_script_declares_what_the_request_asked_for() {
    let router = medialive();
    let cluster_id = cluster(&router, "scripted").await;
    // One live node of the cluster, so the re-registration rows can name a node
    // the account really holds.
    let node = created(&router, "setup", &cluster_id, json!({ "name": "unit-1" })).await;
    let node = node_id("setup", &node);

    let one_mapping = vec![mapping("inputs", "eth0", INTERFACE_MODES[0])];
    let three_mappings = vec![
        mapping("inputs", "eth0", INTERFACE_MODES[0]),
        mapping("outputs", "eth1", INTERFACE_MODES[1]),
        mapping("manage'ment", "eth2", INTERFACE_MODES[0]),
    ];

    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "no member beyond the cluster",
            json!({}),
            Expected::default(),
        ),
        (
            "a name pattern naming both variables",
            json!({ "name": "anywhere-$hn-$ts" }),
            Expected {
                name_pattern: Some("anywhere-$hn-$ts".to_string()),
                ..Expected::default()
            },
        ),
        (
            "the first role",
            json!({ "role": ROLES[0] }),
            Expected {
                role: Some(ROLES[0].to_string()),
                ..Expected::default()
            },
        ),
        (
            "the second role",
            json!({ "role": ROLES[1] }),
            Expected {
                role: Some(ROLES[1].to_string()),
                ..Expected::default()
            },
        ),
        (
            "one interface mapping",
            json!({ "nodeInterfaceMappings": one_mapping }),
            Expected {
                mappings: one_mapping.clone(),
                ..Expected::default()
            },
        ),
        (
            "three interface mappings",
            json!({ "nodeInterfaceMappings": three_mappings }),
            Expected {
                mappings: three_mappings.clone(),
                ..Expected::default()
            },
        ),
        (
            "an empty interface mapping list",
            json!({ "nodeInterfaceMappings": [] }),
            Expected::default(),
        ),
        (
            "every member at once",
            json!({
                "name": "anywhere-$hn",
                "role": ROLES[1],
                "nodeInterfaceMappings": three_mappings,
            }),
            Expected {
                name_pattern: Some("anywhere-$hn".to_string()),
                role: Some(ROLES[1].to_string()),
                mappings: three_mappings.clone(),
                ..Expected::default()
            },
        ),
        (
            "a re-registration naming only the node",
            json!({ "id": node }),
            Expected {
                node: Some(node.clone()),
                ..Expected::default()
            },
        ),
        (
            "a re-registration with a name and a role",
            json!({ "id": node, "name": "anywhere-$ts", "role": ROLES[0] }),
            Expected {
                node: Some(node.clone()),
                name_pattern: Some("anywhere-$ts".to_string()),
                role: Some(ROLES[0].to_string()),
                ..Expected::default()
            },
        ),
        (
            "a re-registration carrying interface mappings",
            json!({ "id": node, "nodeInterfaceMappings": one_mapping }),
            Expected {
                node: Some(node.clone()),
                ..Expected::default()
            },
        ),
    ];

    for (case, body, expected) in rows {
        let script = issued(&router, case, &cluster_id, body).await;

        assert!(
            script.starts_with("#!/bin/sh\n"),
            "case {case:?}: expected a shell script, got {script:?}"
        );
        assert_eq!(
            declared(&script, "CLUSTER_ID").as_deref(),
            Some(cluster_id.as_str()),
            "case {case:?}: expected the script to name the cluster it was issued for, got {script:?}"
        );
        assert_eq!(
            declared(&script, "AWS_DEFAULT_REGION").as_deref(),
            Some(DEFAULT_REGION),
            "case {case:?}: expected the script to address the signed region, got {script:?}"
        );
        assert!(
            declared(&script, "REGISTRATION_ID").is_some_and(|id| id.chars().all(char::is_numeric)),
            "case {case:?}: expected the script to carry a numeric registration id, got {script:?}"
        );
        assert_eq!(
            declared(&script, "NODE_NAME_PATTERN"),
            expected.name_pattern,
            "case {case:?}: expected the name pattern the request carried, got {script:?}"
        );
        assert_eq!(
            declared(&script, "NODE_ROLE"),
            expected.role,
            "case {case:?}: expected the role the request carried, got {script:?}"
        );

        // The mappings are the create's argument, so they are part of a script
        // that creates a node and left out of a re-registration, which runs no
        // create.
        let mappings = declared(&script, "NODE_INTERFACE_MAPPINGS").map(|value| {
            serde_json::from_str::<Value>(&value).unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON mapping list, got {value:?}: {error}")
            })
        });
        let expected_mappings: Option<Value> = (!expected.mappings.is_empty()).then(|| {
            Value::Array(
                expected
                    .mappings
                    .iter()
                    .map(|entry| {
                        json!({
                            "LogicalInterfaceName": entry["logicalInterfaceName"],
                            "NetworkInterfaceMode": entry["networkInterfaceMode"],
                            "PhysicalInterfaceName": entry["physicalInterfaceName"],
                        })
                    })
                    .collect(),
            )
        });
        assert_eq!(
            mappings, expected_mappings,
            "case {case:?}: expected the interface mappings the create runs with, got {script:?}"
        );

        match &expected.node {
            Some(id) => {
                assert_eq!(
                    declared(&script, "NODE_ID").as_deref(),
                    Some(id.as_str()),
                    "case {case:?}: expected the script to name the node it re-registers, got {script:?}"
                );
                assert!(
                    !runs(&script, "create-node"),
                    "case {case:?}: a re-registration creates no node, got {script:?}"
                );
                assert_eq!(
                    runs(&script, "update-node \\"),
                    expected.name_pattern.is_some() || expected.role.is_some(),
                    "case {case:?}: expected the settings the request named to be applied, got {script:?}"
                );
            }
            None => {
                assert!(
                    declared(&script, "NODE_ID").is_none(),
                    "case {case:?}: a script for a unit with no node names none, got {script:?}"
                );
                assert!(
                    runs(&script, "create-node"),
                    "case {case:?}: expected the script to create the unit's node, got {script:?}"
                );
            }
        }

        // Either shape ends by activating the node, which is what the operation
        // documents the script does.
        assert!(
            runs(&script, "update-node-state") && script.contains("--state ACTIVE"),
            "case {case:?}: expected the script to activate the node, got {script:?}"
        );
    }
}

/// Rule 2: issuing a script creates no node — one script registers every
/// hardware unit of the cluster, so the nodes appear only when it is run.
///
/// The rows differ along what the request carries, including the members a
/// create would need, so none of them is what makes a node.
#[tokio::test]
async fn issuing_a_script_creates_no_node() {
    let rows: [(&str, Value); 4] = [
        ("no member beyond the cluster", json!({})),
        (
            "a name pattern and a role",
            json!({ "name": "anywhere-$hn", "role": ROLES[0] }),
        ),
        (
            "interface mappings",
            json!({ "nodeInterfaceMappings": [mapping("inputs", "eth0", INTERFACE_MODES[1])] }),
        ),
        (
            "an idempotency token",
            json!({ "requestId": "no-node-please" }),
        ),
    ];

    for (case, body) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "empty-cluster").await;

        let _ = issued(&router, case, &cluster_id, body).await;

        let (status, listed) = list(&router, &cluster_id, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListNodes: expected 200, got {status}: {listed}"
        );
        assert!(
            summaries(case, &listed).is_empty(),
            "case {case:?}: expected the cluster to hold no node, got {listed}"
        );
    }
}

/// Rule 3: every value the script carries is one shell word, so what it declares
/// reads back exactly as the request spelled it.
///
/// The rows are the characters a shell gives meaning to — quotes, a command
/// substitution, a variable, a separator, a newline, a backslash — each as a name
/// pattern, which is the one free-form string the request carries. A value that
/// survives this is data in the script rather than something it runs.
#[tokio::test]
async fn every_value_the_script_carries_is_one_shell_word() {
    let names: [&str; 10] = [
        "plain",
        "with spaces",
        "it's",
        "''",
        "\"double\"",
        "$(hostname)",
        "$HOME-$hn",
        "one;two && three | four",
        "back\\slash",
        "two\nlines",
    ];

    let router = medialive();
    let cluster_id = cluster(&router, "quoting").await;

    for name in names {
        let case = format!("name {name:?}");
        let script = issued(&router, &case, &cluster_id, json!({ "name": name })).await;
        assert_eq!(
            declared(&script, "NODE_NAME_PATTERN").as_deref(),
            Some(name),
            "case {case:?}: expected the script to declare the pattern verbatim, got {script:?}"
        );
        // The value is quoted, so the shell never sees the substitution: the only
        // place the text appears is inside that one word.
        let quoted = format!("NODE_NAME_PATTERN='{}'", name.replace('\'', r"'\''"));
        assert_eq!(
            script.matches(&quoted).count(),
            1,
            "case {case:?}: expected exactly one quoted assignment, got {script:?}"
        );
    }
}

/// Rule 4: a retried request answers with the script the first attempt issued,
/// and a request carrying another token — or none — is a new script.
///
/// The rows differ along what the second request carries: the same token with the
/// same body, the same token with a different body (the token is what makes it a
/// retry, not the body), another token, no token at all, and the same token on
/// another cluster, whose script is that cluster's.
#[tokio::test]
async fn a_retried_request_answers_with_the_script_it_issued() {
    // (case, the second request's body, whether it must answer with the first
    // script)
    let rows: [(&str, Value, bool); 5] = [
        (
            "the same token and body",
            json!({ "requestId": "token-1", "name": "anywhere-$hn" }),
            true,
        ),
        (
            "the same token, another body",
            json!({ "requestId": "token-1", "role": ROLES[1] }),
            true,
        ),
        (
            "another token",
            json!({ "requestId": "token-2", "name": "anywhere-$hn" }),
            false,
        ),
        (
            "no token at all",
            json!({ "name": "anywhere-$hn" }),
            false,
        ),
        (
            "the same token on another cluster",
            json!({ "requestId": "token-1", "name": "anywhere-$hn" }),
            false,
        ),
    ];

    for (case, second_body, same_script) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "retried").await;
        let elsewhere = cluster(&router, "retried-elsewhere").await;

        let first = issued(
            &router,
            case,
            &cluster_id,
            json!({ "requestId": "token-1", "name": "anywhere-$hn" }),
        )
        .await;

        let on_cluster = if case == "the same token on another cluster" {
            &elsewhere
        } else {
            &cluster_id
        };
        let second = issued(&router, case, on_cluster, second_body).await;

        if same_script {
            assert_eq!(
                second, first,
                "case {case:?}: expected the retry to answer with the script already issued"
            );
        } else {
            assert_ne!(
                second, first,
                "case {case:?}: expected a new script, got the one already issued"
            );
            assert_ne!(
                declared(&second, "REGISTRATION_ID"),
                declared(&first, "REGISTRATION_ID"),
                "case {case:?}: expected a new script to carry its own registration id"
            );
            assert_eq!(
                declared(&second, "CLUSTER_ID").as_deref(),
                Some(on_cluster.as_str()),
                "case {case:?}: expected the script to name the cluster it was issued for, got {second:?}"
            );
        }
    }
}

/// Rule 5: a script registers nodes in a cluster the account holds, for a node
/// the cluster still holds, out of a request that is within bounds — so a request
/// that satisfies none of those is refused with `BadRequestException` (400, the
/// client error this operation models; it declares no `NotFoundException`) and
/// nothing is issued.
///
/// The rows differ along WHY the request cannot be served: a cluster id nothing
/// holds, a node id no node ever had, a live node of another cluster, a node
/// inside its deregistering window, one the clock has settled into
/// `DEREGISTERED`, and an interface mapping list one past the ceiling.
#[tokio::test]
async fn a_request_it_cannot_serve_is_refused() {
    // (case, the refusal expected)
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Row {
        UnknownCluster,
        UnknownNode,
        NodeOfAnotherCluster,
        Deregistering,
        Deregistered,
        PastTheMappingCeiling,
    }

    let rows: [(&str, Row); 6] = [
        ("a cluster the account does not hold", Row::UnknownCluster),
        ("an id no node ever had", Row::UnknownNode),
        ("a node of another cluster", Row::NodeOfAnotherCluster),
        ("inside the deregistering window", Row::Deregistering),
        ("once deregistered", Row::Deregistered),
        (
            "one interface mapping past the ceiling",
            Row::PastTheMappingCeiling,
        ),
    ];

    for (case, row) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "refusing").await;
        let elsewhere = cluster(&router, "refusing-elsewhere").await;
        let node = created(&router, case, &cluster_id, json!({ "name": "unit-1" })).await;
        let node = node_id(case, &node);

        // A well-formed id no `CreateCluster` or `CreateNode` minted.
        let unknown = "9999999";
        let (addressed, body, expected_message) = match row {
            Row::UnknownCluster => (
                unknown.to_string(),
                json!({}),
                format!("Cluster {unknown} not found"),
            ),
            Row::UnknownNode => (
                cluster_id.clone(),
                json!({ "id": unknown }),
                format!("Node {unknown} not found"),
            ),
            Row::NodeOfAnotherCluster => (
                elsewhere.clone(),
                json!({ "id": node }),
                format!("Node {node} not found"),
            ),
            Row::Deregistering | Row::Deregistered => (
                cluster_id.clone(),
                json!({ "id": node }),
                format!("Node {node} already deleted"),
            ),
            Row::PastTheMappingCeiling => {
                let mappings: Vec<Value> = (0..=MAX_NODE_INTERFACE_MAPPINGS)
                    .map(|index| {
                        mapping(
                            &format!("inputs-{index}"),
                            &format!("eth{index}"),
                            INTERFACE_MODES[index % INTERFACE_MODES.len()],
                        )
                    })
                    .collect();
                let count = mappings.len();
                (
                    cluster_id.clone(),
                    json!({ "nodeInterfaceMappings": mappings }),
                    format!(
                        "nodeInterfaceMappings carries {count} mappings; \
                         at most {MAX_NODE_INTERFACE_MAPPINGS} are accepted"
                    ),
                )
            }
        };

        if row == Row::Deregistering || row == Row::Deregistered {
            let (status, deleted) = delete_node(&router, &cluster_id, &node).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteNode: expected 202, got {status}: {deleted}"
            );
            if row == Row::Deregistered {
                wait_for_the_clock();
            }
        }

        let (status, refusal) = create_script(&router, &addressed, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            expected_message,
            "case {case:?}: expected the refusal to name what the request got wrong, got {refusal}"
        );
        assert!(
            refusal.get("nodeRegistrationScript").is_none(),
            "case {case:?}: expected no script on a refusal, got {refusal}"
        );

        // The refused request left the node it named exactly as it was, and the
        // cluster still issues scripts.
        let after = described(&router, case, &cluster_id, &node).await;
        assert_eq!(
            after["id"].as_str(),
            Some(node.as_str()),
            "case {case:?}: expected the node to be readable as it was, got {after}"
        );
        let script = issued(&router, case, &cluster_id, json!({})).await;
        assert_eq!(
            declared(&script, "CLUSTER_ID").as_deref(),
            Some(cluster_id.as_str()),
            "case {case:?}: expected the cluster to go on issuing scripts, got {script:?}"
        );
    }
}

/// Rule 1 and 5 together, on the one row the issue's own call is: a
/// re-registration script names the node `DescribeNode` reports, and once that
/// node's deregistration has been accepted no script is issued for it.
#[tokio::test]
async fn a_re_registration_script_names_the_stored_node() {
    let case = "the issue's own call";
    let router = medialive();
    let cluster_id = cluster(&router, "re-registering").await;
    let node = created(
        &router,
        case,
        &cluster_id,
        json!({ "name": "unit-1", "role": ROLES[0] }),
    )
    .await;
    let id = node_id(case, &node);

    let script = issued(&router, case, &cluster_id, json!({ "id": id })).await;
    let read_back = described(&router, case, &cluster_id, &id).await;
    assert_eq!(
        declared(&script, "NODE_ID").as_deref(),
        read_back["id"].as_str(),
        "case {case:?}: expected the script to name the node the store reports, got {script:?}"
    );

    let (status, deleted) = delete_node(&router, &cluster_id, &id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteNode: expected 202, got {status}: {deleted}"
    );
    let (status, refusal) = create_script(&router, &cluster_id, json!({ "id": id })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the deregistered node to be refused, got {status}: {refusal}"
    );
    assert_eq!(
        error_message(&refusal),
        format!("Node {id} already deleted"),
        "case {case:?}: expected the refusal to name the node, got {refusal}"
    );
}
