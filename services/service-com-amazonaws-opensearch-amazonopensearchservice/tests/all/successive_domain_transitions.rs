// SPDX-License-Identifier: CC0-1.0

//! What a client that sequences two control-plane calls on ONE domain observes, driven
//! over the real wire path: a `restJson1` request through the multi-service router into
//! the mounted service, so what is asserted is what a polling client sees.
//!
//! ONE rule, over every call that puts a domain into a processing state:
//!
//! > a call that changes a domain's configuration owes that domain exactly ONE span of
//! > processing — `DomainStatus$Processing` true with `DomainProcessingStatus` naming what
//! > is in flight while the span is open — and the FIRST read taken once that span has
//! > elapsed publishes the idle configuration (`Processing` false,
//! > `DomainProcessingStatus` `Active`, the change `Completed`, the data-plane endpoint
//! > published), whichever read it is: a read inside the span neither postpones the settle
//! > nor is needed to trigger it, so one wait per call is all a client spends, and every
//! > operation gated on an idle domain is served from that same instant.
//!
//! The rows span the calls that start such a span — the create, an update that reshapes
//! the cluster (a blue/green change), an update that only sets a dynamic option, and an
//! update that only sets an advanced option — taken back to back on the SAME domain, which
//! is what a client sequencing control-plane work does. Each is asserted in the same four
//! positions: the answer the call itself gives, a read inside the span, the FIRST read past
//! it, and a repeat of that read.
//!
//! The axis that is NEW here is the SECOND and later span on one domain and the probe that
//! polling inside a span does not postpone its settle: the create's own span is already
//! covered by [`super::clock_owed_readiness`] (across every resource kind this control plane
//! mints) and by [`super::domain_lifecycle`], which own that rule for the create.
//!
//! The second test takes the whole sequence a client builds out of those waits —
//! create, wait, maintenance refused on a single-data-node domain, scale out, wait,
//! the same maintenance accepted and listed — so what follows the waits is asserted to be
//! served rather than unreachable.
//!
//! The control-plane harness comes from [`super::domain_lifecycle`], the maintenance calls
//! from [`super::domain_maintenance`] and the endpoint listing from
//! [`super::vpc_endpoints_for_domain`]; this suite adds no second copy of any of them.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::service::domain_configuration::CONFIG_CHANGE_PENDING;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    CONFIG_CHANGE_APPLYING_CHANGES, CONFIG_CHANGE_COMPLETED, MAINTAINABLE_DATA_NODES,
    NODE_TYPE_DATA, PROCESSING_STATUS_ACTIVE, PROCESSING_STATUS_CREATING,
    PROCESSING_STATUS_MODIFYING,
};

use super::domain_lifecycle::{error_code, string_member, Emulator, SETTLE_WAIT};
use super::domain_maintenance::{census, list, start, status};
use super::vpc_endpoints_for_domain::list_for_domain;

/// The instance type every row below provisions its data nodes on.
const INSTANCE_TYPE: &str = "t3.small.search";

/// One call that puts a domain into a processing state: the name a failure reports it
/// by, the `UpdateDomainConfig` body it is made with (`None` for the create that mints
/// the domain), and the `DomainProcessingStatusType` value the span it opens is reported
/// under, and the `ConfigChangeStatus` the change it started carries while that span is
/// open — `Pending` for the change a create starts, since a configuration cannot be
/// applied to a cluster that is not provisioned yet, and `ApplyingChanges` for a change
/// on a cluster that is already running.
struct Transition {
    case: &'static str,
    update: Option<Value>,
    transient: &'static str,
    transient_change: &'static str,
}

