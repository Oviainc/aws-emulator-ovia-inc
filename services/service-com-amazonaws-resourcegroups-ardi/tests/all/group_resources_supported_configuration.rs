// SPDX-License-Identifier: CC0-1.0

//! Which groups `GroupResources` serves, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of stored group configurations
//! rather than one row: a static-membership operation serves only a group whose
//! stored service configuration declares one of the configuration types its
//! documentation lists, and refuses a call against any other group with
//! `BadRequestException` (400), recording no membership.
//!
//! A refused call must also leave the store exactly as it was, so each row reads
//! the addressed group's memberships back through `ListGroupResources`.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator};

/// Every group configuration type `GroupResources` is documented as usable with.
///
/// Source: `smithy_query(query="doc",shape="GroupResources")` on this service's
/// model — "You can only use this operation with the following groups:
/// `AWS::EC2::HostManagement`, `AWS::EC2::CapacityReservationPool`,
/// `AWS::ResourceGroups::ApplicationGroup`. Other resource group types and
/// resource types are not currently supported by this operation."
const SUPPORTED_CONFIGURATION_TYPES: [&str; 3] = [
    "AWS::EC2::HostManagement",
    "AWS::EC2::CapacityReservationPool",
    "AWS::ResourceGroups::ApplicationGroup",
];

/// The resources a row groups: two ARNs of different services, so a served row
/// records more than one membership.
const MEMBERS: [&str; 2] = [
    "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
    "arn:aws:dynamodb:us-east-1:000000000000:table/Orders",
];

/// One row: how the group was created, and whether `GroupResources` serves it.
struct GroupCase {
    label: String,
    /// The whole `CreateGroup` request, bar the name this row's group is given.
    create: Value,
    served: bool,
}

/// Every group form the rule is asserted over: each documented configuration type
/// on its own, one carrying parameters, one beside a configuration item of another
/// type, and — on the refusing side — a configuration type the documentation does
/// not list, a near-miss spelling of one that does, an empty configuration, no
/// configuration at all, and each kind of query-populated group.
///
/// The configuration item types outside the catalog above
/// (`AWS::ResourceGroups::Generic`) and the parameter names are AWS
/// service-configuration vocabulary from the "Service configurations for resource
/// groups" reference, which is what `GroupConfigurationItem$Type` and
/// `GroupConfigurationParameter$Name` are documented against; the query types are
/// the modelled `QueryType` values
/// (`smithy_query(query="shape",shape="QueryType")`).
fn group_cases() -> Vec<GroupCase> {
    let mut cases: Vec<GroupCase> = Vec::new();
    for supported in SUPPORTED_CONFIGURATION_TYPES {
        cases.push(GroupCase {
            label: format!("the configuration type {supported} on its own"),
            create: json!({ "Configuration": [{ "Type": supported }] }),
            served: true,
        });
        cases.push(GroupCase {
            label: format!("{supported} beside a configuration item of another type"),
            create: json!({
                "Configuration": [
                    { "Type": "AWS::ResourceGroups::Generic" },
                    { "Type": supported }
                ]
            }),
            served: true,
        });
    }
    cases.push(GroupCase {
        label: "a supported configuration type carrying parameters".to_string(),
        create: json!({
            "Configuration": [{
                "Type": "AWS::EC2::HostManagement",
                "Parameters": [{ "Name": "auto-allocate-host", "Values": ["true"] }]
            }]
        }),
        served: true,
    });
    cases.push(GroupCase {
        label: "a configuration type the documentation does not list".to_string(),
        create: json!({ "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }] }),
        served: false,
    });
    cases.push(GroupCase {
        label: "two configuration types, neither of them listed".to_string(),
        create: json!({
            "Configuration": [
                { "Type": "AWS::ResourceGroups::Generic" },
                { "Type": "AWS::NetworkFirewall::RuleGroup" }
            ]
        }),
        served: false,
    });
    cases.push(GroupCase {
        // The modelled `GroupConfigurationType` pattern
        // `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$` admits one spelling of each type, so
        // a differently-cased near miss is a different type and not a listed one.
        label: "a near-miss spelling of a listed configuration type".to_string(),
        create: json!({ "Configuration": [{ "Type": "AWS::EC2::Hostmanagement" }] }),
        served: false,
    });
    cases.push(GroupCase {
        label: "an empty configuration list, which configures nothing".to_string(),
        create: json!({ "Configuration": [] }),
        served: false,
    });
    cases.push(GroupCase {
        label: "no configuration and no query at all".to_string(),
        create: json!({}),
        served: false,
    });
    cases.push(GroupCase {
        label: "a tag-filter query group, populated from its query".to_string(),
        create: json!({
            "ResourceQuery": {
                "Type": "TAG_FILTERS_1_0",
                "Query": "{\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
            }
        }),
        served: false,
    });
    cases.push(GroupCase {
        label: "a CloudFormation-stack query group, populated from its query".to_string(),
        create: json!({
            "ResourceQuery": {
                "Type": "CLOUDFORMATION_STACK_1_0",
                "Query": "{\"StackIdentifier\":\"arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0\"}"
            }
        }),
        served: false,
    });
    cases
}

/// The ARNs `ListGroupResources` reports as the group's memberships, sorted.
async fn listed_members(emulator: &Emulator, case: &str, name: &str) -> Vec<String> {
    let listed = emulator
        .ok("/list-group-resources", &json!({ "Group": name }))
        .await;
    let mut arns: Vec<String> = listed["ResourceIdentifiers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {listed}"))
        .iter()
        .map(|identifier| {
            identifier["ResourceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceArn, got {identifier}"))
                .to_string()
        })
        .collect();
    arns.sort();
    arns
}

