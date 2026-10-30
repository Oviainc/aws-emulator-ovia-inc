// SPDX-License-Identifier: CC0-1.0

//! `ListGroupResources` `resource-type` filter validation, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rule under test, asserted over a TABLE of groups and filter values rather
//! than one row: a `resource-type` filter value must name a resource type the
//! addressed group's stored query admits, and a request naming any type the query
//! does not admit is `BadRequestException` (400) rather than a listing — except
//! for a query that names no type limit at all, where every value is answered with
//! a listing. A group's limit is what its query holds NOW, so an update to that
//! query moves the refusal with it, and a refusal changes nothing a later caller
//! sees.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, static_group, Emulator};

/// The `ResourceTypeFilters` value that names no limit: the model documents it as
/// making the query "match resources of any resource type that also match the
/// query", and documents the filter validation as not occurring for such a group.
///
/// Source: `smithy_query(query="doc",shape="ResourceQuery",member="Query")` and
/// `smithy_query(query="doc",shape="ListGroupResourcesInput",member="Filters")`.
const ALL_SUPPORTED: &str = "AWS::AllSupported";

/// The fixed part of the message the refusal carries, before the list of resource
/// types that failed the validation.
///
/// Source: the recorded refusal. The model quotes a message "similar to" `The
/// resource types specified as filters in the request are not valid.` and adds that
/// "the error includes a list of resource types that failed the validation because
/// they are not part of the query associated with the group"
/// (`smithy_query(query="doc",shape="ListGroupResourcesInput",member="Filters")`).
const INVALID_FILTER_TYPES_MESSAGE: &str = "Filters not valid: The resource types specified as \
     filters in the request are incompatible with the group query:";

/// One group the table judges filters against: the `CreateGroup` request that
/// stores it, and the resource types its stored query admits — `None` for a query
/// that names no limit, so no filter value is refused.
///
/// The rows differ in what the group's query says about resource types: one type,
/// several types, the element spelled as a single string rather than an array, a
/// stack query carrying the element, the element naming `AWS::AllSupported` alone
/// and beside a concrete type, a query document holding no such element at all,
/// and a group with no query — which is what the validation rule generalises over.
struct GroupCase {
    name: &'static str,
    request: fn() -> Value,
    /// The types the group's query admits, or `None` when it admits any type.
    limit: Option<&'static [&'static str]>,
}

fn group_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            name: "s3only",
            request: || {
                json!({
                    "Name": "s3only",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::S3::Bucket\"],\"TagFilters\":[]}"
                    }
                })
            },
            limit: Some(&["AWS::S3::Bucket"]),
        },
        GroupCase {
            name: "three-types",
            request: || {
                json!({
                    "Name": "three-types",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::EC2::Instance\",\"AWS::EC2::Volume\",\"AWS::DynamoDB::Table\"],\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
                    }
                })
            },
            limit: Some(&[
                "AWS::EC2::Instance",
                "AWS::EC2::Volume",
                "AWS::DynamoDB::Table",
            ]),
        },
        GroupCase {
            name: "one-string",
            request: || {
                json!({
                    "Name": "one-string",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":\"AWS::Lambda::Function\"}"
                    }
                })
            },
            limit: Some(&["AWS::Lambda::Function"]),
        },
        GroupCase {
            name: "stack-query",
            request: || {
                json!({
                    "Name": "stack-query",
                    "ResourceQuery": {
                        "Type": "CLOUDFORMATION_STACK_1_0",
                        "Query": "{\"StackIdentifier\":\"arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0\",\"ResourceTypeFilters\":[\"AWS::EC2::Instance\"]}"
                    }
                })
            },
            limit: Some(&["AWS::EC2::Instance"]),
        },
        GroupCase {
            name: "all-supported",
            request: || {
                json!({
                    "Name": "all-supported",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}"
                    }
                })
            },
            limit: None,
        },
        GroupCase {
            name: "all-supported-and-one",
            request: || {
                json!({
                    "Name": "all-supported-and-one",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\",\"AWS::S3::Bucket\"],\"TagFilters\":[]}"
                    }
                })
            },
            limit: None,
        },
        GroupCase {
            name: "no-type-element",
            request: || {
                json!({
                    "Name": "no-type-element",
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
                    }
                })
            },
            limit: None,
        },
        GroupCase {
            // A group created with a service configuration instead of a query: no
            // query is associated with it, so no filter value is judged.
            name: "no-query",
            request: || static_group("no-query"),
            limit: None,
        },
    ]
}

