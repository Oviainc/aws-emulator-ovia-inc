// SPDX-License-Identifier: CC0-1.0

//! What an aggregate discovered-resource COUNT reports, over the wire.
//!
//! The rule pinned here: `GetAggregateDiscoveredResourceCounts` counts the resources the
//! named configuration aggregator holds — the same inventory the aggregate listing
//! projects, excluding the resources whose deletion Config recorded — narrowed to the
//! sources the aggregator's scope takes in and by whichever `ResourceCountFilters`
//! members the request carries; it reports that number as
//! `TotalDiscoveredResources`, and ONE `GroupedResourceCount` per group of the
//! `GroupByKey` the request names (the resource type, the source account or the source
//! Region), biggest group first and by group name among groups holding equally many,
//! served in pages of at most `Limit` groups (1000 when the request asks for no limit of
//! its own) with a `NextToken` only while groups remain. A request that names no
//! `GroupByKey` gets no groups at all and still gets the total. A count naming an
//! aggregator this account does not hold is refused
//! `NoSuchConfigurationAggregatorException`, and a `NextToken` this operation did not
//! issue `InvalidNextTokenException`.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` already defines. The inventory is
//! written through the operation that writes it, `PutResourceConfig`, and the deletions
//! through `DeleteResourceConfig`, so what the count reports is state the service itself
//! persisted; the aggregators are created through `PutConfigurationAggregator` over the
//! same wire.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{with_recorder_and_channel, Emulator, ROLE_ARN};

/// Resource types the rows record: entries of `com.amazonaws.configservice#ResourceType`
/// (from `smithy_query(query="shape", shape="ResourceType")`) that the CloudFormation
/// registry holds a registration for, which is what `PutResourceConfig` records.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";
/// See [`EC2_VOLUME`].
const KMS_KEY: &str = "AWS::KMS::Key";
/// See [`EC2_VOLUME`]; a type no row records anything of.
const IAM_ROLE: &str = "AWS::IAM::Role";

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`).
const RECORDER: &str = "default";

/// The aggregator every case that is not about the aggregator's own scope uses.
const AGGREGATOR: &str = "org-wide";

/// The Region the harness signs its requests for, which is therefore the Region this
/// store's Config data belongs to and the source Region a count can be grouped by.
const HOME_REGION: &str = "us-east-1";
/// A Region this store holds no Config data for.
const OTHER_REGION: &str = "us-west-2";

/// A source account that is not this emulator's, in the twelve-digit form
/// `ResourceCountFilters$AccountId`'s `^\d{12}$` pattern allows.
const OTHER_ACCOUNT: &str = "111122223333";

/// The only published `AggregatorFilterType` value, from `smithy_query(query="shape",
/// shape="AggregatorFilterType")`: `INCLUDE`.
const FILTER_TYPE_INCLUDE: &str = "INCLUDE";

/// Every published `ResourceCountGroupKey` value, from `smithy_query(query="shape",
/// shape="ResourceCountGroupKey")`: `RESOURCE_TYPE`, `ACCOUNT_ID`, `AWS_REGION`.
const GROUP_BY_RESOURCE_TYPE: &str = "RESOURCE_TYPE";
/// See [`GROUP_BY_RESOURCE_TYPE`].
const GROUP_BY_ACCOUNT_ID: &str = "ACCOUNT_ID";
/// See [`GROUP_BY_RESOURCE_TYPE`].
const GROUP_BY_AWS_REGION: &str = "AWS_REGION";

/// The page size a request that states no limit of its own gets
/// (`GetAggregateDiscoveredResourceCountsRequest$Limit`: "The maximum number of
/// GroupedResourceCount objects returned on each page. The default is 1000. You cannot
/// specify a number greater than 1000. If you specify 0, Config uses the default").
const DEFAULT_LIMIT: usize = 1000;

/// The published bounds of `Limit` (`com.amazonaws.configservice#GroupByAPILimit`,
/// range 0..1000).
const LIMIT_MIN: i64 = 0;
/// See [`LIMIT_MIN`].
const LIMIT_MAX: i64 = 1000;

/// The groups a row expects, as `(groupName, resourceCount)` pairs in the order the
/// operation must report them.
type ExpectedGroups = &'static [(&'static str, i64)];

