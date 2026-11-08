// SPDX-License-Identifier: CC0-1.0

//! What an aggregate configuration-item read reports, end to end over the wire.
//!
//! Two reads are pinned here: `BatchGetAggregateResourceConfig`, which answers a LIST of
//! aggregate resource identifiers, and `GetAggregateResourceConfig`, which answers ONE —
//! the same lookup, with the difference that a single read has a resource to refuse
//! about rather than a list to report unprocessed identifiers in (see the second half of
//! this file).
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
//! `GetAggregateResourceConfig` is the SAME read for ONE identifier, so the rule pinned
//! for it is that rule: it answers the identifier with exactly the configuration item a
//! batch read answers it with, member for member over everything the two output shapes
//! share, and where the aggregator holds no current configuration item for the
//! identifier — the case the batch reports unprocessed — a single read has no item to
//! answer with and is refused `ResourceNotDiscoveredException`. Its output is the FULL
//! `ConfigurationItem`, so it adds the resource's tags, which only the APIs documented
//! to drop them leave out.
//!
//! Two further properties of a reported item are pinned with it: an item carries the
//! configuration state the inventory holds — the account, Region, type, id, name,
//! schema version, configuration document, capture time and the state id that orders
//! the resource's items — and, in a BATCH read, never the resource's tags, which that
//! API does not return; and it reports a `recordingFrequency` and a
//! `configurationItemDeliveryTime` only where the recording recorder records that
//! resource type daily.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` already defines, and creates its
//! aggregators through `PutConfigurationAggregator` over that wire. The recorded
//! resources themselves are seeded in the store the way a handler reaches it
//! (`service_state`, as `discovered_resources` seeds the reader it pins), because these
//! rows pin members of a configuration item that no write operation takes — the
//! account, the Region, the capture instant and the supplementary blocks a recorder
//! captures.

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

/// The published length bounds of the aggregator name both reads carry
/// (`ConfigurationAggregatorName`, `@length(min: 1, max: 256)` with pattern
/// `^[\w\-]+$`, from `smithy_query(query="shape",
/// shape="ConfigurationAggregatorName")`).
const AGGREGATOR_NAME_MIN_LENGTH: usize = 1;
/// See [`AGGREGATOR_NAME_MIN_LENGTH`].
const AGGREGATOR_NAME_MAX_LENGTH: usize = 256;

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

/// One row of the re-recording table: the case name, the configuration document and
/// the tags the resource is recorded again with, and whether that state differs from
/// the state it was first recorded in.
type RerecordCase = (
    &'static str,
    Option<&'static str>,
    &'static [(&'static str, &'static str)],
    bool,
);

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
                ..DiscoveredResourceEntry::default()
            },
        );
    }
}

