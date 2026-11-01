// SPDX-License-Identifier: CC0-1.0

//! The CLUSTER-level APIs of a domain's REST data plane, driven over the real wire path: the
//! nodes it reports, the settings it holds, the ingest pipelines it runs and the
//! index-management policies it keeps.
//!
//! Four rules, each over a SET of inputs rather than one recorded call:
//!
//! * **`GET /_nodes` reports the topology the domain's `ClusterConfig` provisions, and
//!   publishes none of the cluster's internal addressing** — a managed domain's hosts, IPs,
//!   transport addresses and node settings are service-internal, so the report carries the
//!   configured role, the engine release and the Availability Zone and nothing else. The
//!   rows span the node roles a configuration provisions (data only, dedicated masters,
//!   warm nodes, zone awareness);
//! * **a cluster setting is written by its NAME, however the request spells it, and read
//!   back from the scope it was written in** — "settings can be specified in a flattened or
//!   expanded format", a transient value overrides the persistent one for the same setting,
//!   and a setting written `null` is reset. The rows span both spellings, both scopes and
//!   the reset;
//! * **`action.auto_create_index` decides whether a write to an index the cluster does not
//!   hold CREATES it** — `true` permits every index, `false` none, and a pattern list is
//!   decided by the first pattern the name matches, a bare pattern being an allow. The rows
//!   span the booleans, both pattern signs and a name no pattern matches;
//! * **an ingest pipeline rewrites the source a write stores, and `_simulate` predicts
//!   exactly that** — one reading of the pipeline serves both, so the document the simulate
//!   answers is the document a write through the same pipeline reads back. The rows span
//!   every processor type the engine registers, plus a type it does not and a processor
//!   missing a required option;
//! * **an index-management policy is a revisioned record**: creating one answers `201` with
//!   the revision counters of the write, reading it back answers the same document, and
//!   updating it is a write against the revision the caller read.
//!
//! Every case drives the real wire path — a signed HTTP request through the multi-service
//! router into the mounted service, under the prefixed path addressing a client on this
//! machine can reach — so what is asserted is what a client sees. The harness is this
//! crate's own ([`super::domain_lifecycle`], [`super::domain_rest_data_plane`]); this suite
//! adds no second copy of it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::domain_lifecycle::Emulator;
use super::domain_rest_data_plane::{error_type, rest_prefixed};

