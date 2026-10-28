// SPDX-License-Identifier: CC0-1.0

//! What an aggregate discovered-resource listing reports, over the wire.
//!
//! The rule pinned here: `ListAggregateDiscoveredResources` reports, one page at a time,
//! the aggregate resource identifier — source account, source Region, resource type,
//! resource id and the custom name where the resource carries one — of every resource of
//! the ONE requested resource type that the named configuration aggregator takes in and
//! whose deletion Config has not recorded, narrowed by whichever `ResourceFilters`
//! members the request carries, ordered by resource id and served in pages of at most
//! `Limit` identifiers (100 when the request asks for no limit of its own) with a
//! `NextToken` only while resources remain; a listing naming an aggregator this account
//! does not hold is refused `NoSuchConfigurationAggregatorException`, and a `NextToken`
//! this operation did not issue `InvalidNextTokenException`.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` already defines. The inventory is
//! written through the operation that writes it, `PutResourceConfig`, and the deletions
//! through `DeleteResourceConfig`, so what the listing reports is state the service
//! itself persisted; the aggregators are created through `PutConfigurationAggregator`
//! over the same wire.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{with_recorder_and_channel, Emulator, ROLE_ARN};

/// Resource types the rows use: entries of `com.amazonaws.configservice#ResourceType`
/// (from `smithy_query(query="shape", shape="ResourceType")`) that the CloudFormation
/// registry holds a registration for, which is what `PutResourceConfig` records.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";
/// See [`EC2_VOLUME`]; a type no row records anything of.
const IAM_ROLE: &str = "AWS::IAM::Role";

/// The name Config assigns a configuration recorder created without one
/// (`ConfigurationRecorder$name`).
const RECORDER: &str = "default";

/// The aggregator every case that is not about the aggregator's own scope uses.
const AGGREGATOR: &str = "org-wide";

/// The Region the harness signs its requests for, which is therefore the Region this
/// store's Config data belongs to and the source Region every identifier names.
const HOME_REGION: &str = "us-east-1";
/// A Region this store holds no Config data for.
const OTHER_REGION: &str = "us-west-2";

/// A source account that is not this emulator's, in the twelve-digit form
/// `ResourceFilters$AccountId`'s `^\d{12}$` pattern allows.
const OTHER_ACCOUNT: &str = "111122223333";

/// The only published `AggregatorFilterType` value, from `smithy_query(query="shape",
/// shape="AggregatorFilterType")`: `INCLUDE`.
const FILTER_TYPE_INCLUDE: &str = "INCLUDE";

/// The page size a request that states no limit of its own gets
/// (`ListAggregateDiscoveredResourcesRequest$Limit`: "The maximum number of resource
/// identifiers returned on each page. You cannot specify a number greater than 100. If
/// you specify 0, Config uses the default").
const DEFAULT_LIMIT: usize = 100;

/// The published bounds of `Limit` (`com.amazonaws.configservice#Limit`, range 0..100).
const LIMIT_MIN: i64 = 0;
/// See [`LIMIT_MIN`].
const LIMIT_MAX: i64 = 100;

/// One step of the inventory table: the case name, the resources recorded at that step,
/// whether the resource's DELETION is recorded instead, and the `(resource id, custom
/// name)` pairs the listing is expected to report afterwards.
type InventoryStep = (
    &'static str,
    Vec<Recorded>,
    bool,
    &'static [(&'static str, Option<&'static str>)],
);

/// One resource the inventory holds: its type, its id, the custom name it is recorded
/// with if any, and whether its deletion was recorded afterwards.
struct Recorded {
    resource_type: &'static str,
    resource_id: String,
    resource_name: Option<&'static str>,
    deleted: bool,
}

/// A resource recorded with the custom name `resource_name`, or without one when it is
/// `None`.
fn recorded(
    resource_type: &'static str,
    resource_id: &str,
    resource_name: Option<&'static str>,
) -> Recorded {
    Recorded {
        resource_type,
        resource_id: resource_id.to_string(),
        resource_name,
        deleted: false,
    }
}

/// A resource recorded with the custom name `resource_name`.
fn named(
    resource_type: &'static str,
    resource_id: &str,
    resource_name: &'static str,
) -> Recorded {
    recorded(resource_type, resource_id, Some(resource_name))
}

/// A resource recorded without a custom name.
fn unnamed(resource_type: &'static str, resource_id: &str) -> Recorded {
    recorded(resource_type, resource_id, None)
}

/// A resource whose deletion Config recorded after recording its configuration.
fn deleted(
    resource_type: &'static str,
    resource_id: &str,
    resource_name: &'static str,
) -> Recorded {
    Recorded {
        deleted: true,
        ..named(resource_type, resource_id, resource_name)
    }
}