/// One row of the inventory table: the case name, how many resources of each resource
/// type the inventory holds, the groups expected when the count is grouped by resource
/// type, and the total expected whatever it is grouped by.
type InventoryCase = (
    &'static str,
    &'static [(&'static str, usize)],
    ExpectedGroups,
    i64,
);

/// One resource the inventory holds: its type, its id, and whether its deletion was
/// recorded after its configuration.
struct Recorded {
    resource_type: &'static str,
    resource_id: String,
    deleted: bool,
}

/// `count` resources of `resource_type`, each under an id of its own.
fn resources_of(resource_type: &'static str, count: usize) -> Vec<Recorded> {
    (0..count)
        .map(|index| Recorded {
            resource_type,
            resource_id: format!("res-{index:04}"),
            deleted: false,
        })
        .collect()
}

/// The inventory a `&[(resourceType, count)]` row describes.
fn inventory_of(spec: &[(&'static str, usize)]) -> Vec<Recorded> {
    spec.iter()
        .flat_map(|(resource_type, count)| resources_of(resource_type, *count))
        .collect()
}

/// A resource whose deletion Config recorded after recording its configuration.
fn deleted(resource_type: &'static str, resource_id: &str) -> Recorded {
    Recorded {
        resource_type,
        resource_id: resource_id.to_string(),
        deleted: true,
    }
}

/// This emulator's account id, the account whose Config data the store holds.
fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// The `PutResourceConfig` body recording one resource.
fn resource_config(resource: &Recorded) -> Value {
    json!({
        "ResourceType": resource.resource_type,
        "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
        "ResourceId": resource.resource_id,
        "Configuration": format!("{{\"id\":\"{}\"}}", resource.resource_id),
    })
}

/// Record `resources` into Config's inventory, recording the deletion of the ones a row
/// marks deleted.
async fn record(emulator: &Emulator, resources: &[Recorded]) {
    for resource in resources {
        emulator
            .ok("PutResourceConfig", &resource_config(resource))
            .await;
        if resource.deleted {
            emulator
                .ok(
                    "DeleteResourceConfig",
                    &json!({
                        "ResourceType": resource.resource_type,
                        "ResourceId": resource.resource_id,
                    }),
                )
                .await;
        }
    }
}

/// An emulator with a RECORDING configuration recorder holding `resources` in its
/// discovered-resource inventory: Config records a resource's configuration only while
/// a recorder is recording.
async fn emulator_recording(resources: &[Recorded]) -> Emulator {
    let emulator = Emulator::in_region(HOME_REGION);
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": RECORDER }),
        )
        .await;
    record(&emulator, resources).await;
    emulator
}

/// An account aggregation source over `accounts`, aggregating the Regions in `regions`,
/// or every Region when `regions` is empty.
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

/// Create an aggregator named [`AGGREGATOR`] that aggregates this account in every
/// Region.
async fn put_whole_account_aggregator(emulator: &Emulator) {
    put_aggregator(
        emulator,
        AGGREGATOR,
        json!({ "AccountAggregationSources": [account_source(&[&account()], &[])] }),
    )
    .await;
}

/// The `GetAggregateDiscoveredResourceCounts` body for `aggregator`, with `extra` folded
/// in (a group key, filters, a limit, a token).
fn counts(aggregator: &str, extra: &Value) -> Value {
    let mut body = json!({ "ConfigurationAggregatorName": aggregator });
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member.as_str()] = value.clone();
    }
    body
}

/// The same body, grouped by `group_by_key`.
fn counts_grouped_by(aggregator: &str, group_by_key: &str, extra: &Value) -> Value {
    let mut body = counts(aggregator, extra);
    body["GroupByKey"] = json!(group_by_key);
    body
}

/// The `(groupName, resourceCount)` pairs of a response, in the order reported.
fn reported_groups(case: &str, body: &Value) -> Vec<(String, i64)> {
    body["GroupedResourceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected GroupedResourceCounts, got {body}"))
        .iter()
        .map(|group| {
            (
                group["GroupName"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: every group reports a GroupName, got {body}")
                    })
                    .to_string(),
                group["ResourceCount"].as_i64().unwrap_or_else(|| {
                    panic!("case {case:?}: every group reports a ResourceCount, got {body}")
                }),
            )
        })
        .collect()
}

