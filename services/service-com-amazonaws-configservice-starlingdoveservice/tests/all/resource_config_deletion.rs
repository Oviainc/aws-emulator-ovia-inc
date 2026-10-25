// SPDX-License-Identifier: CC0-1.0

//! What recording a custom resource's configuration state and recording its DELETION
//! put into Config's history, end to end over the wire.
//!
//! Two rules are pinned here, each over a TABLE of inputs:
//!
//! 1. every `PutResourceConfig` call records a configuration item, so a resource
//!    recorded again with another configuration gains a NEW item and keeps the states
//!    it has left, while recording the state it is already in leaves those states
//!    alone;
//! 2. `DeleteResourceConfig` records a new configuration item with a `ResourceDeleted`
//!    status for a custom resource Config holds a configuration item for, so the
//!    resource keeps its inventory entry — carrying the instant it went away and the
//!    states it has left — and stops being one of the resources Config reports as
//!    discovered; it refuses a resource type that is not a custom CloudFormation
//!    registry type name with `ValidationException` and any call made while no
//!    configuration recorder is recording with
//!    `NoRunningConfigurationRecorderException`, recording nothing either way; and a
//!    resource Config holds nothing for leaves the inventory exactly as it was, because
//!    this operation models no error for one.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines. What the inventory HOLDS
//! for one resource is read the way a handler reads it (`service_state`), because every
//! configuration-item read of this service takes the modelled `ResourceType` enum,
//! which a CUSTOM resource type is not a member of, so such a read is refused before it
//! reaches a handler; `GetDiscoveredResourceCounts`, whose `resourceTypes` are plain
//! strings, is what reports a custom resource over the wire.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};

use crate::config_connectors::refusal;
use crate::config_discovered_resource_counts::counts_of;
use crate::config_setup_and_rule_loop::{with_recorder_and_channel, Emulator};

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`: "Config automatically assigns the name of
/// \"default\" ... if you do not specify a name at creation time").
const RECORDER: &str = "default";

/// The `ConfigurationItemStatus` of the item that records a resource's deletion
/// (`com.amazonaws.configservice#ConfigurationItemStatus`, whose values are `OK`,
/// `ResourceDiscovered`, `ResourceNotRecorded`, `ResourceDeleted` and
/// `ResourceDeletedNotRecorded`), and the one of the item it replaces.
const RESOURCE_DELETED: &str = "ResourceDeleted";
/// See [`RESOURCE_DELETED`].
const OK: &str = "OK";

/// One row of the deletion table: the case name, the custom resource type, the
/// resource id and the configuration document recorded for it.
type DeletionCase = (&'static str, &'static str, &'static str, &'static str);

/// An emulator with a configuration recorder that is recording, which Config needs
/// before it records anything at all.
async fn recording_emulator() -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    emulator
}

/// The `PutResourceConfig` body recording `configuration` for one resource.
fn resource_config(resource_type: &str, resource_id: &str, configuration: &str) -> Value {
    json!({
        "ResourceType": resource_type,
        "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
        "ResourceId": resource_id,
        "ResourceName": resource_id,
        "Configuration": configuration,
    })
}

/// The `DeleteResourceConfig` body naming one resource.
fn resource_key(resource_type: &str, resource_id: &str) -> Value {
    json!({ "ResourceType": resource_type, "ResourceId": resource_id })
}

/// What the discovered-resource inventory holds for one resource, read the way a
/// handler reaches it.
fn held(
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
) -> Option<DiscoveredResourceEntry> {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let store = state.lock();
    store
        .discovered_resources
        .get(&(resource_type.to_string(), resource_id.to_string()))
        .cloned()
}

/// How many resources the inventory holds altogether, counting the ones a deletion was
/// recorded for — which a discovered-resource read does not report.
fn inventory_size(emulator: &Emulator) -> usize {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let store = state.lock();
    store.discovered_resources.len()
}