/// Resource types used as filter values. None is `AWS::AllSupported`, so each is
/// either admitted or refused by a given group purely by the group's limit.
const SOME_TYPES: [&str; 6] = [
    "AWS::S3::Bucket",
    "AWS::EC2::Instance",
    "AWS::EC2::Volume",
    "AWS::DynamoDB::Table",
    "AWS::Lambda::Function",
    "AWS::SQS::Queue",
];

/// An emulator holding every group of the table.
async fn populated() -> Emulator {
    let emulator = Emulator::new();
    for case in group_cases() {
        emulator.ok("/groups", &(case.request)()).await;
    }
    emulator
}

/// A `ListGroupResources` request for `group` carrying one `resource-type` filter.
fn one_filter(group: &str, values: &[&str]) -> Value {
    json!({
        "Group": group,
        "Filters": [{ "Name": "resource-type", "Values": values }]
    })
}

/// The `ResourceIdentifiers` ARNs of a `ListGroupResources` page.
fn listed_arns(page: &Value) -> Vec<String> {
    page["ResourceIdentifiers"]
        .as_array()
        .expect("ResourceIdentifiers is a list")
        .iter()
        .map(|id| id["ResourceArn"].as_str().expect("an ARN").to_string())
        .collect()
}

/// Every filter value a group's query admits is answered with a listing, and every
/// value it does not admit is refused — over every group of the table crossed with
/// every resource type, so each row is judged by its own query and not by the one
/// the table happens to list first.
#[tokio::test]
async fn a_filter_value_is_answered_exactly_when_the_group_query_admits_it() {
    let emulator = populated().await;
    for case in group_cases() {
        for value in SOME_TYPES {
            let admitted = case
                .limit
                .is_none_or(|limit| limit.contains(&value));
            let (status, body) = emulator.call("/list-group-resources", &one_filter(case.name, &[value])).await;
            if admitted {
                assert_eq!(
                    status, 200,
                    "case {}/{value}: the group's query admits this type, so expected a \
                     200 listing, got {status} {body}",
                    case.name,
                );
                assert!(
                    body["ResourceIdentifiers"].is_array() && body["Resources"].is_array(),
                    "case {}/{value}: expected a listing, got {body}",
                    case.name,
                );
            } else {
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "BadRequestException"),
                    "case {}/{value}: the group's query does not admit this type, so \
                     expected 400 BadRequestException, got {status} {body}",
                    case.name,
                );
            }
        }
    }
}

/// A request carrying no `resource-type` filter at all is answered with a listing
/// for every group, whatever its query says about resource types: there is nothing
/// to validate.
#[tokio::test]
async fn a_request_without_filters_is_always_a_listing() {
    let emulator = populated().await;
    for case in group_cases() {
        for body in [json!({ "Group": case.name }), json!({ "Group": case.name, "Filters": [] })] {
            let page = emulator.ok("/list-group-resources", &body).await;
            assert_eq!(
                listed_arns(&page),
                Vec::<String>::new(),
                "case {} {body}: expected an empty listing, got {page}",
                case.name,
            );
        }
    }
}