/// The rule: `GroupResources` records a static membership only on a group whose
/// stored configuration declares a documented configuration type, and refuses
/// every other group with `BadRequestException` (400) while recording nothing.
#[tokio::test]
async fn group_resources_serves_only_a_group_with_a_documented_configuration_type() {
    for case in group_cases() {
        let label = case.label.as_str();
        let name = "membership-probe";
        let emulator = Emulator::new();
        let mut create = case.create.clone();
        create["Name"] = json!(name);
        emulator.ok("/groups", &create).await;

        let (status, answered) = emulator
            .call(
                "/group-resources",
                &json!({ "Group": name, "ResourceArns": MEMBERS }),
            )
            .await;

        let mut expected: Vec<String> = MEMBERS.iter().map(|m| (*m).to_string()).collect();
        expected.sort();
        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {answered}",
            );
            let mut succeeded: Vec<String> = answered["Succeeded"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: expected Succeeded, got {answered}"))
                .iter()
                .map(|arn| {
                    arn.as_str()
                        .unwrap_or_else(|| panic!("case {label:?}: expected an ARN, got {arn}"))
                        .to_string()
                })
                .collect();
            succeeded.sort();
            assert_eq!(
                succeeded, expected,
                "case {label:?}: expected every ARN in Succeeded, got {answered}",
            );
            // Round trip: the memberships the call reported are the ones a read
            // of the group now answers.
            let stored = listed_members(&emulator, label, name).await;
            assert_eq!(
                stored, expected,
                "case {label:?}: expected the memberships {expected:?} to be persisted, \
                 got {stored:?}",
            );
        } else {
            assert_eq!(
                (status, error_code(&answered)),
                (400, "BadRequestException"),
                "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
            );
            // A refused call records nothing: the group holds no membership.
            let stored = listed_members(&emulator, label, name).await;
            assert!(
                stored.is_empty(),
                "case {label:?}: a refused call must record no membership, got {stored:?}",
            );
        }
    }
}

/// The refusal is decided from the group a request addresses, however it is
/// addressed, and it is decided per group: a served group of the same store still
/// records its memberships, and a group that was refused stays refusable while a
/// served one stays served.
#[tokio::test]
async fn the_refusal_follows_the_addressed_group_and_not_the_identifier_form() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": "served",
                "Configuration": [{ "Type": "AWS::ResourceGroups::ApplicationGroup" }]
            }),
        )
        .await;
    let refused_arn = emulator
        .ok(
            "/groups",
            &json!({
                "Name": "refused",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{\"TagFilters\":[]}" }
            }),
        )
        .await["Group"]["GroupArn"]
        .as_str()
        .expect("CreateGroup answers a GroupArn")
        .to_string();

    // Every identifier form this operation accepts addresses the same refused
    // group: a bare name, and the group's own stored ARN.
    for (label, identifier) in [
        ("the group name", json!("refused")),
        ("the group ARN", json!(refused_arn)),
    ] {
        let (status, answered) = emulator
            .call(
                "/group-resources",
                &json!({ "Group": identifier, "ResourceArns": [MEMBERS[0]] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
        );
    }

    // The served group of the same store records the same resource, twice over:
    // the refusal above is a property of the group, not of the store or the call
    // order.
    for round in ["first", "second"] {
        let grouped = emulator
            .ok(
                "/group-resources",
                &json!({ "Group": "served", "ResourceArns": [MEMBERS[0]] }),
            )
            .await;
        assert_eq!(
            grouped["Succeeded"].as_array().map(Vec::len),
            Some(1),
            "case {round:?}: expected the served group to record the membership, got {grouped}",
        );
    }
    let stored = listed_members(&emulator, "served", "served").await;
    assert_eq!(
        stored,
        vec![MEMBERS[0].to_string()],
        "expected the served group to hold exactly its one membership, got {stored:?}",
    );
    let refused_members = listed_members(&emulator, "refused", "refused").await;
    assert!(
        refused_members.is_empty(),
        "expected the refused group to hold no membership, got {refused_members:?}",
    );
}

/// A group that does not exist is `NotFoundException` (404) whatever its
/// configuration would have been: the group is resolved before this operation
/// decides whether it serves it, so the refusal never masks a missing group.
#[tokio::test]
async fn a_group_that_does_not_exist_is_still_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "present", "Configuration": [{ "Type": "AWS::EC2::HostManagement" }] }),
        )
        .await;
    for (label, identifier) in [
        ("a name nothing answers to", "absent"),
        (
            "an ARN for a group that was never created",
            "arn:aws:resource-groups:us-east-1:000000000000:group/absent",
        ),
    ] {
        let (status, answered) = emulator
            .call(
                "/group-resources",
                &json!({ "Group": identifier, "ResourceArns": [MEMBERS[0]] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "NotFoundException"),
            "case {label:?}: expected 404 NotFoundException, got {status} {answered}",
        );
    }
}