/// `CreateDomain` with the cluster configuration a row asks for, asserting it was accepted.
async fn given_domain(emulator: &Emulator, name: &str, cluster: Option<Value>) {
    let mut body = json!({ "DomainName": name });
    if let Some(cluster) = cluster {
        body["ClusterConfig"] = cluster;
    }
    let answer = emulator.create(body).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// One cluster configuration to report the nodes of: what the create asks for, and how many
/// nodes of each published role the topology gives it.
struct NodeRow {
    case: &'static str,
    domain: &'static str,
    cluster: Value,
    /// `(the role set a node publishes, how many nodes publish it)`, sorted.
    roles: Vec<(Vec<&'static str>, usize)>,
}

/// The table: the node roles a `ClusterConfig` provisions. The roles each node type
/// publishes are the ones OpenSearch 2.x names them by — a data node holds the data and
/// ingest roles and is cluster-manager eligible only where no dedicated master is
/// provisioned.
fn node_rows() -> Vec<NodeRow> {
    vec![
        NodeRow {
            case: "a single data node",
            domain: "nodes-single",
            cluster: json!({ "InstanceType": "t3.small.search", "InstanceCount": 1 }),
            roles: vec![(vec!["cluster_manager", "data", "ingest"], 1)],
        },
        NodeRow {
            case: "three data nodes",
            domain: "nodes-three",
            cluster: json!({ "InstanceType": "t3.small.search", "InstanceCount": 3 }),
            roles: vec![(vec!["cluster_manager", "data", "ingest"], 3)],
        },
        NodeRow {
            case: "data nodes behind dedicated masters",
            domain: "nodes-master",
            cluster: json!({
                "InstanceCount": 2,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
            }),
            roles: vec![(vec!["cluster_manager"], 3), (vec!["data", "ingest"], 2)],
        },
        NodeRow {
            case: "warm nodes beside the data ones",
            domain: "nodes-warm",
            cluster: json!({
                "InstanceCount": 2,
                "DedicatedMasterEnabled": true,
                "DedicatedMasterCount": 3,
                "WarmEnabled": true,
                "WarmCount": 2,
            }),
            roles: vec![
                (vec!["cluster_manager"], 3),
                (vec!["data", "ingest"], 2),
                (vec!["data", "warm"], 2),
            ],
        },
        NodeRow {
            case: "zone-aware data nodes",
            domain: "nodes-zoned",
            cluster: json!({
                "InstanceCount": 4,
                "ZoneAwarenessEnabled": true,
                "ZoneAwarenessConfig": { "AvailabilityZoneCount": 2 },
            }),
            roles: vec![(vec!["cluster_manager", "data", "ingest"], 4)],
        },
    ]
}

/// The members a managed domain publishes for NO node: the cluster's own addressing and the
/// process detail of the machine a node runs on, which a self-managed cluster reports and a
/// managed service does not.
const REDACTED_NODE_MEMBERS: [&str; 8] = [
    "host",
    "ip",
    "transport_address",
    "http_address",
    "settings",
    "os",
    "process",
    "jvm",
];

/// RULE: `GET /_nodes` reports the topology the domain's `ClusterConfig` provisions, and
/// publishes none of the cluster's internal addressing.
#[tokio::test]
async fn the_nodes_api_reports_the_configured_topology_and_redacts_the_internals() {
    let emulator = Emulator::new();

    for row in node_rows() {
        let case = row.case;
        given_domain(&emulator, row.domain, Some(row.cluster.clone())).await;

        let answer = rest_prefixed(&emulator, row.domain, "GET", "/_nodes", None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from GET /_nodes, got {} with {:?}",
            answer.status, answer.body
        );
        let expected_total: usize = row.roles.iter().map(|(_, count)| *count).sum();
        assert_eq!(
            answer.body["_nodes"]["total"].as_u64(),
            Some(expected_total as u64),
            "case {case:?}: expected {expected_total} nodes, got {:?}",
            answer.body["_nodes"]
        );
        assert_eq!(
            answer.body["cluster_name"].as_str(),
            Some(
                format!(
                    "{}:{}",
                    ::engine::emulator_account_id(),
                    row.domain
                )
                .as_str()
            ),
            "case {case:?}: expected the cluster to name this domain, got {:?}",
            answer.body["cluster_name"]
        );

        let nodes = answer.body["nodes"].as_object().unwrap_or_else(|| {
            panic!("case {case:?}: expected a nodes map, got {:?}", answer.body)
        });
        assert_eq!(
            nodes.len(),
            expected_total,
            "case {case:?}: expected {expected_total} node entries, got {nodes:?}"
        );

        // Every node publishes the roles its configured node type holds, and the counts add
        // up to the configured topology.
        let mut counted: Vec<(Vec<String>, usize)> = Vec::new();
        for (id, node) in nodes {
            let roles: Vec<String> = node["roles"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?} ({id}): expected a roles list, got {node:?}")
                })
                .iter()
                .map(|role| role.as_str().unwrap_or_default().to_string())
                .collect();
            match counted.iter_mut().find(|(held, _)| *held == roles) {
                Some((_, count)) => *count += 1,
                None => counted.push((roles, 1)),
            }
            assert_eq!(
                node["name"].as_str(),
                Some(id.as_str()),
                "case {case:?} ({id}): expected the node to be named by its id, got {node:?}"
            );
            assert!(
                node["version"].as_str().is_some_and(|v| v.starts_with("2.")),
                "case {case:?} ({id}): expected the engine release the domain runs, got {node:?}"
            );
            for member in REDACTED_NODE_MEMBERS {
                assert!(
                    node[member].is_null(),
                    "case {case:?} ({id}): a managed domain publishes no {member}, got {:?}",
                    node[member]
                );
            }
        }
        counted.sort();
        let mut expected: Vec<(Vec<String>, usize)> = row
            .roles
            .iter()
            .map(|(roles, count)| {
                (
                    roles.iter().map(|role| (*role).to_string()).collect(),
                    *count,
                )
            })
            .collect();
        expected.sort();
        assert_eq!(
            counted, expected,
            "case {case:?}: expected the configured roles {expected:?}, got {counted:?}"
        );

        // A node filter selects THAT node, and one matching nothing selects no node — never
        // the whole cluster.
        let one = nodes.keys().next().cloned().unwrap_or_default();
        let filtered = rest_prefixed(
            &emulator,
            row.domain,
            "GET",
            &format!("/_nodes/{one}"),
            None,
        )
        .await;
        assert_eq!(
            filtered.body["nodes"].as_object().map(serde_json::Map::len),
            Some(1),
            "case {case:?}: expected the filter to select one node, got {:?}",
            filtered.body
        );
        let nothing =
            rest_prefixed(&emulator, row.domain, "GET", "/_nodes/no-such-node", None).await;
        assert_eq!(
            (
                nothing.status,
                nothing.body["_nodes"]["total"].as_u64(),
                nothing.body["nodes"].as_object().map(serde_json::Map::len),
            ),
            (200, Some(0), Some(0)),
            "case {case:?}: expected a filter matching nothing to select no node, got {:?}",
            nothing.body
        );
    }
}

/// One settings update to apply: the body it is written with, and what the cluster reports
/// afterwards for the two scopes.
struct SettingsRow {
    case: &'static str,
    body: Value,
    /// The `persistent` scope as the report carries it, expanded.
    persistent: Value,
    /// The `transient` scope as the report carries it, expanded.
    transient: Value,
    /// The value a reader of `action.auto_create_index` resolves after the update, if any.
    resolved: Option<&'static str>,
}

