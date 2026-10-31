// SPDX-License-Identifier: CC0-1.0

//! Which RESOURCES a static-membership operation admits into a group, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a group's own service configuration decides which resource types its members
//!   may have — a configuration that sets the `allowed-resource-types` parameter
//!   admits a resource only when the type its ARN names is one of that parameter's
//!   values, and a configuration that sets the parameter nowhere admits any
//!   resource — so a static-membership call is answered HTTP 200 with each ARN the
//!   group does not support reported in `Failed`, under the fixed `ErrorCode`
//!   `ResourceArnValidationException` and `ErrorMessage` `Unsupported resource
//!   ARN`, recording and dropping no membership for it, while the rest of the same
//!   request is applied;
//! * a membership the group already holds is still removable whatever the
//!   configuration in effect by then admits, since replacing a configuration
//!   carries the memberships nowhere;
//! * the two query-evaluating listings always carry their `QueryErrors` member, as
//!   the empty list when the call raised no query error and as the error the call
//!   raised when it did;
//!
//! The sibling `group_resources_supported_configuration` suite asserts the other,
//! REQUEST-level rule: which GROUPS these operations serve at all.

use serde_json::{json, Value};

use super::put_group_configuration::make_update_due;
use super::resource_group_lifecycle::Emulator;
use super::ungroup_resources::{listed_members, sorted, succeeded};

/// The `FailedResource$ErrorCode` a static-membership call reports for a resource
/// ARN the addressed group does not support. Fixed text for the class.
const UNSUPPORTED_CODE: &str = "ResourceArnValidationException";

/// The `FailedResource$ErrorMessage` reported beside [`UNSUPPORTED_CODE`]. Fixed
/// text for the class: the entry's own `ResourceArn` names the resource.
const UNSUPPORTED_MESSAGE: &str = "Unsupported resource ARN";

/// The service-configuration parameter that names the resource types a group's
/// members may have — "the types of resources that you can add to this group" in
/// the AWS "Service configurations for resource groups" reference, which is the
/// vocabulary `GroupConfigurationParameter$Name` is documented against.
pub(crate) const ALLOWED_RESOURCE_TYPES: &str = "allowed-resource-types";

/// One resource: its ARN, and the CloudFormation-style resource type that ARN
/// names.
///
/// The ARN forms are the ones the modelled `ResourceArn` pattern admits
/// (`type/id`, a bare id, a region-less and account-less global resource); the
/// type names are from the AWS CloudFormation resource and property types
/// reference, which is the vocabulary `ResourceIdentifier$ResourceType` is
/// documented in ("such as `AWS::EC2::Instance`"). The last row is a resource
/// whose type this emulator has no name for, so no parameter value can name it.
const RESOURCES: [(&str, &str); 7] = [
    (
        "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
        "AWS::EC2::Instance",
    ),
    (
        "arn:aws:ec2:us-east-1:000000000000:dedicated-host/h-0123456789abcdef0",
        "AWS::EC2::Host",
    ),
    (
        "arn:aws:ec2:us-east-1:000000000000:capacity-reservation/cr-0123456789abcdef0",
        "AWS::EC2::CapacityReservation",
    ),
    (
        "arn:aws:sqs:us-east-1:000000000000:type-probe-queue",
        "AWS::SQS::Queue",
    ),
    ("arn:aws:s3:::type-probe-bucket", "AWS::S3::Bucket"),
    (
        "arn:aws:dynamodb:us-east-1:000000000000:table/Orders",
        "AWS::DynamoDB::Table",
    ),
    (
        "arn:aws:glacier:us-east-1:000000000000:vaults/records",
        "<no type this emulator names>",
    ),
];

/// One row of the configuration table: the `Configuration` a group is created
/// with, and the resource types that configuration admits as members.
struct GroupCase {
    label: &'static str,
    configuration: Value,
    /// The types the row's configuration admits, or `None` when it declares no
    /// limit at all and therefore admits every resource.
    admitted: Option<&'static [&'static str]>,
}