/// The `TotalDiscoveredResources` of a response.
fn reported_total(case: &str, body: &Value) -> i64 {
    body["TotalDiscoveredResources"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected TotalDiscoveredResources, got {body}"))
}

/// The groups `expected` names, as the pairs a response is compared against.
fn owned(expected: ExpectedGroups) -> Vec<(String, i64)> {
    expected
        .iter()
        .map(|(group_name, count)| (group_name.to_string(), *count))
        .collect()
}

/// How many resource identifiers `ListAggregateDiscoveredResources` reports for
/// `resource_type` — the same inventory, projected one resource at a time.
async fn listed_count(emulator: &Emulator, resource_type: &str) -> i64 {
    let mut total = 0;
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "ConfigurationAggregatorName": AGGREGATOR,
            "ResourceType": resource_type,
        });
        if let Some(resume_at) = token.as_deref() {
            body["NextToken"] = json!(resume_at);
        }
        let page = emulator
            .ok("ListAggregateDiscoveredResources", &body)
            .await;
        total += page["ResourceIdentifiers"]
            .as_array()
            .map(|identifiers| identifiers.len() as i64)
            .unwrap_or_default();
        match page["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return total,
        }
    }
}

#[tokio::test]
async fn the_count_groups_the_aggregated_inventory_by_the_key_the_request_names() {
    // Rows differ along what the rule generalises over: how many resource types the
    // inventory holds, how many resources of each, and whether two types hold equally
    // many — which is what makes the group name decide the order.
    let cases: [InventoryCase; 6] = [
        ("nothing recorded at all", &[], &[], 0),
        ("one resource of one type", &[(EC2_VOLUME, 1)], &[(EC2_VOLUME, 1)], 1),
        (
            "several resources of one type",
            &[(EC2_VOLUME, 4)],
            &[(EC2_VOLUME, 4)],
            4,
        ),
        (
            "three types, each holding a different number",
            &[(EC2_VOLUME, 3), (DYNAMODB_TABLE, 1), (S3_BUCKET, 2)],
            &[(EC2_VOLUME, 3), (S3_BUCKET, 2), (DYNAMODB_TABLE, 1)],
            6,
        ),
        (
            "two types holding equally many, ordered by group name",
            &[(S3_BUCKET, 2), (DYNAMODB_TABLE, 2)],
            &[(DYNAMODB_TABLE, 2), (S3_BUCKET, 2)],
            4,
        ),
        (
            "four types, two of them tied",
            &[
                (LAMBDA_FUNCTION, 1),
                (KMS_KEY, 5),
                (EC2_VOLUME, 2),
                (S3_BUCKET, 2),
            ],
            &[
                (KMS_KEY, 5),
                (EC2_VOLUME, 2),
                (S3_BUCKET, 2),
                (LAMBDA_FUNCTION, 1),
            ],
            10,
        ),
    ];

    let this_account = account();
    for (case, spec, expected_types, expected_total) in cases {
        let emulator = emulator_recording(&inventory_of(spec)).await;
        put_whole_account_aggregator(&emulator).await;

        // One group per source account and one per source Region: this store stands for
        // one account in one Region, so each of those keys names ONE group holding every
        // counted resource — and no group at all when it counted none.
        let by_account: Vec<(String, i64)> = if expected_total > 0 {
            vec![(this_account.clone(), expected_total)]
        } else {
            Vec::new()
        };
        let by_region: Vec<(String, i64)> = if expected_total > 0 {
            vec![(HOME_REGION.to_string(), expected_total)]
        } else {
            Vec::new()
        };

        let grouped: [(&str, Vec<(String, i64)>); 3] = [
            (GROUP_BY_RESOURCE_TYPE, owned(expected_types)),
            (GROUP_BY_ACCOUNT_ID, by_account),
            (GROUP_BY_AWS_REGION, by_region),
        ];
        for (group_by_key, expected) in grouped {
            let request = counts_grouped_by(AGGREGATOR, group_by_key, &json!({}));
            let body = emulator
                .ok("GetAggregateDiscoveredResourceCounts", &request)
                .await;
            assert_eq!(
                reported_groups(case, &body),
                expected,
                "case {case:?}: grouped by {group_by_key}, expected the groups \
                 {expected:?}, got {body}"
            );
            assert_eq!(
                reported_total(case, &body),
                expected_total,
                "case {case:?}: grouped by {group_by_key}, expected \
                 {expected_total} resources in total, got {body}"
            );
            assert_eq!(
                body["GroupByKey"].as_str(),
                Some(group_by_key),
                "case {case:?}: the response echoes the key the request passed in, got {body}"
            );
            assert!(
                body.get("NextToken").is_none(),
                "case {case:?}: a count that fits in one page carries no NextToken, got {body}"
            );

            // The count is a report: it must not change what the next read sees.
            let again = emulator
                .ok("GetAggregateDiscoveredResourceCounts", &request)
                .await;
            assert_eq!(
                again, body,
                "case {case:?}: a second count must report the same groups, got {again}"
            );
        }

        // A request that names no group key gets the total and no groups at all.
        let body = emulator
            .ok(
                "GetAggregateDiscoveredResourceCounts",
                &counts(AGGREGATOR, &json!({})),
            )
            .await;
        assert_eq!(
            reported_groups(case, &body),
            Vec::<(String, i64)>::new(),
            "case {case:?}: a count naming no GroupByKey reports no groups, got {body}"
        );
        assert!(
            body.get("GroupByKey").is_none(),
            "case {case:?}: a count naming no GroupByKey echoes none back, got {body}"
        );
        assert_eq!(
            reported_total(case, &body),
            expected_total,
            "case {case:?}: expected {expected_total} resources in total, got {body}"
        );

        // Each per-type count is the number of resources the aggregate LISTING reports
        // for that type: one inventory, two projections of it.
        for (resource_type, expected_count) in expected_types {
            let listed = listed_count(&emulator, resource_type).await;
            assert_eq!(
                listed, *expected_count,
                "case {case:?}: the count of {resource_type} must equal the \
                 {listed} identifiers the aggregate listing reports"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_whose_deletion_is_recorded_is_counted_no_longer() {
    // Rows differ along what the write path does to the inventory between two counts:
    // recording resources, recording the deletion of some of them, and recording one
    // again afterwards.
    let emulator = emulator_recording(&[]).await;
    put_whole_account_aggregator(&emulator).await;

    let steps: [(&str, Vec<Recorded>, ExpectedGroups, i64); 5] = [
        ("nothing recorded yet", vec![], &[], 0),
        (
            "two volumes and a table are recorded",
            vec![
                Recorded {
                    resource_type: EC2_VOLUME,
                    resource_id: "vol-0001".to_string(),
                    deleted: false,
                },
                Recorded {
                    resource_type: EC2_VOLUME,
                    resource_id: "vol-0002".to_string(),
                    deleted: false,
                },
                Recorded {
                    resource_type: DYNAMODB_TABLE,
                    resource_id: "orders".to_string(),
                    deleted: false,
                },
            ],
            &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)],
            3,
        ),
        (
            "one volume's deletion is recorded",
            vec![deleted(EC2_VOLUME, "vol-0002")],
            &[(DYNAMODB_TABLE, 1), (EC2_VOLUME, 1)],
            2,
        ),
        (
            "the table's deletion is recorded too",
            vec![deleted(DYNAMODB_TABLE, "orders")],
            &[(EC2_VOLUME, 1)],
            1,
        ),
        (
            "the deleted volume is recorded again",
            vec![Recorded {
                resource_type: EC2_VOLUME,
                resource_id: "vol-0002".to_string(),
                deleted: false,
            }],
            &[(EC2_VOLUME, 2)],
            2,
        ),
    ];

    for (case, resources, expected_types, expected_total) in steps {
        record(&emulator, &resources).await;

        let body = emulator
            .ok(
                "GetAggregateDiscoveredResourceCounts",
                &counts_grouped_by(AGGREGATOR, GROUP_BY_RESOURCE_TYPE, &json!({})),
            )
            .await;
        assert_eq!(
            reported_groups(case, &body),
            owned(expected_types),
            "case {case:?}: expected the groups {expected_types:?}, got {body}"
        );
        assert_eq!(
            reported_total(case, &body),
            expected_total,
            "case {case:?}: expected {expected_total} resources in total, got {body}"
        );
    }
}

#[tokio::test]
async fn only_the_resources_the_aggregator_takes_in_are_counted() {
    let spec: &[(&str, usize)] = &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)];
    let this_account = account();
    let elsewhere = [OTHER_REGION];

    // Rows differ along the scope the aggregator was created with: which accounts it
    // takes in, which Regions, whether it aggregates an organization instead, and
    // whether its aggregator filters admit a resource type.
    let cases: [(&str, Value, ExpectedGroups, i64); 8] = [
        (
            "this account, every Region",
            json!({ "AccountAggregationSources": [account_source(&[&this_account], &[])] }),
            &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)],
            3,
        ),
        (
            "this account, the Region its data belongs to",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &[HOME_REGION])],
            }),
            &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)],
            3,
        ),
        (
            "this account, another Region only",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &elsewhere)],
            }),
            &[],
            0,
        ),
        (
            "another account only",
            json!({ "AccountAggregationSources": [account_source(&[OTHER_ACCOUNT], &[])] }),
            &[],
            0,
        ),
        (
            "this account beside another, every Region",
            json!({
                "AccountAggregationSources": [
                    account_source(&[OTHER_ACCOUNT, &this_account], &[]),
                ],
            }),
            &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)],
            3,
        ),
        (
            "the organization, every Region",
            json!({ "OrganizationAggregationSource": organization_source(&[]) }),
            &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)],
            3,
        ),
        (
            "the organization, another Region only",
            json!({ "OrganizationAggregationSource": organization_source(&elsewhere) }),
            &[],
            0,
        ),
        (
            "this account, with a filter that takes in one resource type only",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &[])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [DYNAMODB_TABLE] },
                },
            }),
            &[(DYNAMODB_TABLE, 1)],
            1,
        ),
    ];

    for (case, sources, expected_types, expected_total) in cases {
        let emulator = emulator_recording(&inventory_of(spec)).await;
        put_aggregator(&emulator, AGGREGATOR, sources).await;

        let body = emulator
            .ok(
                "GetAggregateDiscoveredResourceCounts",
                &counts_grouped_by(AGGREGATOR, GROUP_BY_RESOURCE_TYPE, &json!({})),
            )
            .await;
        assert_eq!(
            reported_groups(case, &body),
            owned(expected_types),
            "case {case:?}: expected the groups {expected_types:?}, got {body}"
        );
        assert_eq!(
            reported_total(case, &body),
            expected_total,
            "case {case:?}: expected {expected_total} resources in total, got {body}"
        );
    }
}