/// Rewrite the recorded state of one seeded resource, the way a later recording of the
/// same resource would: its configuration document and the tags it was recorded with.
fn rerecord(
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
    configuration: Option<&str>,
    tags: &[(&str, &str)],
) {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    let entry = store
        .discovered_resources
        .get_mut(&(resource_type.to_string(), resource_id.to_string()))
        .expect("the resource was seeded");
    entry.configuration = configuration.map(str::to_string);
    entry.tags = tags
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
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

/// The `GetAggregateResourceConfig` body for one identifier.
fn single(aggregator: &str, resource_identifier: &Value) -> Value {
    json!({
        "ConfigurationAggregatorName": aggregator,
        "ResourceIdentifier": resource_identifier,
    })
}

/// Every member a `BaseConfigurationItem` and a `ConfigurationItem` share, from
/// `smithy_query(query="shape", shape="BaseConfigurationItem,ConfigurationItem")`: a
/// base item is the configuration item without `tags`, `relationships`,
/// `relatedEvents` and `configurationItemMD5Hash`, so everything else the batch read
/// reports the single read must report identically.
const SHARED_MEMBERS: &[&str] = &[
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
        // Each seeded entry holds ONE configuration item, so each is the item that
        // DISCOVERED its resource — whether or not a configuration document was recorded
        // with it ("ResourceDiscovered – The resource was newly discovered",
        // `ConfigurationItem$configurationItemStatus`).
        let expected_status = "ResourceDiscovered";
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
    // enum, so a type outside it is refused at the boundary — whatever the inventory
    // holds, which is why the Amazon Web Services resource types every table above
    // reads are seeded in the store.
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

    // Rows differ along the resource type asked for: a CloudFormation registry type
    // name of a third-party organization, and the account-level type the modelled enum
    // does not list.
    let cases: [(&str, &str, &str); 4] = [
        ("a network switch", "Acme::Network::Switch", "switch-1"),
        ("a storage vault", "Beta::Storage::Vault", "vault-1"),
        ("an analytics stream", "Zeta::Analytics::Stream", "stream-1"),
        ("the account itself", "AWS::::Account", "000000000000"),
    ];

    for (case, resource_type, resource_id) in cases {
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

#[tokio::test]
async fn a_single_read_answers_an_identifier_with_the_item_a_batch_read_answers_it_with() {
    // Rows differ along the same class the batch table spans: which resource type and
    // id the identifier names, whether the resource is recorded, discovered without a
    // configuration, deleted or absent, and whether the source account and Region are
    // the ones this store holds data for.
    let cases: [(&str, Value, bool); 10] = [
        ("a recorded volume", home(EC2_VOLUME, "vol-0001"), true),
        ("a recorded table", home(DYNAMODB_TABLE, "orders"), true),
        ("a recorded bucket", home(S3_BUCKET, "config-bucket"), true),
        (
            "a resource discovered with no configuration recorded",
            home(LAMBDA_FUNCTION, "evaluate"),
            true,
        ),
        (
            "a resource of a type nothing was recorded for",
            home(IAM_ROLE, "config-role"),
            false,
        ),
        (
            "a resource id the inventory does not hold",
            home(EC2_VOLUME, "vol-0404"),
            false,
        ),
        (
            "an id recorded under another type",
            home(DYNAMODB_TABLE, "vol-0001"),
            false,
        ),
        (
            "a deleted resource, which the API does not return",
            home(EC2_VOLUME, "vol-0009"),
            false,
        ),
        (
            "another source account, whose data this aggregator holds none of",
            identifier(OTHER_ACCOUNT, HOME_REGION, EC2_VOLUME, "vol-0001"),
            false,
        ),
        (
            "another source Region, which holds no Config data of this account",
            identifier(&account(), OTHER_REGION, EC2_VOLUME, "vol-0001"),
            false,
        ),
    ];

    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    for (case, sent, holds_item) in cases {
        let batched = emulator
            .ok(
                "BatchGetAggregateResourceConfig",
                &batch(AGGREGATOR, std::slice::from_ref(&sent)),
            )
            .await;

        if !holds_item {
            assert_eq!(
                reported_unprocessed(case, &batched),
                pairs(&[(
                    sent["ResourceType"].as_str().expect("the row sends a type"),
                    sent["ResourceId"].as_str().expect("the row sends an id"),
                )]),
                "case {case:?}: the batch read reports the identifier unprocessed, got {batched}"
            );
            let (status, code) = emulator
                .refused("GetAggregateResourceConfig", &single(AGGREGATOR, &sent))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotDiscoveredException"),
                "case {case:?}: the aggregator holds no configuration item for the \
                 identifier, so a single read is refused 400 \
                 ResourceNotDiscoveredException, got {status} {code}"
            );
            continue;
        }

        let body = emulator
            .ok("GetAggregateResourceConfig", &single(AGGREGATOR, &sent))
            .await;
        let item = &body["ConfigurationItem"];
        let base = &batched["BaseConfigurationItems"][0];
        for member in SHARED_MEMBERS {
            assert_eq!(
                item[member], base[member],
                "case {case:?}: both reads must report {member:?} alike, got {} from the \
                 single read and {} from the batch read",
                item[member], base[member]
            );
        }

        // The read is a report: it must not change what the next read sees.
        let again = emulator
            .ok("GetAggregateResourceConfig", &single(AGGREGATOR, &sent))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second single read must report the same item, got {again}"
        );
    }
}

#[tokio::test]
async fn the_aggregators_own_scope_decides_whether_a_single_read_finds_the_resource() {
    let this_account = account();
    let home_only = [HOME_REGION];
    let elsewhere = [OTHER_REGION];

    // Rows differ along the aggregator's scope: which accounts it aggregates, over
    // which Regions, whether the scope comes from account sources or from an
    // organization, and which resource types its aggregator filters admit.
    let cases: [(&str, Value, bool); 6] = [
        (
            "this account, every Region",
            json!({ "AccountAggregationSources": [account_source(&[&this_account], &[])] }),
            true,
        ),
        (
            "this account, the Region the request is signed for",
            json!({
                "AccountAggregationSources":
                    [account_source(&[&this_account], &home_only)],
            }),
            true,
        ),
        (
            "this account, another Region only",
            json!({
                "AccountAggregationSources":
                    [account_source(&[&this_account], &elsewhere)],
            }),
            false,
        ),
        (
            "another account only",
            json!({ "AccountAggregationSources": [account_source(&[OTHER_ACCOUNT], &[])] }),
            false,
        ),
        (
            "the organization, every Region",
            json!({ "OrganizationAggregationSource": organization_source(&[]) }),
            true,
        ),
        (
            "this account, with a filter that takes in another resource type only",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &[])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [DYNAMODB_TABLE] },
                },
            }),
            false,
        ),
    ];

    for (case, sources, holds_item) in cases {
        let emulator = emulator_holding(&inventory()).await;
        put_aggregator(&emulator, AGGREGATOR, sources).await;
        let request = single(AGGREGATOR, &home(EC2_VOLUME, "vol-0001"));

        if !holds_item {
            let (status, code) = emulator
                .refused("GetAggregateResourceConfig", &request)
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotDiscoveredException"),
                "case {case:?}: the resource is outside what this aggregator holds, so \
                 the read is refused 400 ResourceNotDiscoveredException, got {status} \
                 {code}"
            );
            continue;
        }

        let body = emulator.ok("GetAggregateResourceConfig", &request).await;
        assert_eq!(
            (
                body["ConfigurationItem"]["resourceType"].as_str(),
                body["ConfigurationItem"]["resourceId"].as_str(),
            ),
            (Some(EC2_VOLUME), Some("vol-0001")),
            "case {case:?}: expected the configuration item of the volume, got {body}"
        );
    }
}

