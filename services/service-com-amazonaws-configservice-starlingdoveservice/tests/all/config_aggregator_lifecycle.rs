// SPDX-License-Identifier: CC0-1.0

//! The configuration-aggregator write-and-read loop, end to end over the wire.
//!
//! Four rules are pinned here, each over a TABLE of inputs rather than one case:
//!
//! 1. `PutConfigurationAggregator` persists the aggregator it was given and
//!    `DescribeConfigurationAggregators` reports it back, whatever form its single
//!    aggregation source takes (individual accounts or an organization, all Regions or
//!    a listed set, with or without aggregator filters);
//! 2. the aggregator carries the identity Config assigned it — an ARN naming the
//!    `config-aggregator` resource type, the Region the request was signed for and an
//!    id unique per aggregator — plus the creation and last-update times;
//! 3. a second put of the SAME name updates that aggregator in place: the sources the
//!    request carries replace the stored ones, and the id, the ARN and the creation
//!    time it was created with survive;
//! 4. a request whose aggregation sources cannot hold together is REFUSED with the
//!    modelled client error, and one naming an aggregator that does not exist is
//!    refused rather than answered with an empty page.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{Emulator, ROLE_ARN};

/// Twelve-digit source account ids, the form `AccountId`'s `^\d{12}$` pattern allows.
const ACCOUNT_A: &str = "111122223333";
/// See [`ACCOUNT_A`].
const ACCOUNT_B: &str = "444455556666";
/// See [`ACCOUNT_A`].
const ACCOUNT_C: &str = "777788889999";

/// The only published `AggregatorFilterType` value, from
/// `smithy_query(query="shape", shape="AggregatorFilterType")`: `INCLUDE`.
const FILTER_TYPE_INCLUDE: &str = "INCLUDE";

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

/// The `ConfigurationAggregator` a `PutConfigurationAggregator` response carries.
fn put_aggregator(response: &Value) -> &Value {
    let aggregator = &response["ConfigurationAggregator"];
    assert!(
        aggregator.is_object(),
        "PutConfigurationAggregator must return a ConfigurationAggregator, got {response}"
    );
    aggregator
}

/// The one aggregator `DescribeConfigurationAggregators` reports for `name`.
async fn described(emulator: &Emulator, name: &str) -> Value {
    let response = emulator
        .ok(
            "DescribeConfigurationAggregators",
            &json!({ "ConfigurationAggregatorNames": [name] }),
        )
        .await;
    let reported = response["ConfigurationAggregators"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name:?}: expected a list, got {response}"));
    assert_eq!(
        reported.len(),
        1,
        "case {name:?}: expected exactly the named aggregator, got {response}"
    );
    reported[0].clone()
}

/// Every aggregator `DescribeConfigurationAggregators` reports for `request`, with the
/// `NextToken` it answered.
async fn describe_page(emulator: &Emulator, request: &Value) -> (Vec<Value>, Option<String>) {
    let response = emulator
        .ok("DescribeConfigurationAggregators", request)
        .await;
    let page = response["ConfigurationAggregators"]
        .as_array()
        .unwrap_or_else(|| panic!("case {request}: expected a list, got {response}"))
        .clone();
    let next = response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (page, next)
}

/// `aggregator`'s name, as the service reports it.
fn name_of(aggregator: &Value) -> String {
    aggregator["ConfigurationAggregatorName"]
        .as_str()
        .unwrap_or_else(|| panic!("the aggregator must carry its name, got {aggregator}"))
        .to_string()
}

/// `aggregator`'s ARN, checked against the form the `ConfigurationAggregatorArn`
/// pattern allows — `arn:aws:config:<region>:<account>:config-aggregator/`
/// `config-aggregator-<[a-z\d]+>`, whose id segment the server assigns.
fn aggregator_arn_of(aggregator: &Value, region: &str, case: &str) -> String {
    let prefix =
        emulator_server::emulator_arn("config", region, "config-aggregator/config-aggregator-");
    let arn = aggregator["ConfigurationAggregatorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the aggregator must carry an arn, got {aggregator}"))
        .to_string();
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected the aggregator ARN to start with {prefix:?}, got {arn:?}"
    );
    let id = &arn[prefix.len()..];
    assert!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: expected the assigned aggregator id to be lowercase base-36, got {arn:?}"
    );
    arn
}

/// `key` of `aggregator` as a timestamp, which awsJson1_1 renders as epoch seconds.
fn time_of(aggregator: &Value, key: &str, case: &str) -> f64 {
    aggregator[key]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected {key} to be a timestamp, got {aggregator}"))
}

