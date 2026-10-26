// SPDX-License-Identifier: CC0-1.0

//! What a batched configuration-item read answers, end to end over the wire.
//!
//! The rule pinned here: `BatchGetResourceConfig` answers each requested resource key —
//! a `(resourceType, resourceId)` pair — with the CURRENT configuration item this
//! account's discovered-resource inventory holds for it in the Region the request is
//! signed for, reports every key it holds no such configuration item for (a resource the
//! inventory does not hold, and a deleted one, since "The API does not return results
//! for deleted resources") in `unprocessedResourceKeys`, and refuses the whole request
//! when the account holds no customer managed configuration recorder
//! (`NoAvailableConfigurationRecorderException`) or when the batch carries fewer than
//! one or more than a hundred keys (`ValidationException`). Every key sent is accounted
//! for exactly once, in the order it was sent.
//!
//! Two further properties of a reported item are pinned with it: it is the SAME
//! configuration item the resource's own history reports as current, member for member
//! over everything a `BaseConfigurationItem` carries — so a resource recorded twice is
//! answered with its latest state — and it never carries the resource's `tags`, which
//! this API does not return.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines. The recorded resources are
//! seeded through the store's own write path (`record_discovered_resource`, which the
//! recording pass uses) the way this crate's other inventory suites seed one, because a
//! resource key's `resourceType` is the modelled `ResourceType` enum of the types an
//! Amazon Web Services service owns, which a recording pass discovers, while
//! `PutResourceConfig` records CUSTOM resource types only.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry, SERVICE_LINKED_PRINCIPALS,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation under test.
const BATCH: &str = "BatchGetResourceConfig";

/// Resource types the rows use. Entries of `com.amazonaws.configservice#ResourceType`,
/// from `smithy_query(query="shape", shape="ResourceType")`, deliberately including
/// types no recording in the issue used.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";
/// See [`EC2_VOLUME`].
const IAM_ROLE: &str = "AWS::IAM::Role";

/// The Region the harness signs its requests for, and therefore the Region a reported
/// configuration item belongs to.
const HOME_REGION: &str = "us-east-1";
/// Another Region the same store is read from.
const OTHER_REGION: &str = "eu-west-1";

/// The fewest and the most resource keys one batch may carry (`ResourceKeys`,
/// `@length(min: 1, max: 100)`, from `smithy_query(query="shape", shape="ResourceKeys")`).
const MIN_RESOURCE_KEYS: usize = 1;
/// See [`MIN_RESOURCE_KEYS`].
const MAX_RESOURCE_KEYS: usize = 100;

/// Every member a `BaseConfigurationItem` carries, from `smithy_query(query="shape",
/// shape="BaseConfigurationItem")`: what a batch read and the resource's own history
/// must agree on for the resource's current state.
const BASE_MEMBERS: &[&str] = &[
    "version",
    "accountId",
    "configurationItemCaptureTime",
    "configurationItemStatus",
    "configurationStateId",
    "arn",
    "resourceType",
    "resourceId",
    "resourceName",
    "awsRegion",
    "availabilityZone",
    "resourceCreationTime",
    "configuration",
    "supplementaryConfiguration",
    "recordingFrequency",
    "configurationItemDeliveryTime",
];

/// The instant the first state of a seeded resource was captured: a fixed point in the
/// past, so a later state of the same resource is captured after it.
fn captured_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// A `(resourceType, resourceId)` pair, as a row names a resource and as the response
/// readers below report one.
type Pair = (&'static str, &'static str);