/// The rows: the create, then the three kinds of update that change something — a
/// cluster-shaping change, a snapshot-window change and an advanced-option change — in
/// the order a client issues them, each on the domain the row before it left idle.
///
/// The status values are this service's own statements of the modelled
/// `DomainProcessingStatusType` values, imported rather than spelled here so no row can
/// drift from what the handlers report.
fn transitions() -> Vec<Transition> {
    vec![
        Transition {
            case: "the create that mints the domain",
            update: None,
            transient: PROCESSING_STATUS_CREATING,
            transient_change: CONFIG_CHANGE_PENDING,
        },
        Transition {
            case: "an update that scales the data nodes out",
            update: Some(json!({
                "ClusterConfig": {
                    "InstanceType": INSTANCE_TYPE,
                    "InstanceCount": MAINTAINABLE_DATA_NODES,
                    "DedicatedMasterEnabled": false,
                    "ZoneAwarenessEnabled": false,
                },
            })),
            transient: PROCESSING_STATUS_MODIFYING,
            transient_change: CONFIG_CHANGE_APPLYING_CHANGES,
        },
        Transition {
            case: "an update that moves the automated snapshot hour",
            update: Some(json!({ "SnapshotOptions": { "AutomatedSnapshotStartHour": 3 } })),
            transient: PROCESSING_STATUS_MODIFYING,
            transient_change: CONFIG_CHANGE_APPLYING_CHANGES,
        },
        Transition {
            case: "an update that sets an advanced option",
            update: Some(json!({
                "AdvancedOptions": { "rest.action.multi.allow_explicit_index": "true" },
            })),
            transient: PROCESSING_STATUS_MODIFYING,
            transient_change: CONFIG_CHANGE_APPLYING_CHANGES,
        },
    ]
}

/// The `CreateDomain` body of the domain every row acts on: one data node on a public
/// endpoint, which is the smallest domain this control plane provisions.
fn single_node_create(name: &str) -> Value {
    json!({
        "DomainName": name,
        "EngineVersion": "OpenSearch_2.11",
        "ClusterConfig": {
            "InstanceType": INSTANCE_TYPE,
            "InstanceCount": 1,
            "DedicatedMasterEnabled": false,
            "ZoneAwarenessEnabled": false,
        },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
    })
}

/// What a readiness poll reads off `DescribeDomain`: the `Processing` flag, the
/// processing status, the published endpoint and the status of the change in flight.
#[derive(Debug, PartialEq, Eq)]
struct Readiness {
    processing: Option<bool>,
    processing_status: Option<String>,
    endpoint_published: bool,
    config_change_status: Option<String>,
}

/// `DescribeDomain`, read as a polling client reads it.
async fn readiness(emulator: &Emulator, domain: &str, case: &str) -> Readiness {
    let answer = emulator.describe(domain).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from DescribeDomain({domain:?}), got {} with {:?}",
        answer.status, answer.body
    );
    let status = &answer.body["DomainStatus"];
    Readiness {
        processing: status["Processing"].as_bool(),
        processing_status: status["DomainProcessingStatus"].as_str().map(str::to_string),
        endpoint_published: status["Endpoint"]
            .as_str()
            .is_some_and(|endpoint| !endpoint.is_empty()),
        config_change_status: status["ChangeProgressDetails"]["ConfigChangeStatus"]
            .as_str()
            .map(str::to_string),
    }
}

