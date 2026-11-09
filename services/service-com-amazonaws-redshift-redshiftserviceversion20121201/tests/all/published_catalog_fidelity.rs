// SPDX-License-Identifier: CC0-1.0

//! The rule every pure-read Redshift catalog answers from: what the catalog
//! PUBLISHES, over the awsQuery wire, and the agreement between a catalog read
//! and the operations that consume the same table.
//!
//! Three rules, each asserted over a table of inputs rather than one recorded
//! request:
//!
//! * the orderable-shape catalog offers a pairing for every node type a NEW
//!   cluster can be ordered with and the cluster types that node type is
//!   orderable as — the previous-generation family is offered for neither, and
//!   `single-node` only for the entry-size members — and every pairing it offers
//!   is a cluster `CreateCluster` really creates;
//! * the event catalog returns TEMPLATES: every published `EventDescription`
//!   writes the values an occurrence fills as `<placeholder>`, and the instant it
//!   names as `<time in UTC>`;
//! * a raised event's message is that same published template with its
//!   placeholders filled, so the catalog read and `DescribeEvents` cannot
//!   describe one event two ways.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Emulator};

/// Every node type the model publishes, in its documented order.
///
/// Source: `smithy_query(query="doc", shape="CreateClusterMessage",
/// member="NodeType")` — "Valid Values: dc2.large | dc2.8xlarge | rg.large |
/// rg.xlarge | rg.4xlarge | rg.12xlarge | ra3.large | ra3.xlplus | ra3.4xlarge |
/// ra3.16xlarge".
const NODE_TYPES: [&str; 10] = [
    "dc2.large",
    "dc2.8xlarge",
    "rg.large",
    "rg.xlarge",
    "rg.4xlarge",
    "rg.12xlarge",
    "ra3.large",
    "ra3.xlplus",
    "ra3.4xlarge",
    "ra3.16xlarge",
];

/// The cluster types one node type can be ORDERED as, per node type: the
/// previous generation is not ordered at all, the entry-size member of each
/// current-generation family is ordered as either, and every larger member as
/// `multi-node` alone.
///
/// Source: the orderable-shape catalog the service publishes (what
/// `DescribeOrderableClusterOptions` reports); the model bounds only the two
/// value lists and pairs neither with the other. Written out here, one row per
/// node type the model publishes, so a wrong pairing fails this suite.
const ORDERABLE: [(&str, &[&str]); 10] = [
    ("dc2.large", &[]),
    ("dc2.8xlarge", &[]),
    ("rg.large", &["multi-node", "single-node"]),
    ("rg.xlarge", &["multi-node"]),
    ("rg.4xlarge", &["multi-node"]),
    ("rg.12xlarge", &["multi-node"]),
    ("ra3.large", &["multi-node", "single-node"]),
    ("ra3.xlplus", &["multi-node", "single-node"]),
    ("ra3.4xlarge", &["multi-node"]),
    ("ra3.16xlarge", &["multi-node"]),
];

/// The `(node type, cluster type)` pairs a `DescribeOrderableClusterOptions`
/// body carries, in document order.
///
/// An `OrderableClusterOption` serializes one `NodeType` and one `ClusterType`,
/// so the two element sequences line up index by index.
fn listed_pairs(body: &str) -> Vec<(String, String)> {
    let node_types = all_text(body, "NodeType");
    let cluster_types = all_text(body, "ClusterType");
    assert_eq!(
        node_types.len(),
        cluster_types.len(),
        "an option carries one of each member: got {} node types and {} cluster types in body {body}",
        node_types.len(),
        cluster_types.len()
    );
    node_types.into_iter().zip(cluster_types).collect()
}

