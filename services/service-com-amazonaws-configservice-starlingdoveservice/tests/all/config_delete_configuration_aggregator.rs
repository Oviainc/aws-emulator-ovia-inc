// SPDX-License-Identifier: CC0-1.0

//! Deleting a configuration aggregator, end to end over the wire.
//!
//! The rule pinned here: `DeleteConfigurationAggregator` removes the aggregator the
//! request names from the account's state together with the data aggregated under it,
//! so every later read of that name reports it as absent, every other aggregator and
//! every other resource is left untouched, and a name holding no aggregator is refused
//! with the modelled `NoSuchConfigurationAggregatorException` rather than answered as a
//! no-op.
//!
//! Each case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines. Aggregated resources are
//! seeded in the store the way a handler reaches it (`service_state`), because the
//! aggregate identifier's `ResourceType` is the modelled `ResourceType` enum while
//! `PutResourceConfig` records only custom resource types.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};

use crate::config_setup_and_rule_loop::{Emulator, ROLE_ARN};

/// Twelve-digit source account ids, the form `AccountId`'s `^\d{12}$` pattern allows.
const ACCOUNT_A: &str = "111122223333";
/// See [`ACCOUNT_A`].
const ACCOUNT_B: &str = "444455556666";

/// The only published `AggregatorFilterType` value, from
/// `smithy_query(query="shape", shape="AggregatorFilterType")`: `INCLUDE`.
const FILTER_TYPE_INCLUDE: &str = "INCLUDE";

/// Entries of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// The published bounds of `ConfigurationAggregatorName`, from
/// `smithy_query(query="operation", operation="DeleteConfigurationAggregator")`:
/// `@length(min: 1, max: 256)` with pattern `^[\w\-]+$`.
const NAME_MIN_LENGTH: usize = 1;
/// See [`NAME_MIN_LENGTH`].
const NAME_MAX_LENGTH: usize = 256;

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

/// Create the aggregator `name` from the `PutConfigurationAggregator` body `sources`,
/// returning the ARN Config assigned it.
async fn put_aggregator(emulator: &Emulator, name: &str, sources: &Value) -> String {
    let mut request = sources.clone();
    request["ConfigurationAggregatorName"] = json!(name);
    let created = emulator.ok("PutConfigurationAggregator", &request).await;
    created["ConfigurationAggregator"]["ConfigurationAggregatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: the created aggregator must carry an ARN, got {created}"))
        .to_string()
}

/// Delete the aggregator `name`, requiring a 200 and an empty response body: the
/// operation's output shape is `Unit`, so there is nothing else to report.
async fn delete_aggregator(emulator: &Emulator, name: &str) {
    let body = emulator
        .ok(
            "DeleteConfigurationAggregator",
            &json!({ "ConfigurationAggregatorName": name }),
        )
        .await;
    assert_eq!(
        body,
        json!({}),
        "case {name:?}: DeleteConfigurationAggregator models no output members, got {body}"
    );
}