/// Every configuration the rule is asserted over. The rows differ along WHERE the
/// parameter is set (on the generic item beside another item, on the item that
/// makes the group an application group), how MANY types it names (one, three),
/// whether a named type is one any resource of the table has, the SPELLING of a
/// named type, and whether the parameter is set at all.
///
/// The configuration item types are the three `GroupResources` documents
/// (`smithy_query(query="doc",shape="GroupResources")`), since a group outside
/// that catalog serves no static membership whatever it admits; the parameter
/// names and the `deletion-protection` values are AWS service-configuration
/// vocabulary from the "Service configurations for resource groups" reference
/// (<https://docs.aws.amazon.com/ARG/latest/APIReference/about-slg.html>).
fn group_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            label: "a host-management group whose generic item names one type",
            configuration: json!([
                { "Type": "AWS::EC2::HostManagement" },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [
                        { "Name": ALLOWED_RESOURCE_TYPES, "Values": ["AWS::EC2::Host"] }
                    ]
                }
            ]),
            admitted: Some(&["AWS::EC2::Host"]),
        },
        GroupCase {
            label: "a capacity-reservation pool naming its one type, beside another parameter",
            configuration: json!([
                { "Type": "AWS::EC2::CapacityReservationPool" },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [
                        {
                            "Name": ALLOWED_RESOURCE_TYPES,
                            "Values": ["AWS::EC2::CapacityReservation"]
                        },
                        { "Name": "deletion-protection", "Values": ["DISABLED"] }
                    ]
                }
            ]),
            admitted: Some(&["AWS::EC2::CapacityReservation"]),
        },
        GroupCase {
            label: "an application group naming three types on the item itself",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::ApplicationGroup",
                "Parameters": [{
                    "Name": ALLOWED_RESOURCE_TYPES,
                    "Values": ["AWS::SQS::Queue", "AWS::S3::Bucket", "AWS::DynamoDB::Table"]
                }]
            }]),
            admitted: Some(&[
                "AWS::SQS::Queue",
                "AWS::S3::Bucket",
                "AWS::DynamoDB::Table",
            ]),
        },
        GroupCase {
            // A type no resource of the table has: the limit admits nothing here,
            // which is the same rule and not a special case of it.
            label: "a parameter naming a type none of the resources has",
            configuration: json!([{
                "Type": "AWS::EC2::HostManagement",
                "Parameters": [
                    { "Name": ALLOWED_RESOURCE_TYPES, "Values": ["AWS::EC2::Subnet"] }
                ]
            }]),
            admitted: Some(&["AWS::EC2::Subnet"]),
        },
        GroupCase {
            // `ResourceIdentifier$ResourceType` is patterned `^AWS::[a-zA-Z0-9]+::\w+$`,
            // so a type name has one spelling and a differently-cased value names
            // no type at all.
            label: "a parameter naming a near-miss spelling of a type",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::ApplicationGroup",
                "Parameters": [
                    { "Name": ALLOWED_RESOURCE_TYPES, "Values": ["aws::ec2::host"] }
                ]
            }]),
            admitted: Some(&["aws::ec2::host"]),
        },
        GroupCase {
            label: "a configuration that sets no parameter at all",
            configuration: json!([{ "Type": "AWS::EC2::HostManagement" }]),
            admitted: None,
        },
        GroupCase {
            label: "a configuration that sets another parameter, but not this one",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::ApplicationGroup",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["DISABLED"] }]
            }]),
            admitted: None,
        },
    ]
}

/// The `Failed` reports of an answer as `(ResourceArn, ErrorCode, ErrorMessage)`,
/// in the order the answer carries them.
fn failed_reports(case: &str, answered: &Value) -> Vec<(String, String, String)> {
    let reports = answered["Failed"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Failed list, got {answered}"));
    reports
        .iter()
        .map(|report| {
            let field = |name: &str| {
                report[name]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a {name} on every Failed entry, got {report}")
                    })
                    .to_string()
            };
            (field("ResourceArn"), field("ErrorCode"), field("ErrorMessage"))
        })
        .collect()
}