#[tokio::test]
async fn the_filters_narrow_what_is_counted() {
    let spec: &[(&str, usize)] = &[(EC2_VOLUME, 3), (DYNAMODB_TABLE, 1)];
    let this_account = account();

    // Rows differ along which `ResourceCountFilters` member narrows the count — the
    // resource type, the source account, the source Region, and all three together —
    // and along whether the value names what this store holds.
    let cases: [(&str, Value, ExpectedGroups, i64); 8] = [
        ("no filters at all", json!({}), &[(EC2_VOLUME, 3), (DYNAMODB_TABLE, 1)], 4),
        (
            "narrowed to one recorded resource type",
            json!({ "Filters": { "ResourceType": EC2_VOLUME } }),
            &[(EC2_VOLUME, 3)],
            3,
        ),
        (
            "narrowed to another recorded resource type",
            json!({ "Filters": { "ResourceType": DYNAMODB_TABLE } }),
            &[(DYNAMODB_TABLE, 1)],
            1,
        ),
        (
            "narrowed to a type nothing was recorded of",
            json!({ "Filters": { "ResourceType": IAM_ROLE } }),
            &[],
            0,
        ),
        (
            "narrowed to the source account the data belongs to",
            json!({ "Filters": { "AccountId": this_account.clone() } }),
            &[(EC2_VOLUME, 3), (DYNAMODB_TABLE, 1)],
            4,
        ),
        (
            "narrowed to another source account",
            json!({ "Filters": { "AccountId": OTHER_ACCOUNT } }),
            &[],
            0,
        ),
        (
            "narrowed to another source Region",
            json!({ "Filters": { "Region": OTHER_REGION } }),
            &[],
            0,
        ),
        (
            "narrowed by the type, the account and the Region together",
            json!({
                "Filters": {
                    "ResourceType": EC2_VOLUME,
                    "AccountId": this_account.clone(),
                    "Region": HOME_REGION,
                },
            }),
            &[(EC2_VOLUME, 3)],
            3,
        ),
    ];

    for (case, extra, expected_types, expected_total) in cases {
        let emulator = emulator_recording(&inventory_of(spec)).await;
        put_whole_account_aggregator(&emulator).await;

        let body = emulator
            .ok(
                "GetAggregateDiscoveredResourceCounts",
                &counts_grouped_by(AGGREGATOR, GROUP_BY_RESOURCE_TYPE, &extra),
            )
            .await;
        assert_eq!(
            reported_groups(case, &body),
            owned(expected_types),
            "case {case:?}: expected the groups {expected_types:?}, got {body}"
        );
        assert_eq!(
            reported_total(case, &body),
            expected_total,
            "case {case:?}: expected {expected_total} resources in total, got {body}"
        );

        // The total is the whole narrowed count, so grouping it by the source account
        // reports the same number in one group.
        let by_account = emulator
            .ok(
                "GetAggregateDiscoveredResourceCounts",
                &counts_grouped_by(AGGREGATOR, GROUP_BY_ACCOUNT_ID, &extra),
            )
            .await;
        assert_eq!(
            reported_total(case, &by_account),
            expected_total,
            "case {case:?}: the same filters count the same resources whatever the \
             grouping, got {by_account}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_its_token_resumes_the_count() {
    // The inventory holds a different number of resources of each of five types, so
    // every group holds a different count and the whole ordering is exercised.
    let spec: &[(&str, usize)] = &[
        (EC2_VOLUME, 5),
        (DYNAMODB_TABLE, 4),
        (S3_BUCKET, 3),
        (LAMBDA_FUNCTION, 2),
        (KMS_KEY, 1),
    ];
    let expected: Vec<(String, i64)> = owned(&[
        (EC2_VOLUME, 5),
        (DYNAMODB_TABLE, 4),
        (S3_BUCKET, 3),
        (LAMBDA_FUNCTION, 2),
        (KMS_KEY, 1),
    ]);

    // Rows differ along how many groups one page may carry: no limit at all (the
    // default page size), sizes under, at and over the number of groups, and each
    // published bound of `Limit`.
    let cases: [(&str, Option<i64>); 7] = [
        ("no limit", None),
        ("pages of one", Some(1)),
        ("pages of two", Some(2)),
        ("pages of five, exactly the number of groups", Some(5)),
        ("pages of six, one more than there are groups", Some(6)),
        ("the published minimum, which means the default", Some(LIMIT_MIN)),
        ("the published maximum", Some(LIMIT_MAX)),
    ];

    for (case, limit) in cases {
        let emulator = emulator_recording(&inventory_of(spec)).await;
        put_whole_account_aggregator(&emulator).await;
        let page_size = match limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(stated) => stated as usize,
        };

        let mut walked: Vec<(String, i64)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = json!({});
            if let Some(stated) = limit {
                extra["Limit"] = json!(stated);
            }
            if let Some(resume_at) = token.as_deref() {
                extra["NextToken"] = json!(resume_at);
            }
            let body = emulator
                .ok(
                    "GetAggregateDiscoveredResourceCounts",
                    &counts_grouped_by(AGGREGATOR, GROUP_BY_RESOURCE_TYPE, &extra),
                )
                .await;
            let page = reported_groups(case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} groups, got {body}"
            );
            assert_eq!(
                reported_total(case, &body),
                15,
                "case {case:?}: every page reports the whole total, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: paging must terminate, walked {pages} pages of {body}"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: walking every page must report each group once, biggest first"
        );
        let expected_pages = expected.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of at most {page_size} of \
             {} groups, walked {pages}",
            expected.len()
        );
    }
}