/// RULE: every call that changes a domain's configuration owes it ONE span of processing,
/// and the first read past that span publishes the idle configuration — a read inside the
/// span neither postpones the settle nor is needed to trigger it.
#[tokio::test]
async fn each_transition_settles_on_the_first_read_past_its_own_span() {
    let emulator = Emulator::new();
    let domain = "sequenced-domain";

    // The rows read the service's own constants, so the modelled SPELLINGS are pinned here
    // once: `smithy_query(query="shape", shape="DomainProcessingStatusType")` publishes
    // Creating / Active / Modifying / UpgradingEngineVersion / UpdatingServiceSoftware /
    // Isolated / Deleting, and `shape="ConfigChangeStatus"` publishes Pending /
    // Initializing / Validating / ValidationFailed / ApplyingChanges / Completed /
    // PendingUserInput / Cancelled. A constant renamed or re-spelled in the implementation
    // therefore cannot take an expectation below with it.
    assert_eq!(
        [
            PROCESSING_STATUS_CREATING,
            PROCESSING_STATUS_MODIFYING,
            PROCESSING_STATUS_ACTIVE,
        ],
        ["Creating", "Modifying", "Active"],
        "case the modelled spellings: the rows expect DomainProcessingStatusType's own values"
    );
    assert_eq!(
        [
            CONFIG_CHANGE_PENDING,
            CONFIG_CHANGE_APPLYING_CHANGES,
            CONFIG_CHANGE_COMPLETED,
        ],
        ["Pending", "ApplyingChanges", "Completed"],
        "case the modelled spellings: the rows expect ConfigChangeStatus's own values"
    );

    for transition in transitions() {
        let case = transition.case;

        // The call that opens the span, and what it answers for itself.
        match &transition.update {
            None => {
                let answer = emulator.create(single_node_create(domain)).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200 from CreateDomain, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["DomainStatus"]["DomainProcessingStatus"].as_str(),
                    Some(transition.transient),
                    "case {case}: the call answers the status of the work it started, got {:?}",
                    answer.body["DomainStatus"]
                );
            }
            Some(body) => {
                let answer = emulator.update_config(domain, body.clone()).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200 from UpdateDomainConfig, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.body["DomainConfig"]["ChangeProgressDetails"]["ConfigChangeStatus"]
                        .as_str(),
                    Some(transition.transient_change),
                    "case {case}: the call answers the change it started as in flight, got {:?}",
                    answer.body["DomainConfig"]["ChangeProgressDetails"]
                );
            }
        }

        // Inside the span: the reads a client takes straight after the call report the
        // transient state, which is what keeps the in-flight state and the refusals that
        // go with it reachable. Two of them, because polling must not postpone the settle:
        // the due instant belongs to the clock, not to whoever looks.
        for probe in 1..=2 {
            let inside = readiness(&emulator, domain, case).await;
            assert_eq!(
                (
                    inside.processing,
                    inside.processing_status.as_deref(),
                    inside.config_change_status.as_deref(),
                ),
                (
                    Some(true),
                    Some(transition.transient),
                    Some(transition.transient_change),
                ),
                "case {case} (read {probe} inside the span): expected the work reported in \
                 flight, got {inside:?}"
            );
        }

        tokio::time::sleep(SETTLE_WAIT).await;

        // The FIRST read past the span publishes the idle configuration: one wait per
        // call is all a client spends, and no further poll is needed to materialise it.
        let settled = readiness(&emulator, domain, case).await;
        assert_eq!(
            settled,
            Readiness {
                processing: Some(false),
                processing_status: Some(PROCESSING_STATUS_ACTIVE.to_string()),
                endpoint_published: true,
                config_change_status: Some(CONFIG_CHANGE_COMPLETED.to_string()),
            },
            "case {case}: expected the first read past the span to publish an idle domain, \
             got {settled:?}"
        );

        // And it stays idle: a read reports the same domain the read before it did.
        let again = readiness(&emulator, domain, case).await;
        assert_eq!(
            again, settled,
            "case {case}: a later read must report the same domain, got {again:?} after \
             {settled:?}"
        );
    }
}