/// The table: both spellings of a setting name, both scopes, the transient override and the
/// reset.
fn settings_rows() -> Vec<SettingsRow> {
    vec![
        SettingsRow {
            case: "the flat form, persistent",
            body: json!({ "persistent": { "action.auto_create_index": "false" } }),
            persistent: json!({ "action": { "auto_create_index": "false" } }),
            transient: json!({}),
            resolved: Some("false"),
        },
        SettingsRow {
            case: "the expanded form names the same setting",
            body: json!({ "persistent": { "action": { "auto_create_index": "true" } } }),
            persistent: json!({ "action": { "auto_create_index": "true" } }),
            transient: json!({}),
            resolved: Some("true"),
        },
        SettingsRow {
            case: "a transient value overrides the persistent one",
            body: json!({ "transient": { "action.auto_create_index": "false" } }),
            persistent: json!({ "action": { "auto_create_index": "true" } }),
            transient: json!({ "action": { "auto_create_index": "false" } }),
            resolved: Some("false"),
        },
        SettingsRow {
            case: "a second setting lands beside the first",
            body: json!({ "persistent": { "cluster.max_shards_per_node": 1000 } }),
            persistent: json!({
                "action": { "auto_create_index": "true" },
                "cluster": { "max_shards_per_node": "1000" },
            }),
            transient: json!({ "action": { "auto_create_index": "false" } }),
            resolved: Some("false"),
        },
        SettingsRow {
            case: "a null resets the setting in its own scope",
            body: json!({ "transient": { "action.auto_create_index": Value::Null } }),
            persistent: json!({
                "action": { "auto_create_index": "true" },
                "cluster": { "max_shards_per_node": "1000" },
            }),
            transient: json!({}),
            resolved: Some("true"),
        },
        SettingsRow {
            case: "and in the persistent scope too",
            body: json!({ "persistent": { "action.auto_create_index": Value::Null } }),
            persistent: json!({ "cluster": { "max_shards_per_node": "1000" } }),
            transient: json!({}),
            resolved: None,
        },
    ]
}

/// RULE: a cluster setting is written by its NAME, however the request spells it, read back
/// from the scope it was written in, and a transient value overrides the persistent one.
#[tokio::test]
async fn a_cluster_setting_is_written_by_its_name_and_read_back_from_its_scope() {
    let emulator = Emulator::new();
    let domain = "settings-target";
    given_domain(&emulator, domain, None).await;

    // A cluster nobody wrote a setting on holds none in either scope.
    let empty = rest_prefixed(&emulator, domain, "GET", "/_cluster/settings", None).await;
    assert_eq!(
        (empty.status, &empty.body),
        (200, &json!({ "persistent": {}, "transient": {} })),
        "case \"a cluster with no setting written\": expected both scopes empty, got {:?}",
        empty.body
    );

    for row in settings_rows() {
        let case = row.case;
        let applied = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            "/_cluster/settings",
            Some(row.body.clone()),
        )
        .await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected 200 from PUT /_cluster/settings, got {} with {:?}",
            applied.status, applied.body
        );
        assert_eq!(
            applied.body["acknowledged"].as_bool(),
            Some(true),
            "case {case:?}: expected the update to be acknowledged, got {:?}",
            applied.body
        );

        // The update's own answer and a later read report the same settings: what the
        // cluster holds, not what the request said.
        let read = rest_prefixed(&emulator, domain, "GET", "/_cluster/settings", None).await;
        for (reader, body) in [("PUT", &applied.body), ("GET", &read.body)] {
            assert_eq!(
                (&body["persistent"], &body["transient"]),
                (&row.persistent, &row.transient),
                "case {case:?} ({reader}): expected persistent {:?} and transient {:?}, got {body:?}",
                row.persistent,
                row.transient
            );
        }

        // `flat_settings` reports the same settings under the names they HAVE.
        let flat = rest_prefixed(
            &emulator,
            domain,
            "GET",
            "/_cluster/settings?flat_settings=true",
            None,
        )
        .await;
        let flat_names: BTreeSet<String> = flat.body["persistent"]
            .as_object()
            .map(|held| held.keys().cloned().collect())
            .unwrap_or_default();
        let expanded_leaves = leaf_names(&row.persistent, "");
        assert_eq!(
            flat_names, expanded_leaves,
            "case {case:?}: expected the flat report to name the same settings, got {:?}",
            flat.body["persistent"]
        );

        // And what a reader of the setting resolves is the scope precedence, not the last
        // write: a write to one scope leaves the other alone. An EXPLICIT index create is
        // served whatever the setting says — `action.auto_create_index` governs implicit
        // creation — while a document write to an index the cluster does not hold follows it.
        let created = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            "/auto-create-probe",
            Some(json!({})),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: an explicit index create is served whatever \
             action.auto_create_index says, got {} with {:?}",
            created.status, created.body
        );
        let removed = rest_prefixed(&emulator, domain, "DELETE", "/auto-create-probe", None).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected the probe index to be removable, got {} with {:?}",
            removed.status, removed.body
        );
        let implicit = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            "/auto-create-probe/_doc/1",
            Some(json!({ "message": "hello" })),
        )
        .await;
        let allowed = row.resolved != Some("false");
        assert_eq!(
            implicit.status == 201,
            allowed,
            "case {case:?}: with the resolved action.auto_create_index {:?} an implicit index \
             creation must {}, got {} with {:?}",
            row.resolved,
            if allowed { "be served" } else { "be refused" },
            implicit.status,
            implicit.body
        );
        if implicit.status == 201 {
            let dropped =
                rest_prefixed(&emulator, domain, "DELETE", "/auto-create-probe", None).await;
            assert_eq!(
                dropped.status, 200,
                "case {case:?}: expected the probe index to be removable, got {} with {:?}",
                dropped.status, dropped.body
            );
        }
    }
}

