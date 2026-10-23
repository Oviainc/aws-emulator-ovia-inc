// SPDX-License-Identifier: CC0-1.0

//! What an aggregate configuration-item read reports, end to end over the wire.
//!
//! The rule pinned here: `BatchGetAggregateResourceConfig` answers each requested
//! aggregate resource identifier with the CURRENT configuration item the named
//! configuration aggregator holds for it — the resource recorded in a source account
//! and Region that aggregator aggregates, of a resource type its aggregator filters
//! admit, which has not been deleted — and reports every identifier it holds no such
//! configuration item for in `UnprocessedResourceIdentifiers`, refusing the whole
//! request when the aggregator does not exist (`NoSuchConfigurationAggregatorException`)
//! or when the batch carries fewer than one or more than a hundred identifiers
//! (`ValidationException`).
//!
//! Two further properties of a reported item are pinned with it: an item carries the
//! configuration state the inventory holds — the account, Region, type, id, name,
//! schema version, configuration document, capture time and the state id that orders
//! the resource's items — and never the resource's tags, which this API does not
//! return; and it reports a `recordingFrequency` and a `configurationItemDeliveryTime`
//! only where the recording recorder records that resource type daily.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` already defines, and creates its
//! aggregators through `PutConfigurationAggregator` over that wire. The recorded
//! resources themselves are seeded in the store the way a handler reaches it
//! (`service_state`, as `discovered_resources` seeds the reader it pins): the
//! aggregate identifier's `ResourceType` is the modelled `ResourceType` enum, while
//! `PutResourceConfig` records CUSTOM resource types (`ResourceTypeString`), so a
//! configuration item of an Amazon Web Services resource type cannot be put over the
//! wire by any implemented operation.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

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

/// The Region the harness signs its requests for, which is therefore the Region this
/// store's Config data belongs to.
const HOME_REGION: &str = "us-east-1";
/// A Region this store holds no Config data for.
const OTHER_REGION: &str = "us-west-2";

/// A source account that is not this emulator's, in the twelve-digit form
/// `AccountId`'s `^\d{12}$` pattern allows.
const OTHER_ACCOUNT: &str = "111122223333";

/// The only published `AggregatorFilterType` value, from
/// `smithy_query(query="shape", shape="AggregatorFilterType")`: `INCLUDE`.
const FILTER_TYPE_INCLUDE: &str = "INCLUDE";