/// One row of the lookup table: the case name, the keys the batch carries, the
/// configuration items expected for them and the keys expected back unprocessed.
type LookupCase = (&'static str, Vec<Value>, &'static [Pair], &'static [Pair]);

/// One resource the inventory holds: its type, its id, the custom name it carries if
/// any, the configuration document recorded for it if any, and whether its deletion was
/// recorded.
struct Recorded {
    resource_type: &'static str,
    resource_id: &'static str,
    resource_name: Option<&'static str>,
    configuration: Option<&'static str>,
    deleted: bool,
}

/// A resource whose configuration state has been recorded.
fn recorded(resource_type: &'static str, resource_id: &'static str) -> Recorded {
    Recorded {
        resource_type,
        resource_id,
        resource_name: Some(resource_id),
        configuration: Some(r#"{"state":"available"}"#),
        deleted: false,
    }
}

/// A resource Config discovered but recorded no configuration item for.
fn discovered_only(resource_type: &'static str, resource_id: &'static str) -> Recorded {
    Recorded {
        resource_name: None,
        configuration: None,
        ..recorded(resource_type, resource_id)
    }
}

/// A resource whose deletion the inventory holds.
fn deleted(resource_type: &'static str, resource_id: &'static str) -> Recorded {
    Recorded {
        deleted: true,
        ..recorded(resource_type, resource_id)
    }
}

/// The inventory every table seeds unless it states its own.
fn inventory() -> Vec<Recorded> {
    vec![
        recorded(EC2_VOLUME, "vol-0001"),
        recorded(DYNAMODB_TABLE, "orders"),
        recorded(S3_BUCKET, "config-bucket"),
        discovered_only(LAMBDA_FUNCTION, "evaluate"),
        deleted(EC2_VOLUME, "vol-0009"),
    ]
}

/// Put `resources` into the store through the write path the recording pass uses.
fn seed(emulator: &Emulator, resources: &[Recorded]) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    for resource in resources {
        store.record_discovered_resource(DiscoveredResourceEntry {
            resource_type: resource.resource_type.to_string(),
            resource_id: resource.resource_id.to_string(),
            resource_name: resource.resource_name.map(str::to_string),
            deleted_at: resource.deleted.then(captured_at),
            schema_version_id: resource
                .configuration
                .map(|_| "00000000-0000-0000-0000-000000000001".to_string()),
            configuration: resource.configuration.map(str::to_string),
            tags: [("owner".to_string(), "platform".to_string())]
                .into_iter()
                .collect(),
            first_recorded_at: resource.configuration.map(|_| captured_at()),
            last_recorded_at: resource.configuration.map(|_| captured_at()),
            ..DiscoveredResourceEntry::default()
        });
    }
}

/// Record a LATER state of one resource, the way a further recording of it would: the
/// store's write path keeps the state it is leaving beside the new one.
fn record_again(
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
    configuration: &str,
    captured: SystemTime,
) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    let held = store
        .discovered_resources
        .get(&(resource_type.to_string(), resource_id.to_string()))
        .cloned()
        .expect("the resource was seeded");
    store.record_discovered_resource(DiscoveredResourceEntry {
        configuration: Some(configuration.to_string()),
        last_recorded_at: Some(captured),
        ..held
    });
}

/// An emulator with a customer managed configuration recorder — which a
/// configuration-item read needs — holding `resources` in its inventory.
async fn emulator_holding(resources: &[Recorded]) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    seed(&emulator, resources);
    emulator
}

/// One resource key.
fn key(resource_type: &str, resource_id: &str) -> Value {
    json!({ "resourceType": resource_type, "resourceId": resource_id })
}

/// The `BatchGetResourceConfig` body for `keys`.
fn batch(keys: &[Value]) -> Value {
    json!({ "resourceKeys": keys })
}