#[tokio::test]
async fn the_orderable_catalog_offers_exactly_the_shapes_a_new_cluster_is_ordered_with() {
    let emulator = Emulator::new();

    // The whole catalog first: every row of the table, and nothing else.
    let answer = emulator
        .call("DescribeOrderableClusterOptions", &[])
        .await;
    assert_eq!(
        answer.status, 200,
        "case the whole catalog: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let whole = listed_pairs(&answer.body);
    let expected: Vec<(String, String)> = ORDERABLE
        .iter()
        .flat_map(|(node_type, cluster_types)| {
            cluster_types
                .iter()
                .map(move |cluster_type| ((*node_type).to_string(), (*cluster_type).to_string()))
        })
        .collect();
    assert_eq!(
        whole, expected,
        "case the whole catalog: expected {expected:?}, got {whole:?}"
    );

    // Then one row per node type the model publishes, selected by name: the
    // catalog offers that node type's orderable cluster types and no others.
    for (node_type, cluster_types) in ORDERABLE {
        let selected = emulator
            .call(
                "DescribeOrderableClusterOptions",
                &[("NodeType", node_type)],
            )
            .await;
        assert_eq!(
            selected.status, 200,
            "case select {node_type}: expected 200, got {} with body {}",
            selected.status, selected.body
        );
        let served = listed_pairs(&selected.body);
        let expected: Vec<(String, String)> = cluster_types
            .iter()
            .map(|cluster_type| (node_type.to_string(), (*cluster_type).to_string()))
            .collect();
        assert_eq!(
            served, expected,
            "case select {node_type}: expected {expected:?}, got {served:?}"
        );
    }

    // The node types the model publishes and the ones the catalog prices an
    // offering of are the same list, so no read invents a node type of its own.
    assert_eq!(
        state::ORDERABLE_NODE_TYPES.to_vec(),
        NODE_TYPES.to_vec(),
        "case published node types: expected the documented list, got {:?}",
        state::ORDERABLE_NODE_TYPES
    );
}

#[tokio::test]
async fn every_shape_the_catalog_offers_is_a_cluster_the_service_creates() {
    let emulator = Emulator::new();

    // One create per offered pairing, read back through `DescribeClusters`: a
    // catalog that offers a shape the service then refuses would send a client
    // that validates against it to a cluster it cannot have.
    let mut created = 0;
    for (node_type, cluster_types) in ORDERABLE {
        for cluster_type in cluster_types {
            let identifier = format!(
                "orderable-{}-{}",
                node_type.replace('.', "-"),
                cluster_type
            );
            let mut params: Vec<(&str, &str)> = vec![
                ("ClusterIdentifier", &identifier),
                ("NodeType", node_type),
                ("MasterUsername", "admin"),
                ("ClusterType", cluster_type),
            ];
            if *cluster_type == "multi-node" {
                params.push(("NumberOfNodes", "2"));
            }
            let answer = emulator.call("CreateCluster", &params).await;
            assert_eq!(
                answer.status, 200,
                "case create {node_type} / {cluster_type}: expected 200, got {} / {:?} with body {}",
                answer.status,
                answer.error_code(),
                answer.body
            );
            let described = emulator
                .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
                .await;
            assert_eq!(
                (
                    described.status,
                    described.text("NodeType"),
                    described.text("ClusterIdentifier")
                ),
                (
                    200,
                    Some((*node_type).to_string()),
                    Some(identifier.clone())
                ),
                "case read back {node_type} / {cluster_type}: expected the created cluster, got body {}",
                described.body
            );
            created += 1;
        }
    }
    assert_eq!(
        created,
        ORDERABLE
            .iter()
            .map(|(_, cluster_types)| cluster_types.len())
            .sum::<usize>(),
        "case coverage: expected one create per offered pairing, made {created}"
    );
}

#[tokio::test]
async fn the_event_catalog_publishes_templates_and_a_raised_event_fills_one() {
    let emulator = Emulator::new();

    // Every published description, across every source type the catalog admits:
    // the values an occurrence fills are written as `<placeholder>`, never in
    // another bracket form, and no placeholder is left empty.
    let catalog = emulator.call("DescribeEventCategories", &[]).await;
    assert_eq!(
        catalog.status, 200,
        "case read the catalog: expected 200, got {} with body {}",
        catalog.status, catalog.body
    );
    let entries = element_blocks(&catalog.body, "EventInfoMap");
    assert!(
        entries.len() > 1,
        "case read the catalog: expected the published events, got body {}",
        catalog.body
    );
    let mut templates: Vec<(String, String)> = Vec::new();
    for entry in &entries {
        let event_id = all_text(entry, "EventId")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case catalog entry: expected an EventId in {entry}"));
        let description = all_text(entry, "EventDescription")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {event_id}: expected an EventDescription"));
        assert!(
            !description.contains('[') && !description.contains(']'),
            "case {event_id}: expected `<placeholder>` template text, got {description:?}"
        );
        let mut rest = description.as_str();
        while let Some(open) = rest.find('<') {
            let after = &rest[open + 1..];
            let close = after
                .find('>')
                .unwrap_or_else(|| panic!("case {event_id}: unclosed placeholder in {description:?}"));
            assert!(
                !after[..close].is_empty(),
                "case {event_id}: expected a named placeholder, got {description:?}"
            );
            rest = &after[close + 1..];
        }
        templates.push((event_id, description));
    }

    // The one placeholder filled from the clock is named for what it stands for,
    // so a client rendering an event id reads the same template AWS publishes.
    let timed: Vec<&(String, String)> = templates
        .iter()
        .filter(|(_, description)| description.contains("<time in UTC>"))
        .collect();
    assert!(
        !timed.is_empty(),
        "case the clock placeholder: expected published events naming their instant, got {templates:?}"
    );

    // A raised event's message is the published template with its placeholders
    // filled: the resource's name in quotes and the instant in UTC. Asserted on
    // the event a create raises, read back through `DescribeEvents`.
    let identifier = "catalog-template-cluster";
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed a cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let events = emulator
        .call("DescribeEvents", &[("SourceIdentifier", identifier), ("SourceType", "cluster")])
        .await;
    assert_eq!(
        events.status, 200,
        "case read the events: expected 200, got {} with body {}",
        events.status, events.body
    );
    let event_id = state::EVENT_ID_CLUSTER_CREATED;
    let reported = all_text(&events.body, "EventId");
    assert_eq!(
        reported,
        vec![event_id.to_string()],
        "case read the events: expected the create's own event, got {reported:?}"
    );
    let (_, template) = templates
        .iter()
        .find(|(published, _)| published == event_id)
        .unwrap_or_else(|| panic!("case {event_id}: the catalog read publishes no such event"));
    let message = all_text(&events.body, "Message")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {event_id}: expected a Message, got body {}", events.body));

    // The template's fixed text either side of each placeholder is the fixed
    // text of the message, with the cluster's own name quoted in between and the
    // instant rendered as `YYYY-MM-DD HH:MM UTC`.
    let filled = template.replace("<cluster name>", &format!("'{identifier}'"));
    let (prefix, suffix) = filled
        .split_once("<time in UTC>")
        .unwrap_or_else(|| panic!("case {event_id}: expected the clock placeholder in {template:?}"));
    assert!(
        message.starts_with(prefix) && message.ends_with(suffix),
        "case {event_id}: expected the published template {template:?} filled in, got {message:?}"
    );
    let instant = &message[prefix.len()..message.len() - suffix.len()];
    assert!(
        instant.len() == "YYYY-MM-DD HH:MM UTC".len()
            && instant.ends_with(" UTC")
            && instant
                .trim_end_matches(" UTC")
                .chars()
                .all(|character| character.is_ascii_digit()
                    || character == '-'
                    || character == ':'
                    || character == ' '),
        "case {event_id}: expected the instant in UTC, got {instant:?} from {message:?}"
    );
}