/// RULE: a bound this cluster declares REFUSES the request that would cross it, rather than
/// taking the request and dropping what does not fit — a setting a caller wrote and the
/// cluster discarded would be indistinguishable from one it holds, and a simulate answer is
/// bounded in BYTES because the bytes a processor injects are the caller's too.
#[tokio::test]
async fn a_bound_this_cluster_declares_refuses_rather_than_dropping_what_crosses_it() {
    let emulator = Emulator::new();
    let domain = "bounds-target";
    given_domain(&emulator, domain, None).await;

    // One update past the settings quota is refused WHOLE: nothing it named is stored.
    let quota = service_com_amazonaws_opensearch_amazonopensearchservice::state::
        CLUSTER_SETTING_QUOTA;
    let mut too_many = serde_json::Map::new();
    for index in 0..=quota {
        too_many.insert(format!("cluster.probe_{index}"), json!("1"));
    }
    let refused = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        "/_cluster/settings",
        Some(json!({ "persistent": Value::Object(too_many) })),
    )
    .await;
    assert_eq!(
        (refused.status, error_type(&refused).as_str()),
        (400, "illegal_argument_exception"),
        "case \"an update past the settings quota\": expected the cluster to refuse it, got {} \
         with {:?}",
        refused.status,
        refused.body
    );
    let held = rest_prefixed(&emulator, domain, "GET", "/_cluster/settings", None).await;
    assert_eq!(
        held.body["persistent"],
        json!({}),
        "case \"a refused update\": expected nothing to be stored, got {:?}",
        held.body
    );
    // One setting fewer is taken, so the refusal is the bound and not the route.
    let mut just_enough = serde_json::Map::new();
    for index in 0..quota {
        just_enough.insert(format!("cluster.probe_{index}"), json!("1"));
    }
    let taken = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        "/_cluster/settings",
        Some(json!({ "persistent": Value::Object(just_enough) })),
    )
    .await;
    assert_eq!(
        taken.status, 200,
        "case \"an update at the settings quota\": expected it to be taken, got {} with {:?}",
        taken.status, taken.body
    );

    // A simulate is bounded in BYTES: a pipeline whose processors inject enough to make the
    // per-step report larger than one answer may be is refused, while the same pipeline
    // answers the final document without the per-step reports.
    let injected = "x".repeat(150_000);
    let processors: Vec<Value> = (0..5)
        .map(|index| json!({ "set": { "field": format!("blob_{index}"), "value": injected } }))
        .collect();
    let declared = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        "/_ingest/pipeline/pipe-large",
        Some(json!({ "processors": processors })),
    )
    .await;
    assert_eq!(
        declared.status, 200,
        "case \"a pipeline that injects a large value\": expected it to be declared, got {} \
         with {:?}",
        declared.status, declared.body
    );
    let docs = json!({ "docs": [{ "_index": "events", "_id": "1", "_source": { "m": "a" } }] });
    let verbose = rest_prefixed(
        &emulator,
        domain,
        "POST",
        "/_ingest/pipeline/pipe-large/_simulate?verbose=true",
        Some(docs.clone()),
    )
    .await;
    assert_eq!(
        (verbose.status, error_type(&verbose).as_str()),
        (400, "illegal_argument_exception"),
        "case \"a verbose simulate past the answer bound\": expected the cluster to refuse it, \
         got {} with {:?}",
        verbose.status,
        verbose.body
    );
    let plain = rest_prefixed(
        &emulator,
        domain,
        "POST",
        "/_ingest/pipeline/pipe-large/_simulate",
        Some(docs),
    )
    .await;
    assert_eq!(
        plain.status, 200,
        "case \"the same simulate without the per-step reports\": expected it to be served, \
         got {} with {:?}",
        plain.status, plain.body
    );
    assert_eq!(
        plain.body["docs"][0]["doc"]["_source"]["blob_4"]
            .as_str()
            .map(str::len),
        Some(injected.len()),
        "case \"the same simulate without the per-step reports\": expected the last \
         processor's value in the answered document"
    );
}

/// Every LEAF name an expanded settings report carries, as the flat form spells it.
fn leaf_names(reported: &Value, prefix: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some(held) = reported.as_object() else {
        return names;
    };
    for (key, value) in held {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if value.is_object() {
            names.extend(leaf_names(value, &name));
        } else {
            names.insert(name);
        }
    }
    names
}

/// One `action.auto_create_index` value to write a document under: the value, the index the
/// write names, and whether the cluster creates it.
struct AutoCreateRow {
    case: &'static str,
    domain: &'static str,
    value: Option<&'static str>,
    index: &'static str,
    created: bool,
}