/// This emulator's account id, the account whose Config data the store holds.
fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// The `PutResourceConfig` body recording one resource.
fn resource_config(resource: &Recorded) -> Value {
    let mut body = json!({
        "ResourceType": resource.resource_type,
        "SchemaVersionId": "00000000-0000-0000-0000-000000000001",
        "ResourceId": resource.resource_id,
        "Configuration": format!("{{\"id\":\"{}\"}}", resource.resource_id),
    });
    if let Some(resource_name) = resource.resource_name {
        body["ResourceName"] = json!(resource_name);
    }
    body
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

/// The `ListAggregateDiscoveredResources` body for one resource type, with `extra`
/// folded in (filters, a limit, a token).
fn listing(aggregator: &str, resource_type: &str, extra: &Value) -> Value {
    let mut body = json!({
        "ConfigurationAggregatorName": aggregator,
        "ResourceType": resource_type,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member.as_str()] = value.clone();
    }
    body
}

/// The `ResourceId` of every identifier in a response, in the order reported.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    identifiers(case, body)
        .iter()
        .map(|identifier| {
            identifier["ResourceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every identifier reports a ResourceId, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// The identifiers of a response.
fn identifiers(case: &str, body: &Value) -> Vec<Value> {
    body["ResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {body}"))
        .clone()
}

#[tokio::test]
async fn a_listing_reports_the_aggregated_identifiers_the_request_narrows_to() {
    let inventory = [
        named(EC2_VOLUME, "vol-0001", "data-volume"),
        unnamed(EC2_VOLUME, "vol-0002"),
        deleted(EC2_VOLUME, "vol-0003", "spare-volume"),
        named(DYNAMODB_TABLE, "orders", "orders"),
        named(S3_BUCKET, "config-bucket", "config-bucket"),
        deleted(LAMBDA_FUNCTION, "evaluate", "evaluate"),
    ];
    let this_account = account();

    // Rows differ along what the rule generalises over: the resource type asked for
    // (including one nothing was recorded of and one whose only resource was deleted),
    // and which `ResourceFilters` members narrow the listing — the source account, the
    // source Region, the resource id, the custom name, and all four together.
    let cases: [(&str, &str, Value, &[&str]); 15] = [
        (
            "every aggregated resource of a type",
            EC2_VOLUME,
            json!({}),
            &["vol-0001", "vol-0002"],
        ),
        (
            "another type entirely",
            DYNAMODB_TABLE,
            json!({}),
            &["orders"],
        ),
        (
            "a third type, recorded with its own name",
            S3_BUCKET,
            json!({}),
            &["config-bucket"],
        ),
        (
            "a type nothing was recorded of",
            IAM_ROLE,
            json!({}),
            &[],
        ),
        (
            "a type whose only resource was deleted",
            LAMBDA_FUNCTION,
            json!({}),
            &[],
        ),
        (
            "narrowed to the source account the data belongs to",
            EC2_VOLUME,
            json!({ "Filters": { "AccountId": this_account } }),
            &["vol-0001", "vol-0002"],
        ),
        (
            "narrowed to another source account",
            EC2_VOLUME,
            json!({ "Filters": { "AccountId": OTHER_ACCOUNT } }),
            &[],
        ),
        (
            "narrowed to the source Region the data belongs to",
            EC2_VOLUME,
            json!({ "Filters": { "Region": HOME_REGION } }),
            &["vol-0001", "vol-0002"],
        ),
        (
            "narrowed to another source Region",
            EC2_VOLUME,
            json!({ "Filters": { "Region": OTHER_REGION } }),
            &[],
        ),
        (
            "narrowed to one resource id",
            EC2_VOLUME,
            json!({ "Filters": { "ResourceId": "vol-0002" } }),
            &["vol-0002"],
        ),
        (
            "narrowed to a resource id nothing was recorded under",
            EC2_VOLUME,
            json!({ "Filters": { "ResourceId": "vol-0404" } }),
            &[],
        ),
        (
            "narrowed to the resource id of a deleted resource",
            EC2_VOLUME,
            json!({ "Filters": { "ResourceId": "vol-0003" } }),
            &[],
        ),
        (
            "narrowed to a custom name one resource carries",
            EC2_VOLUME,
            json!({ "Filters": { "ResourceName": "data-volume" } }),
            &["vol-0001"],
        ),
        (
            "narrowed to a custom name nothing carries",
            EC2_VOLUME,
            json!({ "Filters": { "ResourceName": "no-such-name" } }),
            &[],
        ),
        (
            "narrowed by the account, the Region, the id and the name together",
            EC2_VOLUME,
            json!({
                "Filters": {
                    "AccountId": this_account,
                    "Region": HOME_REGION,
                    "ResourceId": "vol-0001",
                    "ResourceName": "data-volume",
                },
            }),
            &["vol-0001"],
        ),
    ];

    for (case, resource_type, extra, expected) in cases {
        let emulator = emulator_recording(&inventory).await;
        put_whole_account_aggregator(&emulator).await;
        let request = listing(AGGREGATOR, resource_type, &extra);

        let body = emulator
            .ok("ListAggregateDiscoveredResources", &request)
            .await;
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected the resource ids {expected:?}, got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {case:?}: a listing that fits in one page carries no NextToken, got {body}"
        );

        // Every identifier names the source this store's data belongs to, the type
        // asked for, and the custom name the resource was recorded with — and only
        // where it was recorded with one.
        for identifier in identifiers(case, &body) {
            let reported_id = identifier["ResourceId"].as_str().unwrap_or_default();
            let recorded = inventory
                .iter()
                .find(|resource| resource.resource_id == reported_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {reported_id:?} is not in the inventory, got {body}")
                });
            assert_eq!(
                identifier["SourceAccountId"].as_str(),
                Some(this_account.as_str()),
                "case {case:?}: expected the source account {this_account:?}, got {identifier}"
            );
            assert_eq!(
                identifier["SourceRegion"].as_str(),
                Some(HOME_REGION),
                "case {case:?}: expected the source Region {HOME_REGION:?}, got {identifier}"
            );
            assert_eq!(
                identifier["ResourceType"].as_str(),
                Some(resource_type),
                "case {case:?}: expected the resource type {resource_type:?}, got {identifier}"
            );
            assert_eq!(
                identifier["ResourceName"].as_str(),
                recorded.resource_name,
                "case {case:?}: expected the custom name {:?} for {reported_id:?}, got \
                 {identifier}",
                recorded.resource_name
            );
            // The identifier a listing reports is one a single aggregate read answers:
            // the same resource, read back through `GetAggregateResourceConfig`.
            let item = emulator
                .ok(
                    "GetAggregateResourceConfig",
                    &json!({
                        "ConfigurationAggregatorName": AGGREGATOR,
                        "ResourceIdentifier": identifier,
                    }),
                )
                .await;
            assert_eq!(
                item["ConfigurationItem"]["resourceId"].as_str(),
                Some(reported_id),
                "case {case:?}: every listed identifier must read back, got {item}"
            );
        }

        // The listing is a report: it must not change what the next read sees.
        let again = emulator
            .ok("ListAggregateDiscoveredResources", &request)
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second listing must report the same identifiers, got {again}"
        );
    }
}