/// The `(resourceType, resourceId)` of every configuration item in a response, in the
/// order reported.
fn reported_items(case: &str, body: &Value) -> Vec<(String, String)> {
    body["baseConfigurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected baseConfigurationItems, got {body}"))
        .iter()
        .map(|item| {
            (
                item["resourceType"].as_str().unwrap_or_default().to_string(),
                item["resourceId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `(resourceType, resourceId)` of every unprocessed key in a response, in the order
/// reported.
fn reported_unprocessed(case: &str, body: &Value) -> Vec<(String, String)> {
    body["unprocessedResourceKeys"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected unprocessedResourceKeys, got {body}"))
        .iter()
        .map(|sent| {
            (
                sent["resourceType"].as_str().unwrap_or_default().to_string(),
                sent["resourceId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `expected` as the pairs [`reported_items`] and [`reported_unprocessed`] return.
fn pairs(expected: &[Pair]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(resource_type, resource_id)| (resource_type.to_string(), resource_id.to_string()))
        .collect()
}

/// The one configuration item a batch read answers for one resource.
async fn one_item(case: &str, emulator: &Emulator, resource_type: &str, resource_id: &str) -> Value {
    let body = emulator
        .ok(BATCH, &batch(&[key(resource_type, resource_id)]))
        .await;
    let items = body["baseConfigurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected baseConfigurationItems, got {body}"))
        .clone();
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: one key the inventory holds is one configuration item, got {body}"
    );
    items[0].clone()
}

/// Every configuration item `GetResourceConfigHistory` reports for one resource, newest
/// first — the order that read answers by default.
async fn history(case: &str, emulator: &Emulator, resource_type: &str, resource_id: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "GetResourceConfigHistory",
            &json!({ "resourceType": resource_type, "resourceId": resource_id }),
        )
        .await;
    body["configurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected configurationItems, got {body}"))
        .clone()
}

/// How many resources the inventory holds, read the way a handler reaches it: a report
/// must leave this alone.
fn inventory_size(emulator: &Emulator) -> usize {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let store = state.lock();
    store.discovered_resources.len()
}

#[tokio::test]
async fn a_batch_answers_the_keys_the_inventory_holds_a_configuration_item_for() {
    // Rows differ along what the rule generalises over: how many keys the batch
    // carries, which resource types and ids they name, whether the resource is
    // recorded, discovered without a configuration, deleted or absent, and the order
    // the keys are sent in.
    let cases: [LookupCase; 9] = [
        (
            "one recorded resource",
            vec![key(EC2_VOLUME, "vol-0001")],
            &[(EC2_VOLUME, "vol-0001")],
            &[],
        ),
        (
            "several recorded resources of different types",
            vec![
                key(EC2_VOLUME, "vol-0001"),
                key(DYNAMODB_TABLE, "orders"),
                key(S3_BUCKET, "config-bucket"),
            ],
            &[
                (EC2_VOLUME, "vol-0001"),
                (DYNAMODB_TABLE, "orders"),
                (S3_BUCKET, "config-bucket"),
            ],
            &[],
        ),
        (
            "the same resources in another order",
            vec![
                key(S3_BUCKET, "config-bucket"),
                key(EC2_VOLUME, "vol-0001"),
                key(DYNAMODB_TABLE, "orders"),
            ],
            &[
                (S3_BUCKET, "config-bucket"),
                (EC2_VOLUME, "vol-0001"),
                (DYNAMODB_TABLE, "orders"),
            ],
            &[],
        ),
        (
            "a resource of a type nothing was recorded for",
            vec![key(IAM_ROLE, "config-role")],
            &[],
            &[(IAM_ROLE, "config-role")],
        ),
        (
            "a resource id the inventory does not hold",
            vec![key(EC2_VOLUME, "vol-0404")],
            &[],
            &[(EC2_VOLUME, "vol-0404")],
        ),
        (
            "an id recorded under another type",
            vec![key(DYNAMODB_TABLE, "vol-0001")],
            &[],
            &[(DYNAMODB_TABLE, "vol-0001")],
        ),
        (
            "a deleted resource, which the API does not return",
            vec![key(EC2_VOLUME, "vol-0009")],
            &[],
            &[(EC2_VOLUME, "vol-0009")],
        ),
        (
            "a resource discovered with no configuration recorded",
            vec![key(LAMBDA_FUNCTION, "evaluate")],
            &[(LAMBDA_FUNCTION, "evaluate")],
            &[],
        ),
        (
            "a batch mixing resources the inventory holds and ones it does not",
            vec![
                key(EC2_VOLUME, "vol-0404"),
                key(DYNAMODB_TABLE, "orders"),
                key(EC2_VOLUME, "vol-0009"),
                key(EC2_VOLUME, "vol-0001"),
            ],
            &[(DYNAMODB_TABLE, "orders"), (EC2_VOLUME, "vol-0001")],
            &[(EC2_VOLUME, "vol-0404"), (EC2_VOLUME, "vol-0009")],
        ),
    ];

    for (case, keys, expected_items, expected_unprocessed) in cases {
        let emulator = emulator_holding(&inventory()).await;
        let held = inventory_size(&emulator);
        let request = batch(&keys);

        let body = emulator.ok(BATCH, &request).await;
        assert_eq!(
            reported_items(case, &body),
            pairs(expected_items),
            "case {case:?}: expected the configuration items {expected_items:?}, got {body}"
        );
        assert_eq!(
            reported_unprocessed(case, &body),
            pairs(expected_unprocessed),
            "case {case:?}: expected the unprocessed keys {expected_unprocessed:?}, got {body}"
        );
        // Every key sent is accounted for exactly once: answered with an item or
        // reported unprocessed.
        assert_eq!(
            reported_items(case, &body).len() + reported_unprocessed(case, &body).len(),
            keys.len(),
            "case {case:?}: every one of the {} keys sent is accounted for once, got {body}",
            keys.len()
        );

        // The read is a report: it writes nothing, so the inventory a later caller sees
        // is the one this call saw.
        let again = emulator.ok(BATCH, &request).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second batch read must report the same items, got {again}"
        );
        assert_eq!(
            inventory_size(&emulator),
            held,
            "case {case:?}: a report must leave the inventory at {held} resources"
        );
    }
}

#[tokio::test]
async fn an_answered_item_is_the_current_item_of_the_resources_own_history() {
    // Rows differ along the state the inventory holds for the resource: recorded with a
    // name and a configuration, discovered without one, and recorded more than once —
    // where the current item is the LATEST state.
    let cases: [(&str, &str, &str, usize); 4] = [
        ("one recorded state", EC2_VOLUME, "vol-0001", 1),
        ("another recorded type", S3_BUCKET, "config-bucket", 1),
        (
            "discovered with no configuration recorded",
            LAMBDA_FUNCTION,
            "evaluate",
            1,
        ),
        ("two recorded states", DYNAMODB_TABLE, "orders", 2),
    ];

    for (case, resource_type, resource_id, states) in cases {
        let emulator = emulator_holding(&inventory()).await;
        if states > 1 {
            record_again(
                &emulator,
                resource_type,
                resource_id,
                r#"{"state":"in-use"}"#,
                captured_at() + Duration::from_secs(3_600),
            );
        }

        let item = one_item(case, &emulator, resource_type, resource_id).await;
        let items = history(case, &emulator, resource_type, resource_id).await;
        assert_eq!(
            items.len(),
            states,
            "case {case:?}: the resource went through {states} recorded states, got {items:?}"
        );
        let current = &items[0];

        for member in BASE_MEMBERS {
            assert_eq!(
                item.get(member),
                current.get(member),
                "case {case:?}: the batch read must answer {member:?} with what the \
                 resource's current configuration item carries, got {item} against {current}"
            );
        }
        // "The API does not return any tags for the requested resources."
        assert!(
            item.get("tags").is_none(),
            "case {case:?}: a batch read reports no tags, got {item}"
        );
        // The item reports what the INVENTORY holds for the resource, never the key
        // sent: type, id and the state the resource is in now.
        assert_eq!(
            (
                item["resourceType"].as_str(),
                item["resourceId"].as_str(),
                item["accountId"].as_str(),
            ),
            (
                Some(resource_type),
                Some(resource_id),
                Some(emulator_server::emulator_account_id()),
            ),
            "case {case:?}: expected the recorded resource in this account, got {item}"
        );
    }
}

#[tokio::test]
async fn an_item_belongs_to_the_region_the_request_is_signed_for() {
    // One store, read from two Regions: this account's Config data belongs to the
    // Region the request is signed for, so that is the Region the item is reported
    // under.
    let emulator = emulator_holding(&inventory()).await;
    let elsewhere = emulator.in_other_region(OTHER_REGION);

    for (case, reader, region) in [
        ("the Region the data was recorded in", &emulator, HOME_REGION),
        ("another Region of the same account", &elsewhere, OTHER_REGION),
    ] {
        let item = one_item(case, reader, EC2_VOLUME, "vol-0001").await;
        assert_eq!(
            item["awsRegion"].as_str(),
            Some(region),
            "case {case:?}: expected the item to be reported under {region}, got {item}"
        );
        let arn = item["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {item}"));
        assert!(
            arn.contains(region),
            "case {case:?}: expected a regional ARN naming {region}, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_outside_the_key_count_the_operation_accepts_is_refused() {
    let emulator = emulator_holding(&inventory()).await;

    let none: Vec<Value> = Vec::new();
    let at_minimum: Vec<Value> = vec![key(EC2_VOLUME, "vol-0001")];
    let at_maximum: Vec<Value> = (0..MAX_RESOURCE_KEYS)
        .map(|index| key(EC2_VOLUME, &format!("vol-{index:04}")))
        .collect();
    let one_over: Vec<Value> = (0..=MAX_RESOURCE_KEYS)
        .map(|index| key(EC2_VOLUME, &format!("vol-{index:04}")))
        .collect();

    // One past each published bound is refused: a batch carrying nothing, and one
    // carrying a hundred and one keys.
    for (case, keys) in [
        ("no resource keys at all", &none),
        ("one key past the maximum", &one_over),
    ] {
        let (status, code) = emulator.refused(BATCH, &batch(keys)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a batch of {} keys is outside {MIN_RESOURCE_KEYS}..\
             {MAX_RESOURCE_KEYS} and must be refused 400 ValidationException, got \
             {status} {code}",
            keys.len()
        );
    }

    // At each bound the same batch is served, and accounts for every key it carries.
    for (case, keys) in [
        ("one key", &at_minimum),
        ("keys at the maximum", &at_maximum),
    ] {
        let body = emulator.ok(BATCH, &batch(keys)).await;
        assert_eq!(
            reported_items(case, &body).len() + reported_unprocessed(case, &body).len(),
            keys.len(),
            "case {case:?}: a batch at the bound accounts for all {} keys, got {body}",
            keys.len()
        );
    }
}

#[tokio::test]
async fn a_batch_read_needs_a_customer_managed_configuration_recorder() {
    // Rows differ along the batch sent, so the refusal is about the account holding no
    // customer managed recorder rather than about what the batch asked for.
    let requests: [(&str, Vec<Value>); 4] = [
        ("one recorded resource", vec![key(EC2_VOLUME, "vol-0001")]),
        (
            "several resources",
            vec![key(EC2_VOLUME, "vol-0001"), key(DYNAMODB_TABLE, "orders")],
        ),
        (
            "a resource the inventory does not hold",
            vec![key(IAM_ROLE, "config-role")],
        ),
        ("a deleted resource", vec![key(EC2_VOLUME, "vol-0009")]),
    ];

    for (case, keys) in requests {
        let emulator = Emulator::new();
        seed(&emulator, &inventory());

        let (status, code) = emulator.refused(BATCH, &batch(&keys)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoAvailableConfigurationRecorderException"),
            "case {case:?}: with no configuration recorder the read is refused 400 \
             NoAvailableConfigurationRecorderException, got {status} {code}"
        );

        // A SERVICE-LINKED recorder records for the service it is linked to, so it does
        // not stand in for a customer managed one.
        emulator
            .ok(
                "PutServiceLinkedConfigurationRecorder",
                &json!({ "ServicePrincipal": SERVICE_LINKED_PRINCIPALS[0] }),
            )
            .await;
        let (status, code) = emulator.refused(BATCH, &batch(&keys)).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoAvailableConfigurationRecorderException"),
            "case {case:?}: a service-linked recorder is no customer managed recorder, so \
             the read is still refused 400 NoAvailableConfigurationRecorderException, got \
             {status} {code}"
        );

        // With a customer managed recorder the same batch is served.
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({ "roleARN": ROLE_ARN })),
            )
            .await;
        let body = emulator.ok(BATCH, &batch(&keys)).await;
        assert_eq!(
            reported_items(case, &body).len() + reported_unprocessed(case, &body).len(),
            keys.len(),
            "case {case:?}: with a recorder every key sent is accounted for, got {body}"
        );
    }
}