/// The table: the booleans, both pattern signs, and a name no pattern matches.
fn auto_create_rows() -> Vec<AutoCreateRow> {
    vec![
        AutoCreateRow {
            case: "no value: the documented default permits every index",
            domain: "auto-default",
            value: None,
            index: "logs-2026",
            created: true,
        },
        AutoCreateRow {
            case: "true permits every index",
            domain: "auto-true",
            value: Some("true"),
            index: "logs-2026",
            created: true,
        },
        AutoCreateRow {
            case: "false permits none",
            domain: "auto-false",
            value: Some("false"),
            index: "logs-2026",
            created: false,
        },
        AutoCreateRow {
            case: "an allowing pattern the name matches",
            domain: "auto-allowed",
            value: Some("+logs-*,-*"),
            index: "logs-2026",
            created: true,
        },
        AutoCreateRow {
            case: "a blocking pattern the name matches",
            domain: "auto-blocked",
            value: Some("-logs-*,+*"),
            index: "logs-2026",
            created: false,
        },
        AutoCreateRow {
            case: "a bare pattern is an allow",
            domain: "auto-bare",
            value: Some("logs-*"),
            index: "logs-2026",
            created: true,
        },
        AutoCreateRow {
            case: "a name no pattern matches",
            domain: "auto-unmatched",
            value: Some("logs-*"),
            index: "metrics-2026",
            created: false,
        },
    ]
}

/// RULE: `action.auto_create_index` decides whether a write to an index the cluster does not
/// hold creates it — and an index the cluster DOES hold takes the write whatever the setting
/// says, because nothing is being created.
#[tokio::test]
async fn auto_create_index_decides_whether_a_write_mints_the_index_it_names() {
    let emulator = Emulator::new();

    for row in auto_create_rows() {
        let case = row.case;
        given_domain(&emulator, row.domain, None).await;
        if let Some(value) = row.value {
            let applied = rest_prefixed(
                &emulator,
                row.domain,
                "PUT",
                "/_cluster/settings",
                Some(json!({ "persistent": { "action.auto_create_index": value } })),
            )
            .await;
            assert_eq!(
                applied.status, 200,
                "case {case:?}: expected 200 from the settings update, got {} with {:?}",
                applied.status, applied.body
            );
        }

        let written = rest_prefixed(
            &emulator,
            row.domain,
            "PUT",
            &format!("/{}/_doc/1", row.index),
            Some(json!({ "message": "hello" })),
        )
        .await;
        if row.created {
            assert_eq!(
                written.status, 201,
                "case {case:?}: expected the write to create the index, got {} with {:?}",
                written.status, written.body
            );
            let read = rest_prefixed(
                &emulator,
                row.domain,
                "GET",
                &format!("/{}/_doc/1", row.index),
                None,
            )
            .await;
            assert_eq!(
                read.body["_source"]["message"].as_str(),
                Some("hello"),
                "case {case:?}: expected the document to read back, got {:?}",
                read.body
            );
        } else {
            assert_eq!(
                (written.status, error_type(&written).as_str()),
                (404, "index_not_found_exception"),
                "case {case:?}: expected the write to be refused as a missing index, got {} \
                 with {:?}",
                written.status,
                written.body
            );
            // The refusal names the index asked for and the setting that refused it.
            let reason = written.body["error"]["reason"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                reason.contains(&format!("no such index [{}]", row.index))
                    && reason.contains("action.auto_create_index"),
                "case {case:?}: expected the reason to name the index and the setting, got \
                 {reason:?}"
            );

            // An index the cluster HOLDS takes the write: the setting governs creation only.
            let made = rest_prefixed(
                &emulator,
                row.domain,
                "PUT",
                &format!("/{}", row.index),
                Some(json!({})),
            )
            .await;
            assert_eq!(
                made.status, 200,
                "case {case:?}: expected an explicit create to be served, got {} with {:?}",
                made.status, made.body
            );
            let again = rest_prefixed(
                &emulator,
                row.domain,
                "PUT",
                &format!("/{}/_doc/1", row.index),
                Some(json!({ "message": "hello" })),
            )
            .await;
            assert_eq!(
                again.status, 201,
                "case {case:?}: expected the write into the held index to be taken, got {} \
                 with {:?}",
                again.status,
                again.body
            );
        }
    }
}

/// One pipeline to declare and run: its processors, the source a write carries, and the
/// source the cluster stores afterwards.
struct PipelineRow {
    case: &'static str,
    id: &'static str,
    processors: Value,
    source: Value,
    stored: Value,
}