/// The one `Failed` report expected for `resource_arn`.
fn unsupported(resource_arn: &str) -> (String, String, String) {
    (
        resource_arn.to_string(),
        UNSUPPORTED_CODE.to_string(),
        UNSUPPORTED_MESSAGE.to_string(),
    )
}

/// Create `name` with `configuration`.
async fn created_with(emulator: &Emulator, name: &str, configuration: &Value) {
    emulator
        .ok(
            "/groups",
            &json!({ "Name": name, "Configuration": configuration }),
        )
        .await;
}

/// The rule: a group admits a resource only when its stored service configuration
/// admits the type the resource's ARN names — every configuration of the table
/// crossed with every resource of it.
#[tokio::test]
async fn group_resources_admits_only_the_resource_types_the_configuration_names() {
    for group in group_cases() {
        for (resource_arn, resource_type) in RESOURCES {
            let case = format!("{} grouping a {resource_type}", group.label);
            let name = "type-probe";
            let emulator = Emulator::new();
            created_with(&emulator, name, &group.configuration).await;

            let (status, answered) = emulator
                .call(
                    "/group-resources",
                    &json!({ "Group": name, "ResourceArns": [resource_arn] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a request naming a resource the group may not hold is \
                 still answered, got {status} {answered}",
            );
            let admits = group
                .admitted
                .is_none_or(|types| types.contains(&resource_type));
            let expected_members = if admits {
                vec![resource_arn.to_string()]
            } else {
                Vec::new()
            };
            assert_eq!(
                succeeded(&case, &answered),
                expected_members,
                "case {case:?}: expected Succeeded to be {expected_members:?}, got {answered}",
            );
            let expected_failed = if admits {
                Vec::new()
            } else {
                vec![unsupported(resource_arn)]
            };
            assert_eq!(
                failed_reports(&case, &answered),
                expected_failed,
                "case {case:?}: expected Failed to be {expected_failed:?}, got {answered}",
            );
            assert_eq!(
                answered["Pending"],
                json!([]),
                "case {case:?}: nothing is left half-added, got {answered}",
            );
            // Round trip: what the group now holds is read back through an
            // implemented listing, so a report of a refused resource is backed by
            // the state it left — no membership at all.
            let stored = listed_members(&emulator, &case, name).await;
            assert_eq!(
                stored, expected_members,
                "case {case:?}: expected the group to hold {expected_members:?}, got {stored:?}",
            );
        }
    }
}

/// One request is applied to the resources the group supports and reports the
/// rest: the per-resource reports are not a refusal of the whole call. An ARN the
/// request repeats is reported once, by whichever list it belongs in.
#[tokio::test]
async fn a_request_is_applied_to_the_resources_the_group_supports_and_reports_the_rest() {
    let emulator = Emulator::new();
    let name = "mixed";
    created_with(
        &emulator,
        name,
        &json!([{
            "Type": "AWS::EC2::CapacityReservationPool",
            "Parameters": [{
                "Name": ALLOWED_RESOURCE_TYPES,
                "Values": ["AWS::EC2::Host", "AWS::EC2::CapacityReservation"]
            }]
        }]),
    )
    .await;

    let host = RESOURCES[1].0;
    let reservation = RESOURCES[2].0;
    let queue = RESOURCES[3].0;
    let bucket = RESOURCES[4].0;
    let case = "a request mixing supported and unsupported resources";
    let answered = emulator
        .ok(
            "/group-resources",
            &json!({
                "Group": name,
                "ResourceArns": [queue, host, bucket, queue, reservation, host]
            }),
        )
        .await;
    assert_eq!(
        succeeded(case, &answered),
        sorted([host, reservation]),
        "case {case:?}: expected the supported resources in Succeeded, got {answered}",
    );
    assert_eq!(
        failed_reports(case, &answered),
        vec![unsupported(queue), unsupported(bucket)],
        "case {case:?}: expected one report per unsupported resource, in request order, \
         got {answered}",
    );
    let stored = listed_members(&emulator, case, name).await;
    assert_eq!(
        stored,
        sorted([host, reservation]),
        "case {case:?}: expected only the supported resources to be recorded, got {stored:?}",
    );

    // The removal side of the same request: the supported resources are dropped
    // and the unsupported ones are reported again, so the two directions agree.
    let answered = emulator
        .ok(
            "/ungroup-resources",
            &json!({
                "Group": name,
                "ResourceArns": [bucket, reservation, queue, host]
            }),
        )
        .await;
    assert_eq!(
        succeeded(case, &answered),
        sorted([host, reservation]),
        "case {case:?}: expected the memberships this call dropped in Succeeded, got {answered}",
    );
    assert_eq!(
        failed_reports(case, &answered),
        vec![unsupported(bucket), unsupported(queue)],
        "case {case:?}: expected one report per unsupported resource, in request order, \
         got {answered}",
    );
    let stored = listed_members(&emulator, case, name).await;
    assert!(
        stored.is_empty(),
        "case {case:?}: expected the group to hold nothing, got {stored:?}",
    );
}

/// A membership recorded while the configuration admitted it stays removable after
/// a put narrows what the group admits: the configuration decides what may JOIN,
/// and a membership already recorded is one a removal must still be able to drop.
#[tokio::test]
async fn a_membership_recorded_before_the_configuration_narrowed_is_still_removable() {
    let emulator = Emulator::new();
    let name = "narrowed";
    let host = RESOURCES[1].0;
    let queue = RESOURCES[3].0;
    created_with(
        &emulator,
        name,
        &json!([{
            "Type": "AWS::ResourceGroups::ApplicationGroup",
            "Parameters": [{
                "Name": ALLOWED_RESOURCE_TYPES,
                "Values": ["AWS::EC2::Host", "AWS::SQS::Queue"]
            }]
        }]),
    )
    .await;
    let case = "a configuration narrowed after the memberships were recorded";
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": name, "ResourceArns": [host, queue] }),
        )
        .await;

    let (status, answered) = emulator
        .call(
            "/put-group-configuration",
            &json!({
                "Group": name,
                "Configuration": [{
                    "Type": "AWS::ResourceGroups::ApplicationGroup",
                    "Parameters": [
                        { "Name": ALLOWED_RESOURCE_TYPES, "Values": ["AWS::EC2::Host"] }
                    ]
                }]
            }),
        )
        .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected the put to be accepted with 202, got {status} {answered}",
    );
    // The update falls due a span after the put, and the stored instant is what the
    // handlers read: moving it reaches the state a caller reaches by waiting.
    make_update_due(&emulator, name);

    // The narrowed configuration refuses the queue as a NEW membership...
    let refused = emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "narrowed", "ResourceArns": [RESOURCES[5].0] }),
        )
        .await;
    assert_eq!(
        failed_reports(case, &refused),
        vec![unsupported(RESOURCES[5].0)],
        "case {case:?}: expected the narrowed configuration to refuse a type it no longer \
         names, got {refused}",
    );
    // ...and still lets the membership it already holds be dropped.
    let answered = emulator
        .ok(
            "/ungroup-resources",
            &json!({ "Group": name, "ResourceArns": [queue] }),
        )
        .await;
    assert_eq!(
        succeeded(case, &answered),
        vec![queue.to_string()],
        "case {case:?}: expected the stored membership to be dropped, got {answered}",
    );
    assert_eq!(
        failed_reports(case, &answered),
        Vec::new(),
        "case {case:?}: a membership the group holds is not an unsupported resource, \
         got {answered}",
    );
    let stored = listed_members(&emulator, case, name).await;
    assert_eq!(
        stored,
        vec![host.to_string()],
        "case {case:?}: expected the other membership to be left, got {stored:?}",
    );
    // Once it is gone it is an ARN the group neither holds nor supports, so the
    // repeated removal reports it rather than reporting a removal.
    let answered = emulator
        .ok(
            "/ungroup-resources",
            &json!({ "Group": name, "ResourceArns": [queue] }),
        )
        .await;
    assert_eq!(
        (succeeded(case, &answered), failed_reports(case, &answered)),
        (Vec::new(), vec![unsupported(queue)]),
        "case {case:?}: expected the repeated removal to report the resource as \
         unsupported, got {answered}",
    );
}