/// The aggregation source `member` as the service REPORTS it, given the one a request
/// carried: `AllAwsRegions` carries the modelled default of `false` in a read even
/// when the request left it out, so the expected form carries it too.
fn source_as_reported(member: &str, requested: Option<Value>) -> Option<Value> {
    let mut requested = requested?;
    if member == "AccountAggregationSources" || member == "OrganizationAggregationSource" {
        let sources: Vec<&mut Value> = match &mut requested {
            Value::Array(sources) => sources.iter_mut().collect(),
            source => vec![source],
        };
        for source in sources {
            if let Some(source) = source.as_object_mut() {
                source.entry("AllAwsRegions").or_insert(json!(false));
            }
        }
    }
    Some(requested)
}

#[tokio::test]
async fn an_aggregator_is_read_back_with_the_single_source_it_was_created_with() {
    // Rows differ along the FORM of the aggregation source: which kind it is, how many
    // accounts it names, whether it aggregates every Region or a listed set, and
    // whether the aggregator filters on a resource type or on a service principal.
    let cases: [(&str, Value); 7] = [
        (
            "one account, one region",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])] }),
        ),
        (
            "one account, three regions",
            json!({
                "AccountAggregationSources": [
                    account_source(&[ACCOUNT_A], &["us-east-1", "eu-west-2", "ap-south-1"]),
                ],
            }),
        ),
        (
            "three accounts, all regions",
            json!({
                "AccountAggregationSources": [
                    account_source(&[ACCOUNT_A, ACCOUNT_B, ACCOUNT_C], &[]),
                ],
            }),
        ),
        (
            "organization, two regions",
            json!({
                "OrganizationAggregationSource": organization_source(&["us-west-2", "eu-west-1"]),
            }),
        ),
        (
            "organization, all regions",
            json!({ "OrganizationAggregationSource": organization_source(&[]) }),
        ),
        (
            "accounts with a resource-type filter",
            json!({
                "AccountAggregationSources": [account_source(&[ACCOUNT_B], &["us-east-2"])],
                "AggregatorFilters": {
                    "ResourceType": {
                        "Type": FILTER_TYPE_INCLUDE,
                        "Value": ["AWS::EC2::Volume", "AWS::S3::Bucket"],
                    },
                },
            }),
        ),
        (
            "organization with a service-principal filter",
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
    ];

    let emulator = Emulator::new();
    for (index, (case, source)) in cases.iter().enumerate() {
        let name = format!("aggregator-{index}");
        let mut request = source.clone();
        request["ConfigurationAggregatorName"] = json!(name);
        request["Tags"] = json!([{ "Key": "team", "Value": "platform" }]);

        let created = emulator
            .ok("PutConfigurationAggregator", &request)
            .await;
        let created = put_aggregator(&created).clone();
        let read_back = described(&emulator, &name).await;

        assert_eq!(
            name_of(&created),
            name,
            "case {case:?}: expected the created aggregator to carry the requested name, \
             got {created}"
        );
        assert_eq!(
            read_back, created,
            "case {case:?}: expected the describe to report what the put returned, got \
             {read_back:?}"
        );
        for member in [
            "AccountAggregationSources",
            "OrganizationAggregationSource",
            "AggregatorFilters",
        ] {
            let expected = source_as_reported(member, source.get(member).cloned());
            let actual = read_back.get(member).cloned();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} to be reported as it was persisted, got \
                 {actual:?}"
            );
        }
        // Tags are metadata on the aggregator, not part of the aggregator itself: the
        // modelled `ConfigurationAggregator` carries no `Tags` member, so none is
        // reported.
        assert!(
            read_back.get("Tags").is_none(),
            "case {case:?}: the reported aggregator models no Tags member, got {read_back}"
        );
    }
}

#[tokio::test]
async fn the_aggregator_arn_names_the_region_and_an_id_unique_per_aggregator() {
    // Rows differ along the Region the request is signed for; ARNs are also compared
    // ACROSS aggregators in each Region, because an assigned id must be unique.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let emulator = Emulator::in_region(region);
        let mut arns: Vec<String> = Vec::new();
        for index in 0..3 {
            let name = format!("arn-case-{index}");
            let created = emulator
                .ok(
                    "PutConfigurationAggregator",
                    &json!({
                        "ConfigurationAggregatorName": name,
                        "AccountAggregationSources": [account_source(&[ACCOUNT_A], &[region])],
                    }),
                )
                .await;
            let created = put_aggregator(&created).clone();
            let case = format!("{region}/{name}");
            let arn = aggregator_arn_of(&created, region, &case);

            let created_at = time_of(&created, "CreationTime", &case);
            let updated_at = time_of(&created, "LastUpdatedTime", &case);
            assert_eq!(
                created_at, updated_at,
                "case {case:?}: a freshly created aggregator was last updated when it was \
                 created, got {created_at} and {updated_at}"
            );
            assert!(
                created_at > 0.0,
                "case {case:?}: expected a wall-clock creation time, got {created_at}"
            );

            assert!(
                !arns.contains(&arn),
                "case {case:?}: expected an id no other aggregator carries, got {arn:?} \
                 among {arns:?}"
            );
            arns.push(arn);
        }
    }
}