#[tokio::test]
async fn a_count_of_an_aggregator_this_account_does_not_hold_is_refused() {
    let spec: &[(&str, usize)] = &[(EC2_VOLUME, 2)];

    // Rows differ along the request shape, so the refusal is about the aggregator rather
    // than about what the count asked for.
    let cases: [(&str, Value); 4] = [
        ("a bare count", json!({})),
        (
            "a count grouped by resource type",
            json!({ "GroupByKey": GROUP_BY_RESOURCE_TYPE }),
        ),
        (
            "a count narrowed by a filter",
            json!({ "Filters": { "ResourceType": EC2_VOLUME } }),
        ),
        ("a count asking for one group a page", json!({ "Limit": 1 })),
    ];

    for (case, extra) in cases {
        let emulator = emulator_recording(&inventory_of(spec)).await;
        let request = counts("no-such-aggregator", &extra);

        let (status, code) = emulator
            .refused("GetAggregateDiscoveredResourceCounts", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: an unknown aggregator is refused 400 \
             NoSuchConfigurationAggregatorException, got {status} {code}"
        );

        // The same count is served once an aggregator of that name exists.
        put_aggregator(
            &emulator,
            "no-such-aggregator",
            json!({ "AccountAggregationSources": [account_source(&[&account()], &[])] }),
        )
        .await;
        let body = emulator
            .ok("GetAggregateDiscoveredResourceCounts", &request)
            .await;
        assert_eq!(
            reported_total(case, &body),
            2,
            "case {case:?}: expected the two recorded resources to be counted, got {body}"
        );
    }
}