/// `QueryErrors` is always on the wire: a call that raised no query error carries
/// it as the empty list rather than leaving the member out, and a call that raised
/// one carries that error — asserted over a table of groups and of searched
/// queries.
#[tokio::test]
async fn the_query_evaluating_listings_always_carry_their_query_errors() {
    let emulator = Emulator::new();
    let host = RESOURCES[1].0;
    // The groups a listing is asked for: one holding a membership, one holding
    // none, and one populated from each kind of resource query.
    created_with(&emulator, "holding", &json!([{ "Type": "AWS::EC2::HostManagement" }])).await;
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "holding", "ResourceArns": [host] }),
        )
        .await;
    created_with(&emulator, "empty", &json!([{ "Type": "AWS::EC2::HostManagement" }])).await;
    let stack = "arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0";
    for (name, document) in [
        (
            "tag-queried",
            json!({
                "Type": "TAG_FILTERS_1_0",
                "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}"
            }),
        ),
        (
            "stack-queried",
            json!({
                "Type": "CLOUDFORMATION_STACK_1_0",
                "Query": format!("{{\"StackIdentifier\":\"{stack}\"}}")
            }),
        ),
    ] {
        emulator
            .ok("/groups", &json!({ "Name": name, "ResourceQuery": document }))
            .await;
    }

    // (the group, the QueryErrors its listing must carry) — a group that runs no
    // query, and one whose query raises nothing, carry the empty list; a stack
    // query names a stack this service records none of, so it carries that error.
    // What the listing REPORTS for each kind of query is the sibling
    // `list_group_resources_query_membership` suite's own rule.
    for (name, expected) in [
        ("holding", json!([])),
        ("empty", json!([])),
        ("tag-queried", json!([])),
        (
            "stack-queried",
            json!([{
                "ErrorCode": "CLOUDFORMATION_STACK_NOT_EXISTING",
                "Message": format!("Stack with id {stack} does not exist")
            }]),
        ),
    ] {
        let listed = emulator
            .ok("/list-group-resources", &json!({ "Group": name }))
            .await;
        assert_eq!(
            listed["QueryErrors"],
            expected,
            "case {name:?}: expected ListGroupResources to carry QueryErrors as \
             {expected}, got {listed}",
        );
    }

    // A search evaluates the query the request carries, so the rows here are
    // queries: one matching nothing, one matching by type, and one matching every
    // supported type. A query whose evaluation DOES raise an error is the sibling
    // `search_resources` suite's own rule.
    for (case, query) in [
        (
            "a tag query nothing matches",
            "{\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Absent\"]}]}",
        ),
        (
            "a query limited to one resource type",
            "{\"ResourceTypeFilters\":[\"AWS::EC2::Host\"],\"TagFilters\":[]}",
        ),
        (
            "a query over every supported type",
            "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}",
        ),
    ] {
        let searched = emulator
            .ok(
                "/resources/search",
                &json!({ "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": query } }),
            )
            .await;
        assert_eq!(
            searched["QueryErrors"],
            json!([]),
            "case {case:?}: expected SearchResources to carry QueryErrors as [], \
             got {searched}",
        );
    }
}