#[tokio::test]
async fn putting_an_aggregator_that_exists_updates_it_in_place() {
    // Rows differ along WHAT the second put changes: the accounts, the Region set, the
    // kind of source, the filters, and the tags Config refuses to update.
    let first = json!({
        "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
        "Tags": [{ "Key": "team", "Value": "platform" }],
    });
    let cases: [(&str, Value); 5] = [
        (
            "more accounts",
            json!({
                "AccountAggregationSources": [
                    account_source(&[ACCOUNT_A, ACCOUNT_B], &["us-east-1"]),
                ],
            }),
        ),
        (
            "fewer accounts",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_C], &["us-east-1"])] }),
        ),
        (
            "every region instead of a listed one",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &[])] }),
        ),
        (
            "an organization instead of accounts",
            json!({ "OrganizationAggregationSource": organization_source(&["us-east-1"]) }),
        ),
        (
            "filters added and different tags",
            json!({
                "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": ["AWS::IAM::Role"] },
                },
                "Tags": [{ "Key": "team", "Value": "security" }],
            }),
        ),
    ];

    for (index, (case, update)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("update-case-{index}");

        let mut create = first.clone();
        create["ConfigurationAggregatorName"] = json!(name);
        let created = emulator.ok("PutConfigurationAggregator", &create).await;
        let created = put_aggregator(&created).clone();

        let mut second = update.clone();
        second["ConfigurationAggregatorName"] = json!(name);
        let updated = emulator.ok("PutConfigurationAggregator", &second).await;
        let updated = put_aggregator(&updated).clone();

        assert_eq!(
            updated["ConfigurationAggregatorArn"], created["ConfigurationAggregatorArn"],
            "case {case:?}: an update keeps the ARN the aggregator was created with, got \
             {updated}"
        );
        assert_eq!(
            time_of(&updated, "CreationTime", case),
            time_of(&created, "CreationTime", case),
            "case {case:?}: an update keeps the creation time, got {updated}"
        );
        assert!(
            time_of(&updated, "LastUpdatedTime", case) >= time_of(&created, "CreationTime", case),
            "case {case:?}: an update is not older than the creation it updates, got {updated}"
        );

        // The sources the second request carried REPLACED the stored ones, and the
        // describe reports the replacement, not the original.
        let read_back = described(&emulator, &name).await;
        assert_eq!(
            read_back, updated,
            "case {case:?}: expected the describe to report the updated aggregator, got \
             {read_back}"
        );
        for member in [
            "AccountAggregationSources",
            "OrganizationAggregationSource",
            "AggregatorFilters",
        ] {
            let expected = source_as_reported(member, update.get(member).cloned());
            let actual = read_back.get(member).cloned();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} to be replaced by the second put, got \
                 {actual:?}"
            );
        }

        // One aggregator, not two: a repeat put is idempotent on the resource.
        let (all, _) = describe_page(&emulator, &json!({})).await;
        assert_eq!(
            all.len(),
            1,
            "case {case:?}: expected the repeat put to update the one aggregator, got {all:?}"
        );
    }
}