#[tokio::test]
async fn a_single_read_reports_the_tags_the_inventory_recorded_with_the_item() {
    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along the resource read: recorded resources of three types and one
    // discovered with no configuration document at all. Every one of them is seeded
    // carrying the same single tag, which only the APIs documented to drop tags leave
    // out — this one reports them.
    let rows: [(&str, &str, &str); 4] = [
        ("a recorded volume", EC2_VOLUME, "vol-0001"),
        ("a recorded table", DYNAMODB_TABLE, "orders"),
        ("a recorded bucket", S3_BUCKET, "config-bucket"),
        ("a discovered function", LAMBDA_FUNCTION, "evaluate"),
    ];

    for (case, resource_type, resource_id) in rows {
        let body = emulator
            .ok(
                "GetAggregateResourceConfig",
                &single(AGGREGATOR, &home(resource_type, resource_id)),
            )
            .await;
        let item = &body["ConfigurationItem"];

        assert_eq!(
            item["tags"],
            json!({ "owner": "platform" }),
            "case {case:?}: the item reports the tags the inventory recorded, got {item}"
        );
    }

    // Re-recording the resource changes which tags the item reports, and nothing else
    // about the read changes what a later read sees.
    let request = single(AGGREGATOR, &home(EC2_VOLUME, "vol-0001"));

    // Rows differ along what the re-recording changed: the configuration document, the
    // tags, both, and nothing at all.
    let changes: [RerecordCase; 4] = [
        (
            "another configuration document",
            Some(r#"{"state":"in-use"}"#),
            &[("owner", "platform")],
            true,
        ),
        (
            "another tag value",
            Some(r#"{"state":"available"}"#),
            &[("owner", "data")],
            true,
        ),
        (
            "no tags at all",
            Some(r#"{"state":"available"}"#),
            &[],
            true,
        ),
        (
            "the state it was recorded in",
            Some(r#"{"state":"available"}"#),
            &[("owner", "platform")],
            false,
        ),
    ];

    for (case, configuration, tags, _differs) in changes {
        rerecord(&emulator, EC2_VOLUME, "vol-0001", configuration, tags);
        let body = emulator.ok("GetAggregateResourceConfig", &request).await;
        let expected_tags: Value = if tags.is_empty() {
            Value::Null
        } else {
            Value::Object(
                tags.iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            )
        };
        assert_eq!(
            body["ConfigurationItem"]["tags"], expected_tags,
            "case {case:?}: the item reports the tags the inventory now holds, got {body}"
        );
        assert_eq!(
            body["ConfigurationItem"]["configuration"].as_str(),
            configuration,
            "case {case:?}: the item reports the configuration document now recorded, \
             got {body}"
        );
        // The read is a report: reading twice in the same state reports the same item.
        let again = emulator.ok("GetAggregateResourceConfig", &request).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read of one recorded state reports the same item, \
             got {again}"
        );
    }
}

#[tokio::test]
async fn a_single_read_naming_an_aggregator_that_does_not_exist_is_refused() {
    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along the identifier sent with the unknown aggregator name, so the
    // refusal is about the aggregator rather than about the resource asked for: the
    // aggregator is checked BEFORE the resource, so even an identifier the inventory
    // holds nothing for is refused for the aggregator.
    let cases: [(&str, &str, Value); 4] = [
        (
            "a name no aggregator was created under",
            "no-such-aggregator",
            home(EC2_VOLUME, "vol-0001"),
        ),
        (
            "that same name with an identifier the inventory holds nothing for",
            "no-such-aggregator",
            home(IAM_ROLE, "config-role"),
        ),
        (
            "a name differing from the existing one only in case",
            "ORG-WIDE",
            home(DYNAMODB_TABLE, "orders"),
        ),
        (
            "another unknown name, naming a deleted resource",
            "another-aggregator",
            home(EC2_VOLUME, "vol-0009"),
        ),
    ];

    for (case, aggregator, sent) in cases {
        let (status, code) = emulator
            .refused("GetAggregateResourceConfig", &single(aggregator, &sent))
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
async fn a_single_read_whose_aggregator_name_misses_the_published_bounds_is_refused() {
    let emulator = emulator_holding(&inventory()).await;
    put_whole_account_aggregator(&emulator).await;
    // The same identifier throughout, so each row is about the NAME it carries and
    // nothing else: the inventory holds a current configuration item for it, which the
    // aggregator created above answers.
    let sent = home(EC2_VOLUME, "vol-0001");

    // A name INSIDE what the member accepts reaches the lookup, so it is answered
    // about the aggregator it names — none of these named one — rather than refused as
    // malformed. Rows sit at each published length bound and on the character classes
    // the pattern admits.
    let accepted: [(&str, String); 4] = [
        (
            "the shortest name the member accepts",
            "x".repeat(AGGREGATOR_NAME_MIN_LENGTH),
        ),
        (
            "the longest name the member accepts",
            "y".repeat(AGGREGATOR_NAME_MAX_LENGTH),
        ),
        ("a word character and a hyphen", "org_wide-2".to_string()),
        ("digits alone", "90210".to_string()),
    ];
    for (case, name) in accepted {
        let (status, code) = emulator
            .refused("GetAggregateResourceConfig", &single(&name, &sent))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: the name is inside what the member accepts, so the read is \
             refused for the aggregator it names with 400 \
             NoSuchConfigurationAggregatorException, got {status} {code}"
        );
    }

    // A name OUTSIDE it names no aggregator at all, so the request is refused at the
    // boundary before the lookup: one past each published length bound, and names the
    // pattern does not admit.
    let malformed: [(&str, String); 4] = [
        (
            "one character short of the minimum",
            "z".repeat(AGGREGATOR_NAME_MIN_LENGTH - 1),
        ),
        (
            "one character past the maximum",
            "z".repeat(AGGREGATOR_NAME_MAX_LENGTH + 1),
        ),
        ("a name with a space", "org wide".to_string()),
        ("a name with a slash", "org/wide".to_string()),
    ];
    for (case, name) in malformed {
        let (status, code) = emulator
            .refused("GetAggregateResourceConfig", &single(&name, &sent))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: the name is outside what the member accepts, so the read is \
             refused 400 ValidationException, got {status} {code}"
        );
    }

    // Every refusal left the inventory alone: the aggregator still answers the
    // identifier with its configuration item.
    let body = emulator
        .ok("GetAggregateResourceConfig", &single(AGGREGATOR, &sent))
        .await;
    assert_eq!(
        (
            body["ConfigurationItem"]["resourceType"].as_str(),
            body["ConfigurationItem"]["resourceId"].as_str(),
        ),
        (Some(EC2_VOLUME), Some("vol-0001")),
        "expected the refusals to change nothing the read reports, got {body}"
    );
}