/// The table: every processor type the engine registers, each over a source it changes.
fn pipeline_rows() -> Vec<PipelineRow> {
    vec![
        PipelineRow {
            case: "set adds a field",
            id: "pipe-set",
            processors: json!([{ "set": { "field": "env", "value": "prod" } }]),
            source: json!({ "message": "hello" }),
            stored: json!({ "message": "hello", "env": "prod" }),
        },
        PipelineRow {
            case: "set writes a nested path",
            id: "pipe-set-nested",
            processors: json!([{ "set": { "field": "meta.env", "value": "prod" } }]),
            source: json!({ "message": "hello" }),
            stored: json!({ "message": "hello", "meta": { "env": "prod" } }),
        },
        PipelineRow {
            case: "rename moves a field",
            id: "pipe-rename",
            processors: json!([{ "rename": { "field": "msg", "target_field": "message" } }]),
            source: json!({ "msg": "hello" }),
            stored: json!({ "message": "hello" }),
        },
        PipelineRow {
            case: "remove drops the fields it names",
            id: "pipe-remove",
            processors: json!([{ "remove": { "field": ["secret", "token"] } }]),
            source: json!({ "message": "hello", "secret": "s", "token": "t" }),
            stored: json!({ "message": "hello" }),
        },
        PipelineRow {
            case: "lowercase rewrites one field",
            id: "pipe-lower",
            processors: json!([{ "lowercase": { "field": "level" } }]),
            source: json!({ "level": "WARN" }),
            stored: json!({ "level": "warn" }),
        },
        PipelineRow {
            case: "uppercase rewrites into another field",
            id: "pipe-upper",
            processors: json!([{
                "uppercase": { "field": "level", "target_field": "level_upper" }
            }]),
            source: json!({ "level": "warn" }),
            stored: json!({ "level": "warn", "level_upper": "WARN" }),
        },
        PipelineRow {
            case: "trim strips the surrounding space",
            id: "pipe-trim",
            processors: json!([{ "trim": { "field": "message" } }]),
            source: json!({ "message": "  hello  " }),
            stored: json!({ "message": "hello" }),
        },
        PipelineRow {
            case: "the processors run in the order they are declared",
            id: "pipe-chain",
            processors: json!([
                { "rename": { "field": "msg", "target_field": "message" } },
                { "trim": { "field": "message" } },
                { "uppercase": { "field": "message" } },
                { "set": { "field": "stage", "value": "ingested" } },
            ]),
            source: json!({ "msg": "  hello  " }),
            stored: json!({ "message": "HELLO", "stage": "ingested" }),
        },
        PipelineRow {
            case: "a field the source does not carry is skipped where the processor says so",
            id: "pipe-ignore-missing",
            processors: json!([{
                "rename": {
                    "field": "absent",
                    "target_field": "message",
                    "ignore_missing": true,
                }
            }]),
            source: json!({ "message": "hello" }),
            stored: json!({ "message": "hello" }),
        },
    ]
}