#[tokio::test]
async fn a_token_this_operation_did_not_issue_and_a_limit_past_the_range_are_refused() {
    let spec: &[(&str, usize)] = &[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)];
    let emulator = emulator_recording(&inventory_of(spec)).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along how the token fails to be one this operation issued: not base64
    // at all, a resource type sent in its place, base64 of bytes that are not text, a
    // group name without the count it sorts by, and a count that is not a number.
    for (case, token) in [
        ("not base64 at all", "!!! not a token !!!".to_string()),
        ("a resource type sent in place of a token", EC2_VOLUME.to_string()),
        ("base64 of bytes that are not text", "/w==".to_string()),
        (
            "a group name with no count before it",
            aws_smithy_types::base64::encode(EC2_VOLUME),
        ),
        (
            "a count that is not a number",
            aws_smithy_types::base64::encode(format!("many|{EC2_VOLUME}")),
        ),
    ] {
        let (status, code) = emulator
            .refused(
                "GetAggregateDiscoveredResourceCounts",
                &counts_grouped_by(
                    AGGREGATOR,
                    GROUP_BY_RESOURCE_TYPE,
                    &json!({ "NextToken": token }),
                ),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: a NextToken this operation did not issue is refused 400 \
             InvalidNextTokenException, got {status} {code}"
        );
    }

    // A limit outside the published range is refused — one past each bound. The modelled
    // `@range(0..1000)` on `Limit` is enforced at the request boundary, so a value past
    // either bound never reaches the handler and is refused there as a
    // `ValidationException`; the handler's own `InvalidLimitException` branch stands as
    // defence in depth for a direct caller of `handle`.
    for (case, limit) in [
        ("below the minimum", LIMIT_MIN - 1),
        ("above the maximum", LIMIT_MAX + 1),
    ] {
        let (status, code) = emulator
            .refused(
                "GetAggregateDiscoveredResourceCounts",
                &counts(AGGREGATOR, &json!({ "Limit": limit })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: Limit {limit} is outside {LIMIT_MIN}..{LIMIT_MAX} and must be \
             refused 400 ValidationException, got {status} {code}"
        );
    }

    // At each published bound the same count is served.
    for (case, limit) in [("the minimum", LIMIT_MIN), ("the maximum", LIMIT_MAX)] {
        let body = emulator
            .ok(
                "GetAggregateDiscoveredResourceCounts",
                &counts_grouped_by(
                    AGGREGATOR,
                    GROUP_BY_RESOURCE_TYPE,
                    &json!({ "Limit": limit }),
                ),
            )
            .await;
        assert_eq!(
            reported_groups(case, &body),
            owned(&[(EC2_VOLUME, 2), (DYNAMODB_TABLE, 1)]),
            "case {case:?}: Limit {limit} is inside the published range and is served, \
             got {body}"
        );
    }

    // A group key the modelled enum does not publish is a bad request, not a group.
    let (status, code) = emulator
        .refused(
            "GetAggregateDiscoveredResourceCounts",
            &counts_grouped_by(AGGREGATOR, "RESOURCE_NAME", &json!({})),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "a GroupByKey outside {GROUP_BY_RESOURCE_TYPE}, {GROUP_BY_ACCOUNT_ID}, \
         {GROUP_BY_AWS_REGION} must be refused 400 ValidationException, got {status} {code}"
    );
}