#[tokio::test]
async fn only_the_resources_the_aggregator_takes_in_are_listed() {
    let inventory = [
        named(EC2_VOLUME, "vol-0001", "data-volume"),
        named(DYNAMODB_TABLE, "orders", "orders"),
    ];
    let this_account = account();
    let elsewhere = [OTHER_REGION];

    // Rows differ along the scope the aggregator was created with: which accounts it
    // takes in, which Regions, whether it aggregates an organization instead, and
    // whether its aggregator filters admit the resource type asked for. Each row states
    // what is reported for BOTH recorded types, so a scope that narrows one is held to
    // reporting the other.
    let cases: [(&str, Value, &[&str], &[&str]); 8] = [
        (
            "this account, every Region",
            json!({ "AccountAggregationSources": [account_source(&[&this_account], &[])] }),
            &["vol-0001"],
            &["orders"],
        ),
        (
            "this account, the Region its data belongs to",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &[HOME_REGION])],
            }),
            &["vol-0001"],
            &["orders"],
        ),
        (
            "this account, another Region only",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &elsewhere)],
            }),
            &[],
            &[],
        ),
        (
            "another account only",
            json!({ "AccountAggregationSources": [account_source(&[OTHER_ACCOUNT], &[])] }),
            &[],
            &[],
        ),
        (
            "this account beside another, every Region",
            json!({
                "AccountAggregationSources": [
                    account_source(&[OTHER_ACCOUNT, &this_account], &[]),
                ],
            }),
            &["vol-0001"],
            &["orders"],
        ),
        (
            "the organization, every Region",
            json!({ "OrganizationAggregationSource": organization_source(&[]) }),
            &["vol-0001"],
            &["orders"],
        ),
        (
            "the organization, another Region only",
            json!({ "OrganizationAggregationSource": organization_source(&elsewhere) }),
            &[],
            &[],
        ),
        (
            "this account, with a filter that takes in another resource type only",
            json!({
                "AccountAggregationSources": [account_source(&[&this_account], &[])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [DYNAMODB_TABLE] },
                },
            }),
            &[],
            &["orders"],
        ),
    ];

    for (case, sources, expected_volumes, expected_tables) in cases {
        let emulator = emulator_recording(&inventory).await;
        put_aggregator(&emulator, AGGREGATOR, sources).await;

        for (resource_type, expected) in
            [(EC2_VOLUME, expected_volumes), (DYNAMODB_TABLE, expected_tables)]
        {
            let body = emulator
                .ok(
                    "ListAggregateDiscoveredResources",
                    &listing(AGGREGATOR, resource_type, &json!({})),
                )
                .await;
            assert_eq!(
                reported_ids(case, &body),
                expected,
                "case {case:?}: expected the resource ids {expected:?} of {resource_type}, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_its_token_resumes_the_listing() {
    // Rows differ along how many resources of the type the aggregator holds and how
    // many identifiers one page may carry: no limit at all (the default page size),
    // each published bound of `Limit`, and sizes under, at and over the page size.
    let cases: [(&str, usize, Option<i64>); 8] = [
        ("nothing recorded, no limit", 0, None),
        ("one resource, no limit", 1, None),
        ("seven resources, pages of one", 7, Some(1)),
        ("seven resources, pages of three", 7, Some(3)),
        ("seven resources, pages of seven", 7, Some(7)),
        (
            "seven resources, limit at the published maximum",
            7,
            Some(LIMIT_MAX),
        ),
        (
            "seven resources, limit at the published minimum, which means the default",
            7,
            Some(LIMIT_MIN),
        ),
        (
            "one resource past the default page size, no limit",
            DEFAULT_LIMIT + 1,
            None,
        ),
    ];

    for (case, size, limit) in cases {
        let inventory: Vec<Recorded> = (0..size)
            .map(|index| unnamed(EC2_VOLUME, &format!("vol-{index:04}")))
            .collect();
        let expected: Vec<String> = inventory
            .iter()
            .map(|resource| resource.resource_id.clone())
            .collect();
        let page_size = match limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(stated) => stated as usize,
        };
        let emulator = emulator_recording(&inventory).await;
        put_whole_account_aggregator(&emulator).await;

        let mut walked: Vec<String> = Vec::new();
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
                    "ListAggregateDiscoveredResources",
                    &listing(AGGREGATOR, EC2_VOLUME, &extra),
                )
                .await;
            let page = reported_ids(case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} identifiers, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= size + 1,
                "case {case:?}: paging must terminate, walked {pages} pages of {body}"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, expected,
            "case {case:?}: walking every page must report each aggregated resource once, \
             in resource-id order"
        );
        let expected_pages = size.max(1).div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of at most {page_size} of {size} \
             resources, walked {pages}"
        );
    }
}