/// RULE, continued: every operation gated on an idle domain is served from the instant
/// the span closes — so the sequence a client builds out of those waits runs through,
/// with the gate deciding on the configuration each wait left in place.
#[tokio::test]
async fn the_sequence_built_out_of_those_waits_is_served() {
    let emulator = Emulator::new();
    let domain = "maintenance-sequence";

    let created = emulator.create(single_node_create(domain)).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({domain:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    // The node census is served on the first read past the create's span, and publishes
    // the one data node the domain was provisioned with.
    let nodes = census(&emulator, domain).await;
    let data_nodes: Vec<String> = nodes
        .iter()
        .filter(|(_, node_type)| node_type == NODE_TYPE_DATA)
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(
        data_nodes.len(),
        1,
        "case one data node: expected the census to publish the single data node, got {nodes:?}"
    );

    // Maintenance takes a data node out of the cluster, so a role that cannot lose one is
    // refused — and refused without recording anything, which the listing shows.
    let refused = start(
        &emulator,
        domain,
        "RESTART_SEARCH_PROCESS",
        Some(data_nodes[0].as_str()),
    )
    .await;
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case maintenance on the only data node: expected ResourceNotFoundException, got {} with \
         {:?}",
        refused.status,
        refused.body
    );
    let listed = list(&emulator, domain, "").await;
    assert_eq!(
        (
            listed.status,
            listed.body["DomainMaintenances"].as_array().map(Vec::len),
        ),
        (200, Some(0)),
        "case a refused maintenance: expected no record written, got {} with {:?}",
        listed.status,
        listed.body
    );

    // A domain nobody created an interface endpoint for reports an empty set rather than
    // a refusal — the read a client takes between the two waits.
    let endpoints = list_for_domain(&emulator, domain, None).await;
    assert_eq!(
        (
            endpoints.status,
            endpoints.body["VpcEndpointSummaryList"]
                .as_array()
                .map(Vec::len),
        ),
        (200, Some(0)),
        "case no endpoint for the domain: expected an empty list, got {} with {:?}",
        endpoints.status,
        endpoints.body
    );

    // Scaling the data nodes out is the second transition, and the same single wait
    // closes it.
    let scaled = emulator
        .update_config(
            domain,
            json!({
                "ClusterConfig": {
                    "InstanceType": INSTANCE_TYPE,
                    "InstanceCount": MAINTAINABLE_DATA_NODES,
                    "DedicatedMasterEnabled": false,
                    "ZoneAwarenessEnabled": false,
                },
            }),
        )
        .await;
    assert_eq!(
        scaled.status, 200,
        "case UpdateDomainConfig({domain:?}): expected 200, got {} with {:?}",
        scaled.status, scaled.body
    );

    tokio::time::sleep(SETTLE_WAIT).await;

    // The census is served again on the first read past the update's span, and now
    // publishes the data nodes the scaled-out configuration has.
    let scaled_nodes = census(&emulator, domain).await;
    let scaled_data_nodes: Vec<String> = scaled_nodes
        .iter()
        .filter(|(_, node_type)| node_type == NODE_TYPE_DATA)
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(
        i32::try_from(scaled_data_nodes.len()).unwrap_or(-1),
        MAINTAINABLE_DATA_NODES,
        "case the scaled-out census: expected {MAINTAINABLE_DATA_NODES} data nodes, got \
         {scaled_nodes:?}"
    );

    // The SAME call the single-node domain refused is now accepted, on the configuration
    // the second wait left in place, and the record it mints is readable both ways.
    let accepted = start(
        &emulator,
        domain,
        "RESTART_SEARCH_PROCESS",
        Some(scaled_data_nodes[0].as_str()),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case maintenance on a scaled-out domain: expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    let maintenance_id = string_member(
        &accepted.body,
        "MaintenanceId",
        "maintenance on a scaled-out domain",
    );
    let read = status(&emulator, domain, &maintenance_id).await;
    assert_eq!(
        (
            read.status,
            read.body["Action"].as_str(),
            read.body["NodeId"].as_str(),
        ),
        (
            200,
            Some("RESTART_SEARCH_PROCESS"),
            Some(scaled_data_nodes[0].as_str()),
        ),
        "case GetDomainMaintenanceStatus({maintenance_id}): expected the action the request \
         named, got {} with {:?}",
        read.status,
        read.body
    );
    let listed = list(&emulator, domain, "").await;
    let ids: Vec<String> = listed.body["DomainMaintenances"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case the listing after acceptance: expected a list, got {:?}",
                listed.body
            )
        })
        .iter()
        .map(|details| string_member(details, "MaintenanceId", "the listing after acceptance"))
        .collect();
    assert_eq!(
        ids,
        vec![maintenance_id.clone()],
        "case the listing after acceptance: expected the accepted action to be visible, got \
         {ids:?}"
    );
}