/// The refusal names every type the request asked for that the group's query does
/// not admit, once each and in request order, and names no admitted type — across
/// several filter entries, repeated values, and values that differ only in case.
#[tokio::test]
async fn the_refusal_reports_the_types_the_query_does_not_admit() {
    let emulator = populated().await;
    // (label, the whole Filters list, the types the refusal must name in order)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one unadmitted value",
            json!([{ "Name": "resource-type", "Values": ["AWS::EC2::Instance"] }]),
            vec!["AWS::EC2::Instance"],
        ),
        (
            "an admitted value beside an unadmitted one",
            json!([{ "Name": "resource-type", "Values": ["AWS::S3::Bucket", "AWS::SQS::Queue"] }]),
            vec!["AWS::SQS::Queue"],
        ),
        (
            "several unadmitted values in one entry",
            json!([{
                "Name": "resource-type",
                "Values": ["AWS::SQS::Queue", "AWS::EC2::Volume", "AWS::S3::Bucket"]
            }]),
            vec!["AWS::SQS::Queue", "AWS::EC2::Volume"],
        ),
        (
            "unadmitted values spread over two entries, one repeated",
            json!([
                { "Name": "resource-type", "Values": ["AWS::Lambda::Function"] },
                { "Name": "resource-type", "Values": ["AWS::SQS::Queue", "AWS::Lambda::Function"] }
            ]),
            vec!["AWS::Lambda::Function", "AWS::SQS::Queue"],
        ),
        (
            // Filter values are case-sensitive (the modelled `ResourceFilter$Values`),
            // so a type spelled differently is a type the query does not admit. The
            // spelling still has to satisfy the modelled `ResourceFilterValue`
            // pattern `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$`, which the boundary
            // enforces before this handler runs.
            "an admitted type spelled in another case",
            json!([{ "Name": "resource-type", "Values": ["AWS::S3::bucket"] }]),
            vec!["AWS::S3::bucket"],
        ),
    ];
    for (label, filters, expected) in cases {
        let (status, body) = emulator
            .call(
                "/list-group-resources",
                &json!({ "Group": "s3only", "Filters": filters }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {body}",
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(INVALID_FILTER_TYPES_MESSAGE),
            "case {label:?}: expected the documented message, got {message:?}",
        );
        // Each type that failed is named once, and in the order the request listed
        // them: the positions of the expected types rise through the message.
        let mut previous = 0usize;
        for wanted in &expected {
            let at = message.find(wanted).unwrap_or_else(|| {
                panic!("case {label:?}: expected the message to name {wanted}, got {message:?}")
            });
            assert!(
                at >= previous,
                "case {label:?}: expected {expected:?} in request order, got {message:?}",
            );
            assert_eq!(
                message.matches(wanted).count(),
                1,
                "case {label:?}: expected {wanted} to be named once, got {message:?}",
            );
            previous = at;
        }
        // An admitted type the request also asked for did not fail, so the report
        // does not name it.
        if !expected
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case("AWS::S3::Bucket"))
        {
            assert!(
                !message.contains("AWS::S3::Bucket"),
                "case {label:?}: the message must not name the admitted type, got {message:?}",
            );
        }
    }
}

/// A group's limit is the one its query holds NOW: updating the query moves which
/// filter values are refused, in both directions.
#[tokio::test]
async fn validation_follows_an_updated_query() {
    let emulator = Emulator::new();
    let name = "moving";
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": name,
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"ResourceTypeFilters\":[\"AWS::S3::Bucket\"],\"TagFilters\":[]}"
                }
            }),
        )
        .await;

    // (label, the query to store, the filter value, whether it must be answered)
    let steps: Vec<(&str, Option<&str>, &str, bool)> = vec![
        ("as created, an admitted type", None, "AWS::S3::Bucket", true),
        ("as created, another type", None, "AWS::EC2::Instance", false),
        (
            "after widening to the other type",
            Some("{\"ResourceTypeFilters\":[\"AWS::EC2::Instance\"],\"TagFilters\":[]}"),
            "AWS::EC2::Instance",
            true,
        ),
        (
            "after widening, the formerly admitted type",
            None,
            "AWS::S3::Bucket",
            false,
        ),
        (
            "after the query admits every type",
            Some("{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}"),
            "AWS::S3::Bucket",
            true,
        ),
        (
            "after the query admits every type, any other type",
            None,
            "AWS::SQS::Queue",
            true,
        ),
    ];
    for (label, update, value, answered) in steps {
        if let Some(query) = update {
            emulator
                .ok(
                    "/update-group-query",
                    &json!({
                        "Group": name,
                        "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": query }
                    }),
                )
                .await;
        }
        let (status, body) = emulator
            .call("/list-group-resources", &one_filter(name, &[value]))
            .await;
        if answered {
            assert_eq!(
                status, 200,
                "case {label:?} ({value}): expected a 200 listing, got {status} {body}",
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {label:?} ({value}): expected 400 BadRequestException, got {status} {body}",
            );
        }
    }
}