/// Published `RecordingFrequency` values, from `smithy_query(query="shape",
/// shape="RecordingFrequency")`: `CONTINUOUS` and `DAILY`.
const DAILY: &str = "DAILY";

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`).
const RECORDER: &str = "default";

/// The aggregator every case that is not about the aggregator's own scope uses.
const AGGREGATOR: &str = "org-wide";

/// The most aggregate resource identifiers one batch may carry, and the fewest
/// (`ResourceIdentifiersList`, `@length(min: 1, max: 100)`).
const MIN_IDENTIFIERS: usize = 1;
/// See [`MIN_IDENTIFIERS`].
const MAX_IDENTIFIERS: usize = 100;

/// The instant a seeded resource's configuration item was captured: a fixed point in
/// the past, so the reported capture time and configuration state id can be checked
/// against it.
fn captured_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// This emulator's account id, the account whose Config data the store holds.
fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// A `(resourceType, resourceId)` pair, as a row names a resource and as the
/// response readers below report one.
type Pair = (&'static str, &'static str);

/// One row of the lookup table: the case name, the identifiers the batch carries, the
/// configuration items expected for them and the identifiers expected back
/// unprocessed.
type LookupCase = (&'static str, Vec<Value>, &'static [Pair], &'static [Pair]);

/// One row of the scope table: the case name, the `PutConfigurationAggregator` body
/// that gives the aggregator its scope, and the two lists expected under it.
type ScopeCase = (&'static str, Value, &'static [Pair], &'static [Pair]);

/// One row of the reported-members table: the case name, the resource type and id to
/// read, and the resource name and configuration document it was recorded with.
type ItemCase = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
);

/// One row of the recording-mode table: the case name, the configuration recorder to
/// create, and per resource type whether its configuration item reports daily
/// recording.
type RecordingModeCase = (&'static str, Value, &'static [(&'static str, bool)]);

/// One resource the store holds: its type, its id, the custom name it carries if any,
/// the configuration document recorded for it if any, and whether it has been deleted.
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

/// An emulator with a configuration recorder, holding `resources` in its
/// discovered-resource inventory.
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

/// Put `resources` into the store the way a handler reaches it.
fn seed(emulator: &Emulator, resources: &[Recorded]) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    for resource in resources {
        store.discovered_resources.insert(
            (
                resource.resource_type.to_string(),
                resource.resource_id.to_string(),
            ),
            DiscoveredResourceEntry {
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
            },
        );
    }
}

/// An account aggregation source over `accounts`, aggregating the Regions in
/// `regions`, or every Region when `regions` is empty.
fn account_source(accounts: &[&str], regions: &[&str]) -> Value {
    let mut source = json!({ "AccountIds": accounts });
    if regions.is_empty() {
        source["AllAwsRegions"] = json!(true);
    } else {
        source["AwsRegions"] = json!(regions);
    }
    source
}

/// An organization aggregation source over the Regions in `regions`, or every Region
/// when `regions` is empty.
fn organization_source(regions: &[&str]) -> Value {
    let mut source = json!({ "RoleArn": ROLE_ARN });
    if regions.is_empty() {
        source["AllAwsRegions"] = json!(true);
    } else {
        source["AwsRegions"] = json!(regions);
    }
    source
}

/// Create the aggregator `name` from the `PutConfigurationAggregator` body `sources`.
async fn put_aggregator(emulator: &Emulator, name: &str, mut sources: Value) {
    sources["ConfigurationAggregatorName"] = json!(name);
    emulator.ok("PutConfigurationAggregator", &sources).await;
}

/// An aggregator named [`AGGREGATOR`] that aggregates this account in every Region.
async fn put_whole_account_aggregator(emulator: &Emulator) {
    put_aggregator(
        emulator,
        AGGREGATOR,
        json!({ "AccountAggregationSources": [account_source(&[&account()], &[])] }),
    )
    .await;
}

/// One aggregate resource identifier.
fn identifier(account_id: &str, region: &str, resource_type: &str, resource_id: &str) -> Value {
    json!({
        "SourceAccountId": account_id,
        "SourceRegion": region,
        "ResourceType": resource_type,
        "ResourceId": resource_id,
    })
}

/// An identifier naming this account's data in the Region the harness signs for.
fn home(resource_type: &str, resource_id: &str) -> Value {
    identifier(&account(), HOME_REGION, resource_type, resource_id)
}

/// The `BatchGetAggregateResourceConfig` body for `identifiers`.
fn batch(aggregator: &str, identifiers: &[Value]) -> Value {
    json!({
        "ConfigurationAggregatorName": aggregator,
        "ResourceIdentifiers": identifiers,
    })
}

/// The `(resourceType, resourceId)` of every configuration item in a response, in the
/// order reported.
fn reported_items(case: &str, body: &Value) -> Vec<(String, String)> {
    body["BaseConfigurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected BaseConfigurationItems, got {body}"))
        .iter()
        .map(|item| {
            (
                item["resourceType"].as_str().unwrap_or_default().to_string(),
                item["resourceId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `(ResourceType, ResourceId)` of every unprocessed identifier in a response, in
/// the order reported.
fn reported_unprocessed(case: &str, body: &Value) -> Vec<(String, String)> {
    body["UnprocessedResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected UnprocessedResourceIdentifiers, got {body}")
        })
        .iter()
        .map(|identifier| {
            (
                identifier["ResourceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                identifier["ResourceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// `expected` as the pairs [`reported_items`] and [`reported_unprocessed`] return.
fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(resource_type, resource_id)| (resource_type.to_string(), resource_id.to_string()))
        .collect()
}

#[tokio::test]
async fn a_batch_answers_the_identifiers_the_aggregator_holds_a_configuration_item_for() {
    // Rows differ along what the rule generalises over: how many identifiers the batch
    // carries, which resource types and ids they name, whether the resource is
    // recorded, discovered without a configuration, deleted or absent, whether the
    // source account and Region are the ones this store holds data for, and whether
    // the identifier carries a resource name at all or a wrong one.
    let cases: [LookupCase; 10] = [
        (
            "one recorded resource",
            vec![home(EC2_VOLUME, "vol-0001")],
            &[(EC2_VOLUME, "vol-0001")],
            &[],
        ),
        (
            "several recorded resources of different types",
            vec![
                home(EC2_VOLUME, "vol-0001"),
                home(DYNAMODB_TABLE, "orders"),
                home(S3_BUCKET, "config-bucket"),
            ],
            &[
                (EC2_VOLUME, "vol-0001"),
                (DYNAMODB_TABLE, "orders"),
                (S3_BUCKET, "config-bucket"),
            ],
            &[],
        ),
        (
            "a resource of a type nothing was recorded for",
            vec![home(IAM_ROLE, "config-role")],
            &[],
            &[(IAM_ROLE, "config-role")],
        ),
        (
            "a resource id the inventory does not hold",
            vec![home(EC2_VOLUME, "vol-0404")],
            &[],
            &[(EC2_VOLUME, "vol-0404")],
        ),
        (
            "an id recorded under another type",
            vec![home(DYNAMODB_TABLE, "vol-0001")],
            &[],
            &[(DYNAMODB_TABLE, "vol-0001")],
        ),
        (
            "a deleted resource, which the API does not return",
            vec![home(EC2_VOLUME, "vol-0009")],
            &[],
            &[(EC2_VOLUME, "vol-0009")],
        ),
        (
            "a resource discovered with no configuration recorded",
            vec![home(LAMBDA_FUNCTION, "evaluate")],
            &[(LAMBDA_FUNCTION, "evaluate")],
            &[],
        ),
        (
            "another source account, whose data this aggregator holds none of",
            vec![identifier(
                OTHER_ACCOUNT,
                HOME_REGION,
                EC2_VOLUME,
                "vol-0001",
            )],
            &[],
            &[(EC2_VOLUME, "vol-0001")],
        ),
        (
            "another source Region, which holds no Config data of this account",
            vec![identifier(&account(), OTHER_REGION, EC2_VOLUME, "vol-0001")],
            &[],
            &[(EC2_VOLUME, "vol-0001")],
        ),
        (
            "a batch mixing resources the aggregator holds and ones it does not",
            vec![
                home(EC2_VOLUME, "vol-0404"),
                home(DYNAMODB_TABLE, "orders"),
                home(EC2_VOLUME, "vol-0009"),
                home(EC2_VOLUME, "vol-0001"),
            ],
            &[(DYNAMODB_TABLE, "orders"), (EC2_VOLUME, "vol-0001")],
            &[(EC2_VOLUME, "vol-0404"), (EC2_VOLUME, "vol-0009")],
        ),
    ];

    for (case, identifiers, expected_items, expected_unprocessed) in cases {
        let emulator = emulator_holding(&inventory()).await;
        put_whole_account_aggregator(&emulator).await;
        let request = batch(AGGREGATOR, &identifiers);

        let body = emulator
            .ok("BatchGetAggregateResourceConfig", &request)
            .await;
        assert_eq!(
            reported_items(case, &body),
            pairs(expected_items),
            "case {case:?}: expected the configuration items {expected_items:?}, got {body}"
        );
        assert_eq!(
            reported_unprocessed(case, &body),
            pairs(expected_unprocessed),
            "case {case:?}: expected the unprocessed identifiers {expected_unprocessed:?}, \
             got {body}"
        );

        // The read is a report: it must not change what the next read sees.
        let again = emulator
            .ok("BatchGetAggregateResourceConfig", &request)
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second batch read must report the same items, got {again}"
        );
    }
}

#[tokio::test]
async fn an_identifier_carrying_a_resource_name_is_answered_by_its_four_required_members() {
    // `ResourceName` is optional on an aggregate resource identifier, so it names no
    // resource on its own: rows differ along the name sent — none, the recorded one,
    // another resource's, and one nothing carries.
    let cases: [(&str, Option<&str>); 4] = [
        ("no resource name at all", None),
        ("the recorded resource name", Some("vol-0001")),
        ("another resource's name", Some("orders")),
        ("a name nothing carries", Some("no-such-name")),
    ];

    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    for (case, resource_name) in cases {
        let mut sent = home(EC2_VOLUME, "vol-0001");
        if let Some(resource_name) = resource_name {
            sent["ResourceName"] = json!(resource_name);
        }
        let body = emulator
            .ok("BatchGetAggregateResourceConfig", &batch(AGGREGATOR, &[sent]))
            .await;

        assert_eq!(
            reported_items(case, &body),
            pairs(&[(EC2_VOLUME, "vol-0001")]),
            "case {case:?}: the four required members identify the resource, got {body}"
        );
        // The item reports the name the INVENTORY holds, never the one sent.
        assert_eq!(
            body["BaseConfigurationItems"][0]["resourceName"],
            json!("vol-0001"),
            "case {case:?}: expected the recorded resourceName, got {body}"
        );
    }
}

#[tokio::test]
async fn the_aggregators_own_scope_decides_which_identifiers_it_can_answer() {
    let this_account = account();
    let home_only = [HOME_REGION];
    let elsewhere = [OTHER_REGION];

    // Rows differ along the aggregator's scope: which accounts it aggregates, over
    // which Regions, whether the scope comes from account sources or from an
    // organization, and which resource types its aggregator filters admit.
    let cases: [ScopeCase; 7] = [
        (
            "this account, every Region",
            json!({ "AccountAggregationSources": [account_source(&[&this_account], &[])] }),
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
            &[],
        ),
        (
            "this account, the Region the request is signed for",
            json!({
                "AccountAggregationSources":
                    [account_source(&[&this_account], &home_only)],
            }),
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
            &[],
        ),
        (
            "this account, another Region only",
            json!({
                "AccountAggregationSources":
                    [account_source(&[&this_account], &elsewhere)],
            }),
            &[],
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
        ),
        (
            "another account only",
            json!({
                "AccountAggregationSources": [account_source(&[OTHER_ACCOUNT], &[])],
            }),
            &[],
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
        ),
        (
            "the organization, every Region",
            json!({ "OrganizationAggregationSource": organization_source(&[]) }),
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
            &[],
        ),
        (
            "the organization, another Region only",
            json!({ "OrganizationAggregationSource": organization_source(&elsewhere) }),
            &[],
            &[(EC2_VOLUME, "vol-0001"), (DYNAMODB_TABLE, "orders")],
        ),
        (
            "this account, with a filter that takes in one resource type",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &[])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [EC2_VOLUME] },
                },
            }),
            &[(EC2_VOLUME, "vol-0001")],
            &[(DYNAMODB_TABLE, "orders")],
        ),
    ];

    for (case, sources, expected_items, expected_unprocessed) in cases {
        let emulator = emulator_holding(&inventory()).await;
        put_aggregator(&emulator, AGGREGATOR, sources).await;

        let body = emulator
            .ok(
                "BatchGetAggregateResourceConfig",
                &batch(
                    AGGREGATOR,
                    &[home(EC2_VOLUME, "vol-0001"), home(DYNAMODB_TABLE, "orders")],
                ),
            )
            .await;

        assert_eq!(
            reported_items(case, &body),
            pairs(expected_items),
            "case {case:?}: expected the configuration items {expected_items:?}, got {body}"
        );
        assert_eq!(
            reported_unprocessed(case, &body),
            pairs(expected_unprocessed),
            "case {case:?}: expected the unprocessed identifiers {expected_unprocessed:?}, \
             got {body}"
        );
    }
}

#[tokio::test]
async fn a_reported_configuration_item_is_the_recorded_configuration_state() {
    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along what was recorded for the resource: a configuration document
    // and a custom name, or a discovery with neither.
    let cases: [ItemCase; 3] = [
        (
            "a recorded volume",
            EC2_VOLUME,
            "vol-0001",
            Some("vol-0001"),
            Some(r#"{"state":"available"}"#),
        ),
        (
            "a recorded table",
            DYNAMODB_TABLE,
            "orders",
            Some("orders"),
            Some(r#"{"state":"available"}"#),
        ),
        (
            "a resource discovered with nothing recorded",
            LAMBDA_FUNCTION,
            "evaluate",
            None,
            None,
        ),
    ];

    for (case, resource_type, resource_id, resource_name, configuration) in cases {
        let body = emulator
            .ok(
                "BatchGetAggregateResourceConfig",
                &batch(AGGREGATOR, &[home(resource_type, resource_id)]),
            )
            .await;
        let item = &body["BaseConfigurationItems"][0];

        assert_eq!(
            (
                item["accountId"].as_str(),
                item["awsRegion"].as_str(),
                item["resourceType"].as_str(),
                item["resourceId"].as_str(),
            ),
            (
                Some(account().as_str()),
                Some(HOME_REGION),
                Some(resource_type),
                Some(resource_id),
            ),
            "case {case:?}: the item names the account, Region, type and id it was \
             recorded for, got {item}"
        );
        assert_eq!(
            item["resourceName"].as_str(),
            resource_name,
            "case {case:?}: expected resourceName {resource_name:?}, got {item}"
        );
        assert_eq!(
            item["configuration"].as_str(),
            configuration,
            "case {case:?}: expected the recorded configuration document, got {item}"
        );

        // The capture time and the state id come from the instant the configuration
        // item was recorded; a resource nothing was recorded for carries neither.
        let captured_seconds = captured_at()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the seeded capture time is after the epoch")
            .as_secs_f64();
        let expected_status = if configuration.is_some() {
            "OK"
        } else {
            "ResourceDiscovered"
        };
        assert_eq!(
            item["configurationItemStatus"].as_str(),
            Some(expected_status),
            "case {case:?}: expected configurationItemStatus {expected_status:?}, got {item}"
        );
        assert_eq!(
            item["configurationItemCaptureTime"].as_f64(),
            configuration.map(|_| captured_seconds),
            "case {case:?}: expected the capture time the inventory holds, got {item}"
        );
        assert_eq!(
            item["configurationStateId"].as_str(),
            configuration.map(|_| "1700000000000"),
            "case {case:?}: the state id orders the items of the resource by capture \
             instant in milliseconds, got {item}"
        );
        assert_eq!(
            item["version"].as_str(),
            configuration.map(|_| "00000000-0000-0000-0000-000000000001"),
            "case {case:?}: expected the recorded schema version, got {item}"
        );

        // "The API does not return tags and relationships", so the tags the inventory
        // holds for the resource appear nowhere in the item.
        assert!(
            !serde_json::to_string(item)
                .expect("the item serialises")
                .contains("platform"),
            "case {case:?}: an aggregate configuration item carries no tags, got {item}"
        );
        // Continuous recording reports neither the recording frequency nor a delivery
        // time.
        assert!(
            item.get("recordingFrequency").is_none()
                && item.get("configurationItemDeliveryTime").is_none(),
            "case {case:?}: under continuous recording neither member is reported, got {item}"
        );
    }
}

#[tokio::test]
async fn daily_recording_is_what_puts_the_frequency_and_the_delivery_time_in_the_item() {
    // Rows differ along the recording mode the account's recorder records with: the
    // default (continuous), a daily default, and a per-resource-type override that
    // makes one type daily and leaves the others continuous.
    let cases: [RecordingModeCase; 3] = [
        (
            "the default recording mode",
            json!({ "roleARN": ROLE_ARN }),
            &[(EC2_VOLUME, false), (DYNAMODB_TABLE, false)],
        ),
        (
            "a daily default",
            json!({
                "roleARN": ROLE_ARN,
                "recordingMode": { "recordingFrequency": DAILY },
            }),
            &[(EC2_VOLUME, true), (DYNAMODB_TABLE, true)],
        ),
        (
            "a daily override for one resource type",
            json!({
                "roleARN": ROLE_ARN,
                "recordingMode": {
                    "recordingFrequency": "CONTINUOUS",
                    "recordingModeOverrides": [{
                        "resourceTypes": [EC2_VOLUME],
                        "recordingFrequency": DAILY,
                    }],
                },
            }),
            &[(EC2_VOLUME, true), (DYNAMODB_TABLE, false)],
        ),
    ];

    for (case, recorder, expected) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("PutConfigurationRecorder", &recorder_request(recorder))
            .await;
        // A recorder needs a delivery channel before it can be turned on.
        emulator
            .ok(
                "PutDeliveryChannel",
                &json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
            )
            .await;
        // The recording mode is the one the RECORDING recorder records with, so the
        // recorder has to be turned on for it to apply.
        emulator
            .ok(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": RECORDER }),
            )
            .await;
        seed(&emulator, &inventory());
        put_whole_account_aggregator(&emulator).await;

        for (resource_type, daily) in expected {
            let resource_id = if *resource_type == EC2_VOLUME {
                "vol-0001"
            } else {
                "orders"
            };
            let body = emulator
                .ok(
                    "BatchGetAggregateResourceConfig",
                    &batch(AGGREGATOR, &[home(resource_type, resource_id)]),
                )
                .await;
            let item = &body["BaseConfigurationItems"][0];

            assert_eq!(
                item["recordingFrequency"].as_str(),
                daily.then_some(DAILY),
                "case {case:?}: {resource_type} reports its recordingFrequency only under \
                 daily recording, got {item}"
            );
            assert_eq!(
                item.get("configurationItemDeliveryTime").is_some(),
                *daily,
                "case {case:?}: {resource_type} reports a delivery time only under daily \
                 recording, got {item}"
            );
        }
    }
}

#[tokio::test]
async fn an_identifier_naming_a_resource_type_the_model_does_not_declare_is_refused() {
    // An aggregate resource identifier's `ResourceType` is the modelled `ResourceType`
    // enum, while `PutResourceConfig` records CUSTOM resource types
    // (`PutResourceConfigRequest$ResourceType` targets `ResourceTypeString`, a plain
    // 1..196 string): a configuration item recorded through that operation is
    // therefore not addressable by this one, which is why the Amazon Web Services
    // resource types every table above reads are seeded in the store.
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    emulator
        .ok(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
        )
        .await;
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along the custom resource type recorded and then asked for, each a
    // CloudFormation registry type name `PutResourceConfig` accepts.
    let cases: [(&str, &str, &str); 3] = [
        ("a network switch", "Acme::Network::Switch", "switch-1"),
        ("a storage vault", "Beta::Storage::Vault", "vault-1"),
        ("an analytics stream", "Zeta::Analytics::Stream", "stream-1"),
    ];

    for (case, resource_type, resource_id) in cases {
        emulator
            .ok(
                "PutResourceConfig",
                &json!({
                    "ResourceType": resource_type,
                    "SchemaVersionId": "00000000-0000-0000-0000-000000000002",
                    "ResourceId": resource_id,
                    "Configuration": r#"{"ports":4}"#,
                }),
            )
            .await;

        let (status, code) = emulator
            .refused(
                "BatchGetAggregateResourceConfig",
                &batch(AGGREGATOR, &[home(resource_type, resource_id)]),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: {resource_type} is no modelled ResourceType, so the batch is \
             refused 400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_batch_naming_an_aggregator_that_does_not_exist_is_refused() {
    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along the batch sent, so the refusal is about the aggregator rather
    // than about what the batch asked for.
    let cases: [(&str, &str, Vec<Value>); 4] = [
        (
            "a name no aggregator was created under",
            "no-such-aggregator",
            vec![home(EC2_VOLUME, "vol-0001")],
        ),
        (
            "that same name with a batch of several identifiers",
            "no-such-aggregator",
            vec![home(EC2_VOLUME, "vol-0001"), home(DYNAMODB_TABLE, "orders")],
        ),
        (
            "a name differing from the existing one only in case",
            "ORG-WIDE",
            vec![home(EC2_VOLUME, "vol-0001")],
        ),
        (
            "a name whose batch names nothing the inventory holds",
            "another-aggregator",
            vec![home(IAM_ROLE, "config-role")],
        ),
    ];

    for (case, aggregator, identifiers) in cases {
        let (status, code) = emulator
            .refused(
                "BatchGetAggregateResourceConfig",
                &batch(aggregator, &identifiers),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: an absent aggregator is refused 400 \
             NoSuchConfigurationAggregatorException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_batch_outside_the_identifier_count_the_operation_accepts_is_refused() {
    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    let one_under: Vec<Value> = Vec::new();
    let at_minimum: Vec<Value> = vec![home(EC2_VOLUME, "vol-0001")];
    let at_maximum: Vec<Value> = (0..MAX_IDENTIFIERS)
        .map(|index| home(EC2_VOLUME, &format!("vol-{index:04}")))
        .collect();
    let one_over: Vec<Value> = (0..=MAX_IDENTIFIERS)
        .map(|index| home(EC2_VOLUME, &format!("vol-{index:04}")))
        .collect();

    // One past each published bound is refused: a batch carrying nothing, and one
    // carrying a hundred and one identifiers.
    for (case, identifiers) in [
        ("no identifiers at all", &one_under),
        ("one identifier past the maximum", &one_over),
    ] {
        let (status, code) = emulator
            .refused(
                "BatchGetAggregateResourceConfig",
                &batch(AGGREGATOR, identifiers),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a batch of {} identifiers is outside {MIN_IDENTIFIERS}..\
             {MAX_IDENTIFIERS} and must be refused 400 ValidationException, got \
             {status} {code}",
            identifiers.len()
        );
    }

    // At each bound the same batch is served, and answers every identifier it carries.
    for (case, identifiers) in [
        ("one identifier", &at_minimum),
        ("identifiers at the maximum", &at_maximum),
    ] {
        let body = emulator
            .ok(
                "BatchGetAggregateResourceConfig",
                &batch(AGGREGATOR, identifiers),
            )
            .await;
        let answered = reported_items(case, &body).len() + reported_unprocessed(case, &body).len();
        assert_eq!(
            answered,
            identifiers.len(),
            "case {case:?}: every one of the {} identifiers sent is either answered or \
             reported unprocessed, got {body}",
            identifiers.len()
        );
    }
}