#[tokio::test]
async fn a_listing_of_an_aggregator_this_account_does_not_hold_is_refused() {
    let inventory = [named(EC2_VOLUME, "vol-0001", "data-volume")];

    // Rows differ along the request shape, so the refusal is about the aggregator
    // rather than about what the listing asked for.
    let cases: [(&str, &str, Value); 4] = [
        ("a bare listing", EC2_VOLUME, json!({})),
        (
            "a listing narrowed by id",
            EC2_VOLUME,
            json!({ "Filters": { "ResourceId": "vol-0001" } }),
        ),
        (
            "a listing of a type nothing was recorded of",
            IAM_ROLE,
            json!({}),
        ),
        (
            "a listing asking for one identifier a page",
            EC2_VOLUME,
            json!({ "Limit": 1 }),
        ),
    ];

    for (case, resource_type, extra) in cases {
        let emulator = emulator_recording(&inventory).await;
        let request = listing("no-such-aggregator", resource_type, &extra);

        let (status, code) = emulator
            .refused("ListAggregateDiscoveredResources", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: an unknown aggregator is refused 400 \
             NoSuchConfigurationAggregatorException, got {status} {code}"
        );

        // The same listing is served once an aggregator of that name exists.
        put_aggregator(
            &emulator,
            "no-such-aggregator",
            json!({ "AccountAggregationSources": [account_source(&[&account()], &[])] }),
        )
        .await;
        let body = emulator
            .ok("ListAggregateDiscoveredResources", &request)
            .await;
        let expected: &[&str] = if resource_type == EC2_VOLUME {
            &["vol-0001"]
        } else {
            &[]
        };
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected the resource ids {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_token_this_operation_did_not_issue_and_a_limit_past_the_range_are_refused() {
    let inventory = [
        named(EC2_VOLUME, "vol-0001", "data-volume"),
        unnamed(EC2_VOLUME, "vol-0002"),
    ];
    let emulator = emulator_recording(&inventory).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along how the token fails to be one this operation issued: not base64
    // at all, a resource id sent in its place, and base64 of bytes that are not text.
    for (case, token) in [
        ("not base64 at all", "!!! not a token !!!"),
        ("a resource id sent in place of a token", "vol-0001"),
        ("base64 of bytes that are not text", "/w=="),
    ] {
        let (status, code) = emulator
            .refused(
                "ListAggregateDiscoveredResources",
                &listing(AGGREGATOR, EC2_VOLUME, &json!({ "NextToken": token })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: a NextToken this operation did not issue is refused 400 \
             InvalidNextTokenException, got {status} {code}"
        );
    }

    // A limit outside the published range is refused — one past each bound.
    for (case, limit) in [
        ("below the minimum", LIMIT_MIN - 1),
        ("above the maximum", LIMIT_MAX + 1),
    ] {
        let (status, code) = emulator
            .refused(
                "ListAggregateDiscoveredResources",
                &listing(AGGREGATOR, EC2_VOLUME, &json!({ "Limit": limit })),
            )
            .await;
        // The modelled `@range(0..100)` on `Limit` is enforced at the request boundary,
        // so a value past either bound never reaches the handler and is refused there as
        // a `ValidationException`; the handler's own `InvalidLimitException` branch is
        // therefore unreachable over the wire and stands as defence in depth for a
        // direct caller of `handle`.
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: Limit {limit} is outside {LIMIT_MIN}..{LIMIT_MAX} and must be \
             refused 400 ValidationException, got {status} {code}"
        );
    }

    // At each published bound the same listing is served.
    for (case, limit) in [("the minimum", LIMIT_MIN), ("the maximum", LIMIT_MAX)] {
        let body = emulator
            .ok(
                "ListAggregateDiscoveredResources",
                &listing(AGGREGATOR, EC2_VOLUME, &json!({ "Limit": limit })),
            )
            .await;
        assert_eq!(
            reported_ids(case, &body),
            ["vol-0001", "vol-0002"],
            "case {case:?}: Limit {limit} is inside the published range and is served, got \
             {body}"
        );
    }
}

#[tokio::test]
async fn the_listing_follows_what_the_inventory_holds() {
    // Rows differ along what the write path does to one resource between two listings:
    // recording it, re-recording it under another custom name, recording its deletion,
    // and recording it again afterwards.
    let emulator = emulator_recording(&[]).await;
    put_whole_account_aggregator(&emulator).await;

    let listed = |body: &Value| -> Vec<(String, Option<String>)> {
        identifiers("the listing follows the inventory", body)
            .iter()
            .map(|identifier| {
                (
                    identifier["ResourceId"].as_str().unwrap_or_default().to_string(),
                    identifier["ResourceName"].as_str().map(str::to_string),
                )
            })
            .collect()
    };

    let steps: [InventoryStep; 4] = [
        (
            "the resource is recorded",
            vec![named(EC2_VOLUME, "vol-0001", "data-volume")],
            false,
            &[("vol-0001", Some("data-volume"))],
        ),
        (
            "it is recorded again under another name",
            vec![named(EC2_VOLUME, "vol-0001", "spare-volume")],
            false,
            &[("vol-0001", Some("spare-volume"))],
        ),
        (
            "its deletion is recorded",
            vec![named(EC2_VOLUME, "vol-0001", "spare-volume")],
            true,
            &[],
        ),
        (
            "it is recorded once more",
            vec![named(EC2_VOLUME, "vol-0001", "data-volume")],
            false,
            &[("vol-0001", Some("data-volume"))],
        ),
    ];

    for (case, resources, delete, expected) in steps {
        if delete {
            emulator
                .ok(
                    "DeleteResourceConfig",
                    &json!({ "ResourceType": EC2_VOLUME, "ResourceId": "vol-0001" }),
                )
                .await;
        } else {
            record(&emulator, &resources).await;
        }

        let body = emulator
            .ok(
                "ListAggregateDiscoveredResources",
                &listing(AGGREGATOR, EC2_VOLUME, &json!({})),
            )
            .await;
        let expected: Vec<(String, Option<String>)> = expected
            .iter()
            .map(|(resource_id, resource_name)| {
                (resource_id.to_string(), resource_name.map(str::to_string))
            })
            .collect();
        assert_eq!(
            listed(&body),
            expected,
            "case {case:?}: expected the identifiers {expected:?}, got {body}"
        );
    }
}