/// The `(resourceType, count)` pairs `GetDiscoveredResourceCounts` reports, which is
/// what Config reports as discovered over the wire for a CUSTOM resource type
/// (`GetDiscoveredResourceCountsRequest$resourceTypes` is a list of plain strings,
/// while every configuration-item read takes the modelled `ResourceType` enum). The
/// response is read through the one helper the counts suite already defines.
async fn counts(case: &str, emulator: &Emulator) -> Vec<(String, i64)> {
    let body = emulator.ok("GetDiscoveredResourceCounts", &json!({})).await;
    counts_of(case, &body)
}

#[tokio::test]
async fn recording_a_resource_again_records_a_new_configuration_item_per_state() {
    // The deletion item is one item in a series, so the same rule decides the rest of
    // it: every call records a configuration item, and the states the resource has
    // LEFT are kept beside the current one. Rows differ along the sequence of
    // configurations recorded for one resource — how many, and whether consecutive
    // calls record the same state or another one — and state the configurations the
    // resource must be left holding, oldest first.
    let cases: [(&str, &[&str], &[&str]); 5] = [
        ("one state", &[r#"{"ports":1}"#], &[]),
        (
            "two states",
            &[r#"{"ports":1}"#, r#"{"ports":2}"#],
            &[r#"{"ports":1}"#],
        ),
        (
            "four states",
            &[
                r#"{"ports":1}"#,
                r#"{"ports":2}"#,
                r#"{"ports":3}"#,
                r#"{"ports":4}"#,
            ],
            &[r#"{"ports":1}"#, r#"{"ports":2}"#, r#"{"ports":3}"#],
        ),
        (
            "the same state recorded three times",
            &[r#"{"ports":1}"#, r#"{"ports":1}"#, r#"{"ports":1}"#],
            &[],
        ),
        (
            "a state recorded again after another one",
            &[r#"{"ports":1}"#, r#"{"ports":2}"#, r#"{"ports":1}"#],
            &[r#"{"ports":1}"#, r#"{"ports":2}"#],
        ),
    ];

    let resource_type = "Acme::Network::Switch";
    for (case, configurations, left) in cases {
        let emulator = recording_emulator().await;
        for configuration in configurations {
            emulator
                .ok(
                    "PutResourceConfig",
                    &resource_config(resource_type, "switch-1", configuration),
                )
                .await;
        }

        let entry = held(&emulator, resource_type, "switch-1")
            .unwrap_or_else(|| panic!("case {case:?}: the recorded resource has an entry"));
        let current = configurations
            .last()
            .expect("every row records at least one configuration");
        assert_eq!(
            entry.configuration.as_deref(),
            Some(*current),
            "case {case:?}: the current item holds the configuration recorded last, got {:?}",
            entry.configuration
        );
        // The states the resource left, oldest first, each holding the configuration
        // recorded for it.
        let kept: Vec<Option<&str>> = entry
            .history
            .iter()
            .map(|state| state.configuration.as_deref())
            .collect();
        let expected: Vec<Option<&str>> = left.iter().map(|state| Some(*state)).collect();
        assert_eq!(
            kept, expected,
            "case {case:?}: expected the states {expected:?} left behind, got {kept:?}"
        );
        // One resource, however many items it has recorded.
        let reported = counts(case, &emulator).await;
        assert_eq!(
            reported,
            vec![(resource_type.to_string(), 1)],
            "case {case:?}: the states of one resource are one discovered resource, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_recorded_resource_records_a_resource_deleted_configuration_item() {
    // Rows differ along what the rule generalises over: the custom resource type, the
    // resource id and the configuration document the resource was recorded with (an
    // empty object, a nested document, a list, a non-ASCII value, a document whose
    // only member is null).
    let cases: [DeletionCase; 5] = [
        (
            "a switch recorded with no members at all",
            "Acme::Network::Switch",
            "switch-1",
            "{}",
        ),
        (
            "a vault recorded with a nested document",
            "Beta::Storage::Vault",
            "vault-01",
            r#"{"capacity":{"bytes":1024,"tier":"cold"}}"#,
        ),
        (
            "a node recorded with a list",
            "Contoso::Compute::Node",
            "node/7",
            r#"{"cores":[0,1,2,3]}"#,
        ),
        (
            "a topic recorded with a non-ASCII value",
            "Delta::Queue::Topic",
            "topic-é-1",
            r#"{"name":"füße"}"#,
        ),
        (
            "a stream recorded with a null member",
            "Zeta::Analytics::Stream",
            "stream-1",
            r#"{"window":null}"#,
        ),
    ];

    for (case, resource_type, resource_id, configuration) in cases {
        let emulator = recording_emulator().await;
        emulator
            .ok(
                "PutResourceConfig",
                &resource_config(resource_type, resource_id, configuration),
            )
            .await;
        let before = counts(case, &emulator).await;
        assert_eq!(
            before,
            vec![(resource_type.to_string(), 1)],
            "case {case:?}: the recorded resource must be one discovered resource of its \
             type, got {before:?}"
        );

        let deleted = emulator
            .ok("DeleteResourceConfig", &resource_key(resource_type, resource_id))
            .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case:?}: DeleteResourceConfig returns an empty body, got {deleted}"
        );

        // The resource is no longer one Config reports as discovered.
        let after = counts(case, &emulator).await;
        assert!(
            after.is_empty(),
            "case {case:?}: a resource whose deletion is recorded is no longer discovered, \
             got {after:?}"
        );

        // Its entry survives its own deletion, carrying the instant it went away, and
        // the item that records the deletion is a NEW configuration item: the state the
        // resource left is kept beside it, holding the configuration it was recorded
        // with.
        let entry = held(&emulator, resource_type, resource_id)
            .unwrap_or_else(|| panic!("case {case:?}: the deleted resource keeps its entry"));
        assert!(
            entry.deleted_at.is_some(),
            "case {case:?}: the entry must carry the instant the resource went away, got \
             {entry:?}"
        );
        assert_eq!(
            entry.configuration_item_status(),
            RESOURCE_DELETED,
            "case {case:?}: expected the current configuration item to be {RESOURCE_DELETED}, \
             got {:?}",
            entry.configuration_item_status()
        );
        assert_eq!(
            entry.configuration.as_deref(),
            Some(configuration),
            "case {case:?}: the recorded configuration document must be readable back byte \
             for byte, got {:?}",
            entry.configuration
        );
        assert_eq!(
            entry.history.len(),
            1,
            "case {case:?}: recording the deletion keeps the one state the resource left, \
             got {:?}",
            entry.history
        );
        assert_eq!(
            entry.history[0].configuration.as_deref(),
            Some(configuration),
            "case {case:?}: the state the resource left holds the configuration it was \
             recorded with, got {:?}",
            entry.history[0].configuration
        );
    }
}

#[tokio::test]
async fn recording_the_same_deletion_twice_records_one_deletion() {
    let case = "the same resource deleted three times";
    let resource_type = "Acme::Network::Switch";
    let emulator = recording_emulator().await;
    emulator
        .ok(
            "PutResourceConfig",
            &resource_config(resource_type, "switch-1", r#"{"ports":4}"#),
        )
        .await;

    emulator
        .ok("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
        .await;
    let first = held(&emulator, resource_type, "switch-1")
        .unwrap_or_else(|| panic!("case {case:?}: the deleted resource keeps its entry"));

    for repeat in 1..3 {
        emulator
            .ok("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
            .await;
        let again = held(&emulator, resource_type, "switch-1")
            .unwrap_or_else(|| panic!("case {case:?}: the deleted resource keeps its entry"));
        assert_eq!(
            (again.deleted_at, again.history.len()),
            (first.deleted_at, first.history.len()),
            "case {case:?}: repeat {repeat} must not record another state, got {again:?}"
        );
        let reported = counts(case, &emulator).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: repeat {repeat} leaves the resource undiscovered, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_resource_config_holds_nothing_for_changes_nothing() {
    // Rows differ along how the named resource misses the one the inventory holds: a
    // resource id nothing was recorded for, a custom type nothing was recorded for,
    // and an id differing from the recorded one only in case.
    let cases: [(&str, &str, &str); 3] = [
        (
            "a resource id nothing was recorded for",
            "Acme::Network::Switch",
            "switch-9",
        ),
        (
            "a custom resource type nothing was recorded for",
            "Beta::Storage::Vault",
            "switch-1",
        ),
        (
            "an id differing from the recorded one only in case",
            "Acme::Network::Switch",
            "SWITCH-1",
        ),
    ];

    for (case, resource_type, resource_id) in cases {
        let emulator = recording_emulator().await;
        emulator
            .ok(
                "PutResourceConfig",
                &resource_config("Acme::Network::Switch", "switch-1", r#"{"ports":4}"#),
            )
            .await;

        emulator
            .ok("DeleteResourceConfig", &resource_key(resource_type, resource_id))
            .await;

        let reported = counts(case, &emulator).await;
        assert_eq!(
            reported,
            vec![("Acme::Network::Switch".to_string(), 1)],
            "case {case:?}: the recorded resource is untouched, got {reported:?}"
        );
        assert_eq!(
            inventory_size(&emulator),
            1,
            "case {case:?}: a resource Config holds nothing for must add no entry"
        );
        let entry = held(&emulator, "Acme::Network::Switch", "switch-1")
            .unwrap_or_else(|| panic!("case {case:?}: the recorded resource keeps its entry"));
        assert_eq!(
            entry.configuration_item_status(),
            OK,
            "case {case:?}: the recorded resource is still recorded, got {:?}",
            entry.configuration_item_status()
        );
    }
}

#[tokio::test]
async fn delete_resource_config_records_only_a_custom_resource_type() {
    // Rows differ along why the resource type names no custom resource: an organization
    // the registry reserves (in the spellings the reservation covers, since an
    // organization name is matched case-insensitively) and a type name that is not a
    // CloudFormation registry type name at all.
    let cases: [(&str, &str); 8] = [
        ("an AWS native resource type", "AWS::S3::Bucket"),
        ("the reserved organization Amazon", "Amazon::Thing::Widget"),
        ("the reserved organization AMZN", "AMZN::Thing::Widget"),
        ("the reserved organization alexa", "alexa::Smart::Home"),
        ("the reserved organization Custom", "Custom::Thing::Widget"),
        ("a type name of one segment", "Switch"),
        ("a type name of two segments", "Acme::Switch"),
        ("a registry module name", "Acme::Network::Switch::MODULE"),
    ];

    for (case, resource_type) in cases {
        let emulator = recording_emulator().await;
        emulator
            .ok(
                "PutResourceConfig",
                &resource_config("Acme::Network::Switch", "switch-1", r#"{"ports":4}"#),
            )
            .await;

        let (status, code) = emulator
            .refused("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: deleting {resource_type:?} must be refused with a 400 \
             ValidationException, got {status} with {code}"
        );
        let reported = counts(case, &emulator).await;
        assert_eq!(
            reported,
            vec![("Acme::Network::Switch".to_string(), 1)],
            "case {case:?}: a refused DeleteResourceConfig records nothing, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn delete_resource_config_needs_a_recording_configuration_recorder() {
    let resource_type = "Acme::Network::Switch";

    // No configuration recorder in the account at all.
    let bare = Emulator::new();
    let (status, code) = bare
        .refused("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoRunningConfigurationRecorderException"),
        "with no configuration recorder at all, recording a deletion must be refused, got \
         {status} with {code}"
    );

    // A recorder that exists but has never been started.
    let idle = Emulator::new();
    with_recorder_and_channel(&idle).await;
    let (status, code) = idle
        .refused("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoRunningConfigurationRecorderException"),
        "with a recorder that has never been started, recording a deletion must be refused, \
         got {status} with {code}"
    );

    // A recorder that recorded the resource and was then turned off: the deletion is
    // refused and the resource stays recorded, which a restarted recorder reports again.
    let stopped = recording_emulator().await;
    stopped
        .ok(
            "PutResourceConfig",
            &resource_config(resource_type, "switch-1", r#"{"ports":4}"#),
        )
        .await;
    stopped
        .ok(
            "StopConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    let (status, code) = stopped
        .refused("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoRunningConfigurationRecorderException"),
        "with the recorder turned off, recording a deletion must be refused, got {status} \
         with {code}"
    );
    let entry = held(&stopped, resource_type, "switch-1")
        .expect("the recorded resource keeps its entry");
    assert!(
        entry.deleted_at.is_none(),
        "a refused DeleteResourceConfig must record no deletion, got {entry:?}"
    );
    stopped
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    let reported = counts("the recorder restarted", &stopped).await;
    assert_eq!(
        reported,
        vec![(resource_type.to_string(), 1)],
        "a restarted recorder reports the resource again, got {reported:?}"
    );

    // The sentence Config answers this precondition with states the precondition and
    // names the API the caller reached for: that name is its only variable part, so the
    // two operations that record a resource's configuration state report the one
    // condition the one way.
    let bare = Emulator::new();
    for (operation, request) in [
        (
            "PutResourceConfig",
            resource_config(resource_type, "switch-1", "{}"),
        ),
        (
            "DeleteResourceConfig",
            resource_key(resource_type, "switch-1"),
        ),
    ] {
        let (status, code, message) = refusal(&bare, operation, &request).await;
        let expected = format!(
            "You must create and start a configuration recorder before you call the \
             {operation} API."
        );
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "NoRunningConfigurationRecorderException", expected.as_str()),
            "{operation}: expected the recorder precondition to be reported as \
             {expected:?}, got {status} with {code} and {message:?}"
        );
    }
}

#[tokio::test]
async fn recording_the_configuration_of_a_deleted_resource_discovers_it_again() {
    let case = "a resource recorded, deleted and recorded again";
    let resource_type = "Acme::Network::Switch";
    let emulator = recording_emulator().await;
    emulator
        .ok(
            "PutResourceConfig",
            &resource_config(resource_type, "switch-1", r#"{"ports":4}"#),
        )
        .await;
    emulator
        .ok("DeleteResourceConfig", &resource_key(resource_type, "switch-1"))
        .await;
    emulator
        .ok(
            "PutResourceConfig",
            &resource_config(resource_type, "switch-1", r#"{"ports":8}"#),
        )
        .await;

    let reported = counts(case, &emulator).await;
    assert_eq!(
        reported,
        vec![(resource_type.to_string(), 1)],
        "case {case:?}: recording the configuration of a deleted resource discovers it \
         again, got {reported:?}"
    );
    let entry = held(&emulator, resource_type, "switch-1")
        .unwrap_or_else(|| panic!("case {case:?}: the resource keeps its entry"));
    assert_eq!(
        (entry.deleted_at.is_none(), entry.configuration.as_deref()),
        (true, Some(r#"{"ports":8}"#)),
        "case {case:?}: the resource is recorded again with the new configuration, got \
         {entry:?}"
    );
    assert_eq!(
        entry.configuration_item_status(),
        OK,
        "case {case:?}: expected the current configuration item to be {OK}, got {:?}",
        entry.configuration_item_status()
    );
    // Three states recorded, three items: the two the resource has left (the first
    // configuration, and its deletion) and the current one.
    assert_eq!(
        entry.history.len(),
        2,
        "case {case:?}: every state the resource left is kept, got {:?}",
        entry.history
    );
    assert!(
        entry.first_recorded_at.is_some(),
        "case {case:?}: the instant Config first recorded the resource is kept, got {entry:?}"
    );
}