/// RULE: an ingest pipeline rewrites the source a write stores, and `_simulate` predicts
/// exactly that — one reading of the pipeline serves both.
#[tokio::test]
async fn an_ingest_pipeline_rewrites_the_stored_source_and_simulate_predicts_it() {
    let emulator = Emulator::new();
    let domain = "pipeline-target";
    given_domain(&emulator, domain, None).await;

    for row in pipeline_rows() {
        let case = row.case;
        let declaration = json!({
            "description": case,
            "processors": row.processors.clone(),
        });
        let declared = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            &format!("/_ingest/pipeline/{}", row.id),
            Some(declaration.clone()),
        )
        .await;
        assert_eq!(
            declared.status, 200,
            "case {case:?}: expected 200 from the pipeline create, got {} with {:?}",
            declared.status, declared.body
        );

        // The pipeline reads back as it was declared.
        let read = rest_prefixed(
            &emulator,
            domain,
            "GET",
            &format!("/_ingest/pipeline/{}", row.id),
            None,
        )
        .await;
        assert_eq!(
            read.body[row.id], declaration,
            "case {case:?}: expected the pipeline to read back as declared, got {:?}",
            read.body
        );

        // The simulate answers the source a write through this pipeline would store.
        let simulated = rest_prefixed(
            &emulator,
            domain,
            "POST",
            &format!("/_ingest/pipeline/{}/_simulate", row.id),
            Some(json!({
                "docs": [{ "_index": "events", "_id": "1", "_source": row.source.clone() }]
            })),
        )
        .await;
        assert_eq!(
            simulated.status, 200,
            "case {case:?}: expected 200 from the simulate, got {} with {:?}",
            simulated.status, simulated.body
        );
        assert_eq!(
            simulated.body["docs"][0]["doc"]["_source"], row.stored,
            "case {case:?}: expected the simulate to predict {:?}, got {:?}",
            row.stored, simulated.body
        );

        // And the write through the same pipeline stores exactly that.
        let written = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            &format!("/events/_doc/{}?pipeline={}", row.id, row.id),
            Some(row.source.clone()),
        )
        .await;
        assert_eq!(
            written.status, 201,
            "case {case:?}: expected the write to be taken, got {} with {:?}",
            written.status, written.body
        );
        let stored = rest_prefixed(
            &emulator,
            domain,
            "GET",
            &format!("/events/_doc/{}", row.id),
            None,
        )
        .await;
        assert_eq!(
            stored.body["_source"], row.stored,
            "case {case:?}: expected the stored source to be what the simulate predicted \
             ({:?}), got {:?}",
            row.stored, stored.body
        );

        // A write naming no pipeline is stored unprocessed: the pipeline runs only where the
        // write names it.
        let plain = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            &format!("/events/_doc/plain-{}", row.id),
            Some(row.source.clone()),
        )
        .await;
        assert_eq!(
            plain.status, 201,
            "case {case:?}: expected the unprocessed write to be taken, got {} with {:?}",
            plain.status, plain.body
        );
        let unprocessed = rest_prefixed(
            &emulator,
            domain,
            "GET",
            &format!("/events/_doc/plain-{}", row.id),
            None,
        )
        .await;
        assert_eq!(
            unprocessed.body["_source"], row.source,
            "case {case:?}: expected a write naming no pipeline to store its own source \
             ({:?}), got {:?}",
            row.source, unprocessed.body
        );
    }

    // A pipeline the cluster does not hold is a missing resource, for the simulate and for a
    // write naming it.
    for (what, answer) in [
        (
            "simulate",
            rest_prefixed(
                &emulator,
                domain,
                "POST",
                "/_ingest/pipeline/no-such-pipe/_simulate",
                Some(json!({ "docs": [] })),
            )
            .await,
        ),
        (
            "write",
            rest_prefixed(
                &emulator,
                domain,
                "PUT",
                "/events/_doc/x?pipeline=no-such-pipe",
                Some(json!({ "message": "hello" })),
            )
            .await,
        ),
        (
            "read",
            rest_prefixed(&emulator, domain, "GET", "/_ingest/pipeline/no-such-pipe", None).await,
        ),
    ] {
        assert_eq!(
            (answer.status, error_type(&answer).as_str()),
            (404, "resource_not_found_exception"),
            "case \"a pipeline the cluster does not hold ({what})\": expected the engine's \
             missing-pipeline refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // A declaration the engine will not run is REFUSED rather than stored: a processor type
    // it does not register, and a processor missing an option its type requires.
    for (case, processors, reason) in [
        (
            "a processor type the engine does not register",
            json!([{ "attachment": { "field": "data" } }]),
            "No processor type exists with name [attachment]",
        ),
        (
            "a set processor naming no field",
            json!([{ "set": { "value": "prod" } }]),
            "[field] required property is missing",
        ),
        (
            "a rename processor naming no target",
            json!([{ "rename": { "field": "msg" } }]),
            "[target_field] required property is missing",
        ),
    ] {
        let refused = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            "/_ingest/pipeline/pipe-refused",
            Some(json!({ "processors": processors })),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.body["error"]["reason"].as_str().unwrap_or_default()
            ),
            (400, reason),
            "case {case:?}: expected the engine's own refusal, got {} with {:?}",
            refused.status,
            refused.body
        );
        let held = rest_prefixed(
            &emulator,
            domain,
            "GET",
            "/_ingest/pipeline/pipe-refused",
            None,
        )
        .await;
        assert_eq!(
            held.status, 404,
            "case {case:?}: a refused declaration stores no pipeline, got {} with {:?}",
            held.status, held.body
        );
    }

    // A processor that cannot act on the document refuses the WRITE, so nothing is stored
    // half-processed.
    let strict = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        "/_ingest/pipeline/pipe-strict",
        Some(json!({
            "processors": [{ "rename": { "field": "absent", "target_field": "message" } }]
        })),
    )
    .await;
    assert_eq!(
        strict.status, 200,
        "case \"a strict pipeline\": expected it to be declared, got {} with {:?}",
        strict.status, strict.body
    );
    let refused = rest_prefixed(
        &emulator,
        domain,
        "PUT",
        "/events/_doc/strict?pipeline=pipe-strict",
        Some(json!({ "message": "hello" })),
    )
    .await;
    assert_eq!(
        (
            refused.status,
            refused.body["error"]["reason"].as_str().unwrap_or_default()
        ),
        (400, "field [absent] doesn't exist"),
        "case \"a processor that cannot act on the document\": expected the write to be \
         refused, got {} with {:?}",
        refused.status,
        refused.body
    );
    let stored = rest_prefixed(&emulator, domain, "GET", "/events/_doc/strict", None).await;
    assert_eq!(
        stored.status, 404,
        "case \"a refused write\": expected nothing to be stored, got {} with {:?}",
        stored.status, stored.body
    );

    // And a pipeline is removable, after which nothing holds it.
    let deleted = rest_prefixed(
        &emulator,
        domain,
        "DELETE",
        "/_ingest/pipeline/pipe-strict",
        None,
    )
    .await;
    assert_eq!(
        (deleted.status, deleted.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"deleting a pipeline\": expected it to be acknowledged, got {} with {:?}",
        deleted.status,
        deleted.body
    );
    let gone = rest_prefixed(
        &emulator,
        domain,
        "GET",
        "/_ingest/pipeline/pipe-strict",
        None,
    )
    .await;
    assert_eq!(
        gone.status, 404,
        "case \"a deleted pipeline\": expected it to be gone, got {} with {:?}",
        gone.status, gone.body
    );
}

/// The policy documents the index-management rows write, each a different shape of policy so
/// the record is the document rather than one remembered body.
fn ism_policies() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "a policy with one state",
            "ism-one",
            json!({
                "description": "hot only",
                "default_state": "hot",
                "states": [{ "name": "hot", "actions": [], "transitions": [] }],
            }),
        ),
        (
            "a policy whose state transitions on an age condition",
            "ism-transition",
            json!({
                "description": "hot then read-only",
                "default_state": "hot",
                "states": [
                    {
                        "name": "hot",
                        "actions": [],
                        "transitions": [
                            { "state_name": "readonly", "conditions": { "min_index_age": "1d" } }
                        ],
                    },
                    {
                        "name": "readonly",
                        "actions": [{ "read_only": {} }],
                        "transitions": [],
                    },
                ],
            }),
        ),
        (
            "a policy carrying an ism_template",
            "ism-template",
            json!({
                "description": "applied by template",
                "default_state": "hot",
                "states": [{ "name": "hot", "actions": [], "transitions": [] }],
                "ism_template": [{ "index_patterns": ["logs-*"], "priority": 100 }],
            }),
        ),
    ]
}