/// Every aggregator name the account holds, in the order reported.
async fn all_names(emulator: &Emulator) -> Vec<String> {
    let response = emulator
        .ok("DescribeConfigurationAggregators", &json!({}))
        .await;
    response["ConfigurationAggregators"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of aggregators, got {response}"))
        .iter()
        .map(|aggregator| {
            aggregator["ConfigurationAggregatorName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Put one recorded resource into the discovered-resource inventory the way a handler
/// reaches it, so an aggregator has data to aggregate.
fn seed_resource(emulator: &Emulator, resource_type: &str, resource_id: &str) {
    let captured_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    state.lock().discovered_resources.insert(
        (resource_type.to_string(), resource_id.to_string()),
        DiscoveredResourceEntry {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            resource_name: Some(resource_id.to_string()),
            deleted_at: None,
            schema_version_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
            configuration: Some(r#"{"state":"available"}"#.to_string()),
            tags: Default::default(),
            first_recorded_at: Some(captured_at),
            last_recorded_at: Some(captured_at),
        },
    );
}

/// The `BatchGetAggregateResourceConfig` body naming one of this account's resources
/// under `aggregator`.
fn aggregate_read(aggregator: &str, resource_type: &str, resource_id: &str, region: &str) -> Value {
    json!({
        "ConfigurationAggregatorName": aggregator,
        "ResourceIdentifiers": [{
            "SourceAccountId": emulator_server::emulator_account_id(),
            "SourceRegion": region,
            "ResourceType": resource_type,
            "ResourceId": resource_id,
        }],
    })
}

/// The rows every lifecycle case runs over: the FORM of the aggregator being deleted —
/// which kind of single aggregation source it has, how many accounts it names, whether
/// it aggregates every Region or a listed set, whether it filters, and whether it
/// carries tags.
fn aggregator_forms() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one account, one region",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])] }),
        ),
        (
            "two accounts, three regions, tagged",
            json!({
                "AccountAggregationSources": [
                    account_source(&[ACCOUNT_A, ACCOUNT_B], &["us-east-1", "eu-west-2", "ap-south-1"]),
                ],
                "Tags": [{ "Key": "team", "Value": "platform" }],
            }),
        ),
        (
            "one account, every region",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &[])] }),
        ),
        (
            "an organization over two regions",
            json!({ "OrganizationAggregationSource": organization_source(&["us-west-2", "eu-west-1"]) }),
        ),
        (
            "an organization over every region, tagged",
            json!({
                "OrganizationAggregationSource": organization_source(&[]),
                "Tags": [{ "Key": "owner", "Value": "security" }],
            }),
        ),
        (
            "accounts with a resource-type filter",
            json!({
                "AccountAggregationSources": [account_source(&[ACCOUNT_B], &["us-east-1"])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [EC2_VOLUME, S3_BUCKET] },
                },
            }),
        ),
        (
            "an organization with a service-principal filter",
            json!({
                "OrganizationAggregationSource": organization_source(&["us-east-1"]),
                "AggregatorFilters": {
                    "ServicePrincipal": {
                        "Type": FILTER_TYPE_INCLUDE,
                        "Value": ["config.amazonaws.com"],
                    },
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn a_deleted_aggregator_is_absent_from_every_read_whatever_form_it_took() {
    for (case, sources) in aggregator_forms() {
        let emulator = Emulator::new();
        let name = "doomed-aggregator";
        let arn = put_aggregator(&emulator, name, &sources).await;

        // It exists before the delete, so what follows is the delete's doing.
        assert_eq!(
            all_names(&emulator).await,
            vec![name.to_string()],
            "case {case:?}: expected the aggregator to exist before the delete"
        );

        delete_aggregator(&emulator, name).await;

        let reported = all_names(&emulator).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected no aggregator left in state, got {reported:?}"
        );

        // Reading the deleted name by name is refused, not answered with an empty page.
        let (status, code) = emulator
            .refused(
                "DescribeConfigurationAggregators",
                &json!({ "ConfigurationAggregatorNames": [name] }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: expected the deleted name to be refused, got {status} {code}"
        );

        // The tags lived on the aggregator, so the ARN resolves to no resource at all
        // ("When a resource is deleted, the tags associated with that resource are
        // deleted as well").
        let (status, code) = emulator
            .refused("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the deleted aggregator's ARN to resolve to nothing, got \
             {status} {code}"
        );
    }
}

#[tokio::test]
async fn the_data_aggregated_under_a_deleted_aggregator_is_gone_with_it() {
    // Rows differ along WHAT is aggregated: which resource type is read back under the
    // aggregator, and whether the aggregator admits every type or filters to one.
    // The aggregator has to aggregate THIS account for the store's own Config data to
    // be in its scope, so the source names it.
    let this_account = emulator_server::emulator_account_id().to_string();
    let home = account_source(&[this_account.as_str()], &[]);
    let cases: [(&str, Value, &str); 3] = [
        (
            "every type, a volume",
            json!({ "AccountAggregationSources": [home.clone()] }),
            EC2_VOLUME,
        ),
        (
            "every type, a bucket",
            json!({ "AccountAggregationSources": [home.clone()] }),
            S3_BUCKET,
        ),
        (
            "filtered to the volume type",
            json!({
                "AccountAggregationSources": [home.clone()],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [EC2_VOLUME] },
                },
            }),
            EC2_VOLUME,
        ),
    ];

    for (case, sources, resource_type) in cases {
        let emulator = Emulator::new();
        let name = "aggregating";
        let resource_id = "resource-0001";
        seed_resource(&emulator, resource_type, resource_id);
        put_aggregator(&emulator, name, &sources).await;

        // The aggregator holds the resource's configuration item before the delete.
        let request = aggregate_read(name, resource_type, resource_id, &emulator.region);
        let before = emulator
            .ok("BatchGetAggregateResourceConfig", &request)
            .await;
        let items = before["BaseConfigurationItems"].as_array();
        assert_eq!(
            items.map(Vec::len),
            Some(1),
            "case {case:?}: expected the aggregator to hold the resource before the delete, \
             got {before}"
        );

        delete_aggregator(&emulator, name).await;

        // After the delete there is no aggregator to aggregate under, so the aggregate
        // read is refused rather than answered from the data it used to report.
        let (status, code) = emulator
            .refused("BatchGetAggregateResourceConfig", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: expected the aggregate read to be refused after the delete, got \
             {status} {code}"
        );
    }
}

#[tokio::test]
async fn deleting_one_aggregator_leaves_every_other_resource_in_state() {
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    // Rows differ along WHICH of the aggregators in state are deleted and how many:
    // the first, one in the middle, the last, a pair, and every one of them.
    let cases: [(&str, &[&str]); 5] = [
        ("the first", &["alpha"]),
        ("one in the middle", &["charlie"]),
        ("the last", &["echo"]),
        ("two of them", &["bravo", "delta"]),
        ("every one of them", &names),
    ];

    for (case, deleted) in cases {
        let emulator = Emulator::new();
        for name in names {
            put_aggregator(
                &emulator,
                name,
                &json!({
                    "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
                }),
            )
            .await;
        }
        // A neighbouring resource of another type, to show the delete is scoped to the
        // aggregator: an aggregation authorization this account granted.
        emulator
            .ok(
                "PutAggregationAuthorization",
                &json!({
                    "AuthorizedAccountId": ACCOUNT_B,
                    "AuthorizedAwsRegion": "us-west-2",
                }),
            )
            .await;

        for name in deleted {
            delete_aggregator(&emulator, name).await;
        }

        let expected: Vec<String> = names
            .iter()
            .filter(|name| !deleted.contains(name))
            .map(|name| (*name).to_string())
            .collect();
        let actual = all_names(&emulator).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected only the undeleted aggregators to remain, got {actual:?}"
        );

        let authorizations = emulator
            .ok("DescribeAggregationAuthorizations", &json!({}))
            .await;
        let granted = authorizations["AggregationAuthorizations"].as_array();
        assert_eq!(
            granted.map(Vec::len),
            Some(1),
            "case {case:?}: deleting an aggregator must not touch the aggregation \
             authorizations, got {authorizations}"
        );
    }
}

#[tokio::test]
async fn deleting_an_aggregator_no_name_holds_is_refused() {
    let emulator = Emulator::new();
    put_aggregator(
        &emulator,
        "Existing-One",
        &json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])] }),
    )
    .await;
    delete_aggregator(&emulator, "Existing-One").await;
    put_aggregator(
        &emulator,
        "Existing-One",
        &json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])] }),
    )
    .await;
    put_aggregator(
        &emulator,
        "already-deleted",
        &json!({ "OrganizationAggregationSource": organization_source(&[]) }),
    )
    .await;
    delete_aggregator(&emulator, "already-deleted").await;

    // Rows differ along HOW the name misses: nothing ever created it, its aggregator
    // was already deleted, it differs from a stored name only in case, it is a prefix
    // or an extension of a stored name, and it sits at each published length bound.
    let cases: [(&str, String); 7] = [
        ("a name nothing created", "never-created".to_string()),
        (
            "a name whose aggregator was already deleted",
            "already-deleted".to_string(),
        ),
        (
            "a name differing only in case",
            "existing-one".to_string(),
        ),
        ("a prefix of a stored name", "Existing".to_string()),
        ("an extension of a stored name", "Existing-One-2".to_string()),
        ("the shortest allowed name", "x".repeat(NAME_MIN_LENGTH)),
        ("the longest allowed name", "y".repeat(NAME_MAX_LENGTH)),
    ];
    for (case, name) in cases {
        let (status, code) = emulator
            .refused(
                "DeleteConfigurationAggregator",
                &json!({ "ConfigurationAggregatorName": name }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: expected HTTP 400 NoSuchConfigurationAggregatorException, got \
             {status} {code}"
        );
    }

    // A name outside the modelled constraints is refused at the boundary instead,
    // before the handler can look it up: one past each published length bound, and a
    // name the `^[\w\-]+$` pattern does not admit.
    let malformed: [(&str, String); 4] = [
        ("shorter than the minimum", String::new()),
        (
            "longer than the maximum",
            "z".repeat(NAME_MAX_LENGTH + 1),
        ),
        ("a name with a space", "not a name".to_string()),
        ("a name with a slash", "not/a/name".to_string()),
    ];
    for (case, name) in malformed {
        let (status, code) = emulator
            .refused(
                "DeleteConfigurationAggregator",
                &json!({ "ConfigurationAggregatorName": name }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }

    // Every refusal left the one aggregator in state alone.
    let actual = all_names(&emulator).await;
    assert_eq!(
        actual,
        vec!["Existing-One".to_string()],
        "expected the refusals to change nothing, got {actual:?}"
    );
}

#[tokio::test]
async fn an_aggregator_recreated_after_a_delete_carries_a_fresh_identity() {
    // Rows differ along the Region the requests are signed for, which the aggregator's
    // ARN names, and along the name's length at each published bound.
    for region in ["us-east-1", "eu-west-2"] {
        for name in [
            "a".repeat(NAME_MIN_LENGTH),
            "recreated".to_string(),
            "b".repeat(NAME_MAX_LENGTH),
        ] {
            let case = format!("{region}/{} chars", name.len());
            let emulator = Emulator::in_region(region);
            let sources =
                json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &[region])] });

            let first = put_aggregator(&emulator, &name, &sources).await;
            delete_aggregator(&emulator, &name).await;
            let second = put_aggregator(&emulator, &name, &sources).await;

            assert_ne!(
                first, second,
                "case {case:?}: an aggregator created after a delete is a new resource, so it \
                 carries an id no deleted one did, got {second:?}"
            );
            let prefix = emulator_server::emulator_arn(
                "config",
                region,
                "config-aggregator/config-aggregator-",
            );
            assert!(
                second.starts_with(&prefix),
                "case {case:?}: expected the new ARN to start with {prefix:?}, got {second:?}"
            );
            let reported = all_names(&emulator).await;
            assert_eq!(
                reported,
                vec![name.clone()],
                "case {case:?}: expected exactly the recreated aggregator in state, got \
                 {reported:?}"
            );
        }
    }
}