#[tokio::test]
async fn a_request_whose_aggregation_sources_cannot_hold_together_is_refused() {
    // Rows differ along WHICH part of the request cannot hold: the source exclusivity,
    // the Regions a source aggregates, the organization role, the aggregator filters,
    // and the modelled constraints on the name, an account id and a tag.
    let cases: [(&str, Value, u16, &str); 11] = [
        (
            "both kinds of source",
            json!({
                "ConfigurationAggregatorName": "both-sources",
                "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
                "OrganizationAggregationSource": organization_source(&["us-east-1"]),
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "neither kind of source",
            json!({ "ConfigurationAggregatorName": "no-source" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an empty account source list",
            json!({
                "ConfigurationAggregatorName": "empty-source-list",
                "AccountAggregationSources": [],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "accounts aggregating no region at all",
            json!({
                "ConfigurationAggregatorName": "no-regions",
                "AccountAggregationSources": [{ "AccountIds": [ACCOUNT_A] }],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "accounts aggregating both all and listed regions",
            json!({
                "ConfigurationAggregatorName": "both-region-forms",
                "AccountAggregationSources": [{
                    "AccountIds": [ACCOUNT_A],
                    "AllAwsRegions": true,
                    "AwsRegions": ["us-east-1"],
                }],
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an organization aggregating no region at all",
            json!({
                "ConfigurationAggregatorName": "org-no-regions",
                "OrganizationAggregationSource": { "RoleArn": ROLE_ARN },
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an organization aggregating both all and listed regions",
            json!({
                "ConfigurationAggregatorName": "org-both-region-forms",
                "OrganizationAggregationSource": {
                    "RoleArn": ROLE_ARN,
                    "AllAwsRegions": true,
                    "AwsRegions": ["us-east-1"],
                },
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "an organization with an empty role ARN",
            json!({
                "ConfigurationAggregatorName": "org-empty-role",
                "OrganizationAggregationSource": { "RoleArn": "", "AllAwsRegions": true },
            }),
            400,
            "InvalidRoleException",
        ),
        (
            "filters naming neither a resource type nor a service principal",
            json!({
                "ConfigurationAggregatorName": "empty-filters",
                "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
                "AggregatorFilters": {},
            }),
            400,
            "InvalidParameterValueException",
        ),
        // The modelled constraints: `AccountId`'s `^\d{12}$` pattern and
        // `ConfigurationAggregatorName`'s `^[\w\-]+$`, refused at the boundary.
        (
            "an account id that is not twelve digits",
            json!({
                "ConfigurationAggregatorName": "bad-account",
                "AccountAggregationSources": [account_source(&["12345"], &["us-east-1"])],
            }),
            400,
            "ValidationException",
        ),
        (
            "a name outside the modelled pattern",
            json!({
                "ConfigurationAggregatorName": "not a valid name",
                "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
            }),
            400,
            "ValidationException",
        ),
    ];

    let emulator = Emulator::new();
    for (case, request, expected_status, expected_code) in cases {
        let (status, code) = emulator
            .refused("PutConfigurationAggregator", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {code}"
        );
    }

    // Nothing was persisted by any refusal.
    let (all, _) = describe_page(&emulator, &json!({})).await;
    assert!(
        all.is_empty(),
        "expected a refused put to persist nothing, got {all:?}"
    );
}

#[tokio::test]
async fn describe_reports_the_aggregators_in_state_a_page_at_a_time() {
    let emulator = Emulator::new();
    let names = ["alpha", "bravo", "charlie", "delta", "echo"];
    for name in names {
        emulator
            .ok(
                "PutConfigurationAggregator",
                &json!({
                    "ConfigurationAggregatorName": name,
                    "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])],
                }),
            )
            .await;
    }

    // Rows differ along the page size asked for: the modelled `Limit` default of 0
    // ("the default is maximum"), each small size, and one past the number in state.
    let cases: [(&str, i64, &[usize]); 5] = [
        ("the default page size", 0, &[5]),
        ("one per page", 1, &[1, 1, 1, 1, 1]),
        ("two per page", 2, &[2, 2, 1]),
        ("exactly what is in state", 5, &[5]),
        ("more than is in state", 6, &[5]),
    ];
    for (case, limit, expected_pages) in cases {
        let mut next: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        for (index, expected_len) in expected_pages.iter().enumerate() {
            let mut request = json!({ "Limit": limit });
            if let Some(token) = next.as_deref() {
                request["NextToken"] = json!(token);
            }
            let (page, token) = describe_page(&emulator, &request).await;
            assert_eq!(
                page.len(),
                *expected_len,
                "case {case:?}: expected {expected_len} aggregators on page {index}, got \
                 {page:?}"
            );
            seen.extend(page.iter().map(name_of));
            next = token;
            let last_page = index + 1 == expected_pages.len();
            assert_eq!(
                next.is_none(),
                last_page,
                "case {case:?}: page {index} carried NextToken {next:?}"
            );
        }
        assert_eq!(
            seen,
            names.map(str::to_string).to_vec(),
            "case {case:?}: expected every aggregator exactly once in name order, got {seen:?}"
        );
    }

    // A request that names aggregators reports exactly those, in name order.
    let selections: [(&str, &[&str]); 3] = [
        ("one name", &["charlie"]),
        ("two names", &["echo", "alpha"]),
        ("every name", &names),
    ];
    for (case, requested) in selections {
        let (page, _) = describe_page(
            &emulator,
            &json!({ "ConfigurationAggregatorNames": requested }),
        )
        .await;
        let mut expected = requested.iter().map(|name| (*name).to_string()).collect::<Vec<_>>();
        expected.sort();
        let actual = page.iter().map(name_of).collect::<Vec<_>>();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected exactly the named aggregators, got {actual:?}"
        );
    }

    // A name no aggregator carries, and a token this operation never issued, are
    // refused rather than answered with a page.
    let refusals: [(&str, Value, &str); 3] = [
        (
            "an unknown name",
            json!({ "ConfigurationAggregatorNames": ["foxtrot"] }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "an unknown name beside a known one",
            json!({ "ConfigurationAggregatorNames": ["alpha", "foxtrot"] }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "a token this operation never issued",
            json!({ "NextToken": "not-a-token-!!" }),
            "InvalidNextTokenException",
        ),
    ];
    for (case, request, expected_code) in refusals {
        let (status, code) = emulator
            .refused("DescribeConfigurationAggregators", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
    }
}