/// RULE: an index-management policy is a revisioned record — creating one answers `201` with
/// the revision counters of the write, reading it back answers the document that was
/// written, and updating it is a write against the revision the caller read.
#[tokio::test]
async fn an_index_management_policy_is_stored_revisioned_and_read_back() {
    let emulator = Emulator::new();
    let domain = "ism-target";
    given_domain(&emulator, domain, None).await;

    let mut seen_seq_no: BTreeSet<i64> = BTreeSet::new();
    for (case, id, policy) in ism_policies() {
        let created = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            &format!("/_plugins/_ism/policies/{id}"),
            Some(json!({ "policy": policy.clone() })),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from the policy create, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            (
                created.body["_id"].as_str(),
                created.body["_version"].as_i64(),
                created.body["_primary_term"].as_i64(),
            ),
            (Some(id), Some(1), Some(1)),
            "case {case:?}: expected the revision counters of a created policy, got {:?}",
            created.body
        );
        let seq_no = created.body["_seq_no"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a _seq_no, got {:?}", created.body)
        });
        assert!(
            seen_seq_no.insert(seq_no),
            "case {case:?}: expected every write to take its own sequence number, got \
             {seq_no} again"
        );

        // The policy reads back as it was written, with the two members the plugin fills in
        // itself: the id the path addressed it by, and when it was last written.
        let read = rest_prefixed(
            &emulator,
            domain,
            "GET",
            &format!("/_plugins/_ism/policies/{id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from the policy read, got {} with {:?}",
            read.status, read.body
        );
        let reported = &read.body["policy"]["policy"];
        assert_eq!(
            reported["policy_id"].as_str(),
            Some(id),
            "case {case:?}: expected the policy to carry its own id, got {reported:?}"
        );
        assert!(
            reported["last_updated_time"].as_i64().is_some_and(|at| at > 0),
            "case {case:?}: expected the write's own timestamp, got {reported:?}"
        );
        for (member, written) in policy.as_object().expect("the row's policy is an object") {
            assert_eq!(
                &reported[member], written,
                "case {case:?}: expected the stored policy to carry {member} as written \
                 ({written:?}), got {:?}",
                reported[member]
            );
        }
        // Nothing the client did not write is added beside them.
        let reported_members: BTreeSet<&str> = reported
            .as_object()
            .expect("the reported policy is an object")
            .keys()
            .map(String::as_str)
            .collect();
        let expected_members: BTreeSet<&str> = policy
            .as_object()
            .expect("the row's policy is an object")
            .keys()
            .map(String::as_str)
            .chain(["policy_id", "last_updated_time"])
            .collect();
        assert_eq!(
            reported_members, expected_members,
            "case {case:?}: expected exactly the written members plus the plugin's own"
        );

        // An update is a write against the revision the caller read: without it the write
        // conflicts, and with it the policy moves to its next version.
        let conflicting = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            &format!("/_plugins/_ism/policies/{id}"),
            Some(json!({ "policy": policy.clone() })),
        )
        .await;
        assert_eq!(
            (conflicting.status, error_type(&conflicting).as_str()),
            (409, "version_conflict_engine_exception"),
            "case {case:?}: expected an update naming no revision to conflict, got {} with \
             {:?}",
            conflicting.status,
            conflicting.body
        );
        let mut updated_policy = policy.clone();
        updated_policy["description"] = json!("updated");
        let updated = rest_prefixed(
            &emulator,
            domain,
            "PUT",
            &format!(
                "/_plugins/_ism/policies/{id}?if_seq_no={seq_no}&if_primary_term=1"
            ),
            Some(json!({ "policy": updated_policy })),
        )
        .await;
        assert_eq!(
            (updated.status, updated.body["_version"].as_i64()),
            (200, Some(2)),
            "case {case:?}: expected the update to be taken at the next version, got {} with \
             {:?}",
            updated.status,
            updated.body
        );
        assert_eq!(
            updated.body["policy"]["policy"]["description"].as_str(),
            Some("updated"),
            "case {case:?}: expected the updated document, got {:?}",
            updated.body
        );
    }

    // A policy the cluster does not hold is not found, and a deleted one is gone.
    let missing = rest_prefixed(
        &emulator,
        domain,
        "GET",
        "/_plugins/_ism/policies/no-such-policy",
        None,
    )
    .await;
    assert_eq!(
        (
            missing.status,
            missing.body["error"]["reason"].as_str().unwrap_or_default()
        ),
        (404, "Policy not found"),
        "case \"a policy the cluster does not hold\": expected the plugin's own refusal, got \
         {} with {:?}",
        missing.status,
        missing.body
    );
    let deleted = rest_prefixed(
        &emulator,
        domain,
        "DELETE",
        "/_plugins/_ism/policies/ism-one",
        None,
    )
    .await;
    assert_eq!(
        (deleted.status, deleted.body["result"].as_str()),
        (200, Some("deleted")),
        "case \"deleting a policy\": expected the plugin's own answer, got {} with {:?}",
        deleted.status,
        deleted.body
    );
    let gone = rest_prefixed(
        &emulator,
        domain,
        "GET",
        "/_plugins/_ism/policies/ism-one",
        None,
    )
    .await;
    assert_eq!(
        gone.status, 404,
        "case \"a deleted policy\": expected it to be gone, got {} with {:?}",
        gone.status, gone.body
    );
}