/// The validation judges the filter, not the store: the refusal is a read that
/// leaves the group and its query exactly as they were, and a group whose query
/// admits every type still selects its static memberships by their derived type.
#[tokio::test]
async fn a_refusal_changes_nothing_and_an_unlimited_group_still_filters_members() {
    let emulator = Emulator::new();
    // A group whose query limits it to one resource type. A query-populated group
    // records no static memberships (`GroupResources` serves only a configured
    // group — the sibling `group_resources_supported_configuration` suite), so the
    // listing side of the rule is exercised on the configured group below.
    let queried = "s3only";
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": queried,
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"ResourceTypeFilters\":[\"AWS::S3::Bucket\"],\"TagFilters\":[]}"
                }
            }),
        )
        .await;
    let stored_query = emulator
        .ok("/get-group-query", &json!({ "Group": queried }))
        .await;

    // Refusing twice refuses the same way: nothing about the group moved.
    for attempt in 1..=2 {
        let (status, body) = emulator
            .call(
                "/list-group-resources",
                &one_filter(queried, &["AWS::EC2::Instance"]),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case attempt {attempt}: expected 400 BadRequestException, got {status} {body}",
        );
    }
    assert_eq!(
        emulator
            .ok("/get-group-query", &json!({ "Group": queried }))
            .await,
        stored_query,
        "expected the refusals to leave the stored query alone",
    );
    let admitted = emulator
        .ok("/list-group-resources", &one_filter(queried, &["AWS::S3::Bucket"]))
        .await;
    assert_eq!(
        listed_arns(&admitted),
        Vec::<String>::new(),
        "expected the admitted filter to still be answered with a listing, got {admitted}",
    );

    // A group with no query admits every type, so each value is answered and
    // selects the memberships whose derived type it names.
    let configured = "members";
    emulator.ok("/groups", &static_group(configured)).await;
    let instance = "arn:aws:ec2:us-east-1:000000000000:instance/i-000000000000000a";
    let volume = "arn:aws:ec2:us-east-1:000000000000:volume/vol-000000000000000a";
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": configured, "ResourceArns": [instance, volume] }),
        )
        .await;
    for (value, expected) in [
        ("AWS::EC2::Instance", vec![instance]),
        ("AWS::EC2::Volume", vec![volume]),
        ("AWS::SQS::Queue", vec![]),
    ] {
        let page = emulator
            .ok("/list-group-resources", &one_filter(configured, &[value]))
            .await;
        let want: Vec<String> = expected.iter().map(|arn| (*arn).to_string()).collect();
        assert_eq!(
            listed_arns(&page),
            want,
            "case {value} on a group with no query: expected {want:?}, got {page}",
        );
    }
}

/// Every group of the table is reachable and the table is self-consistent: each row
/// declaring a limit names only types the `SOME_TYPES` catalog covers, so the cross
/// product above really exercises both sides of the rule for that row.
#[test]
fn the_table_covers_both_sides_of_the_rule_for_every_row() {
    for case in group_cases() {
        match case.limit {
            None => continue,
            Some(limit) => {
                assert!(
                    !limit.is_empty() && !limit.contains(&ALL_SUPPORTED),
                    "case {}: a row declaring a limit must name concrete types, got {limit:?}",
                    case.name,
                );
                assert!(
                    limit.iter().all(|t| SOME_TYPES.contains(t)),
                    "case {}: every declared type must be in the filter catalog so the \
                     cross product covers an admitted value, got {limit:?}",
                    case.name,
                );
                assert!(
                    SOME_TYPES.iter().any(|t| !limit.contains(t)),
                    "case {}: the catalog must carry a type this row does not admit",
                    case.name,
                );
            }
        }
    }
}
