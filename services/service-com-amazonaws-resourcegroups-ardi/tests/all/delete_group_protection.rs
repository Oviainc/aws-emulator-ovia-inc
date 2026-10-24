// SPDX-License-Identifier: CC0-1.0

//! `DeleteGroup` against the deletion protection a group's own stored service
//! configuration can declare, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of configurations crossed with a
//! table of membership counts rather than one row: a group whose stored service
//! configuration sets the `deletion-protection` parameter to `UNLESS_EMPTY` cannot
//! be deleted while it still contains resources — the delete is refused with
//! `BadRequestException` (400) and the group keeps its memberships, its tags and
//! its configuration — while a group that holds nothing, or whose configuration
//! does not set that parameter to that value, deletes and is gone.

use serde_json::{json, Value};

use super::group_tags::stored_tags;
use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// The resources a case groups, as many as its row asks for.
///
/// Three distinct ARNs of two different services, so a case with more than one
/// member is not one ARN repeated.
const MEMBERS: [&str; 3] = [
    "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
    "arn:aws:ec2:us-east-1:000000000000:volume/vol-0123456789abcdef0",
    "arn:aws:dynamodb:us-east-1:000000000000:table/Orders",
];

/// The tags every case creates its group with, so a refused delete can be shown
/// to have destroyed none of them.
fn tags() -> Value {
    json!({ "env": "probe", "team": "platform" })
}

/// The same pairs as [`tags`], in the ordered form the shared `stored_tags`
/// helper reports, so the two compare.
fn tag_pairs() -> std::collections::BTreeMap<String, String> {
    [("env", "probe"), ("team", "platform")]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// One row of the configuration table: the `Configuration` a group is created
/// with, and whether that configuration protects the group from deletion while it
/// contains resources.
struct ProtectionCase {
    label: &'static str,
    /// The `Configuration` member of the create request, or `Value::Null` for a
    /// create that sends none at all.
    configuration: Value,
    protects: bool,
}

/// The configurations a group can carry, spanning the ways the protecting
/// parameter can and cannot be set.
///
/// The configuration item `Type` values, the parameter names and the
/// `deletion-protection` values (`UNLESS_EMPTY`, `DISABLED`) are AWS
/// service-configuration vocabulary from the "Service configurations for resource
/// groups" reference (<https://docs.aws.amazon.com/ARG/latest/APIReference/about-slg.html>),
/// which is what `GroupConfigurationItem$Type` and
/// `GroupConfigurationParameter$Name` are documented against; the list's 0..2
/// length and the parameter name pattern `^[a-z-]+$` come from the model shapes
/// `GroupConfigurationList` and `GroupConfigurationParameterName`.
fn protection_cases() -> Vec<ProtectionCase> {
    vec![
        ProtectionCase {
            label: "the protecting value on the generic item",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
            }]),
            protects: true,
        },
        ProtectionCase {
            // The parameter is read wherever the configuration sets it, not only
            // on one item type.
            label: "the protecting value on an application-group item",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::ApplicationGroup",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
            }]),
            protects: true,
        },
        ProtectionCase {
            label: "the protecting value on the second of two items, beside another parameter",
            configuration: json!([
                {
                    "Type": "AWS::EC2::HostManagement",
                    "Parameters": [{ "Name": "auto-allocate-host", "Values": ["true"] }]
                },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [
                        { "Name": "allowed-resource-types", "Values": ["AWS::EC2::Host"] },
                        { "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }
                    ]
                }
            ]),
            protects: true,
        },
        ProtectionCase {
            label: "the parameter carrying several values, one of them the protecting one",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [
                    { "Name": "deletion-protection", "Values": ["DISABLED", "UNLESS_EMPTY"] }
                ]
            }]),
            protects: true,
        },
        ProtectionCase {
            label: "the parameter explicitly disabled",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["DISABLED"] }]
            }]),
            protects: false,
        },
        ProtectionCase {
            label: "a value the parameter does not define, which protects nothing",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["unless_empty"] }]
            }]),
            protects: false,
        },
        ProtectionCase {
            label: "the parameter absent, another parameter set",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [
                    { "Name": "allowed-resource-types", "Values": ["AWS::EC2::Instance"] }
                ]
            }]),
            protects: false,
        },
        ProtectionCase {
            label: "a configuration item carrying no parameters at all",
            configuration: json!([{ "Type": "AWS::ResourceGroups::Generic" }]),
            protects: false,
        },
        ProtectionCase {
            label: "an empty configuration list, which configures nothing",
            configuration: json!([]),
            protects: false,
        },
        ProtectionCase {
            label: "no configuration member at all",
            configuration: Value::Null,
            protects: false,
        },
    ]
}

/// Create the case's group, name it `name`, and group its first `members`
/// resources. Returns the created `Group` structure.
async fn create_group(emulator: &Emulator, case: &ProtectionCase, name: &str, members: usize) -> Value {
    let mut request = json!({
        "Name": name,
        "Description": "Probe deletion protection",
        "Tags": tags(),
    });
    if !case.configuration.is_null() {
        request["Configuration"] = case.configuration.clone();
    }
    let created = emulator.ok("/groups", &request).await;
    if members > 0 {
        emulator
            .ok(
                "/group-resources",
                &json!({ "Group": name, "ResourceArns": MEMBERS[..members] }),
            )
            .await;
    }
    created["Group"].clone()
}

/// The ARNs `ListGroupResources` reports as the group's memberships.
fn listed_member_arns(case: &str, listed: &Value) -> Vec<String> {
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

/// The rule: a group is deletable unless its own stored service configuration
/// protects it AND it still contains resources. Every configuration is crossed
/// with an empty group, a group with one member and a group with three.
#[tokio::test]
async fn deletion_protection_refuses_a_delete_only_while_the_group_holds_resources() {
    for configuration in protection_cases() {
        for members in [0usize, 1, MEMBERS.len()] {
            let case = format!("{} with {members} member(s)", configuration.label);
            let name = "protection-probe";
            let emulator = Emulator::new();
            let created = create_group(&emulator, &configuration, name, members).await;
            let arn = created["GroupArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateGroup returned no GroupArn"))
                .to_string();

            let refused = configuration.protects && members > 0;
            let (status, body) = emulator.call("/delete-group", &json!({ "Group": name })).await;

            if refused {
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "BadRequestException"),
                    "case {case:?}: expected DeleteGroup to answer 400 BadRequestException, \
                     got {status} {body}",
                );
                // A refused delete destroys nothing: the group, its memberships,
                // its tags and its configuration are all still there.
                let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
                assert_eq!(
                    read["Group"], created,
                    "case {case:?}: expected the group to survive as {created}, got {}",
                    read["Group"],
                );
                let listed = emulator
                    .ok("/list-group-resources", &json!({ "Group": name }))
                    .await;
                let mut expected: Vec<String> =
                    MEMBERS[..members].iter().map(|m| (*m).to_string()).collect();
                expected.sort();
                assert_eq!(
                    listed_member_arns(&case, &listed),
                    expected,
                    "case {case:?}: expected the memberships to survive as {expected:?}, \
                     got {listed}",
                );
                let surviving_tags = stored_tags(&emulator, "us-east-1", &case, &arn).await;
                assert_eq!(
                    surviving_tags,
                    tag_pairs(),
                    "case {case:?}: expected the tags to survive as {:?}, got {surviving_tags:?}",
                    tag_pairs(),
                );
                let configured = emulator
                    .ok("/get-group-configuration", &json!({ "Group": name }))
                    .await;
                assert_eq!(
                    configured["GroupConfiguration"]["Configuration"], configuration.configuration,
                    "case {case:?}: expected the configuration to survive as {}, got {}",
                    configuration.configuration, configured["GroupConfiguration"],
                );
                // And the delete stays refused while the group is unchanged, so
                // the refusal is a property of the state, not of the first call.
                let (again, body) = emulator.call("/delete-group", &json!({ "Group": name })).await;
                assert_eq!(
                    (again, error_code(&body)),
                    (400, "BadRequestException"),
                    "case {case:?}: expected a repeated DeleteGroup to answer 400 \
                     BadRequestException, got {again} {body}",
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DeleteGroup to answer 200, got {status} {body}",
                );
                assert_eq!(
                    body["Group"], created,
                    "case {case:?}: expected DeleteGroup to report the deleted group {created}, \
                     got {}",
                    body["Group"],
                );
                let (status, read) = emulator.call("/get-group", &json!({ "Group": name })).await;
                assert_eq!(
                    (status, error_code(&read)),
                    (404, "NotFoundException"),
                    "case {case:?}: expected the deleted group to be gone, got {status} {read}",
                );
            }
        }
    }
}

/// The protection is decided from the group a request addresses, however it is
/// addressed, and it protects that group alone: an unprotected group of the same
/// store still deletes.
#[tokio::test]
async fn protection_follows_the_group_and_not_the_identifier_form() {
    let protecting = json!([{
        "Type": "AWS::ResourceGroups::Generic",
        "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
    }]);
    let emulator = Emulator::new();
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "guarded", "Configuration": protecting }),
        )
        .await;
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "guarded", "ResourceArns": [MEMBERS[0]] }),
        )
        .await;
    // An unprotected group holding the same resource, in the same store.
    emulator.ok("/groups", &json!({ "Name": "open" })).await;
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "open", "ResourceArns": [MEMBERS[0]] }),
        )
        .await;

    let guarded_arn = format!("arn:aws:resource-groups:us-east-1:{ACCOUNT}:group/guarded");
    // Every identifier form `DeleteGroup` accepts addresses the same protected group.
    for (label, request) in [
        ("the group name in Group", json!({ "Group": "guarded" })),
        ("the deprecated GroupName", json!({ "GroupName": "guarded" })),
        ("the group ARN", json!({ "Group": guarded_arn })),
    ] {
        let (status, body) = emulator.call("/delete-group", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {body}",
        );
    }
    let read = emulator.ok("/get-group", &json!({ "Group": "guarded" })).await;
    assert_eq!(
        read["Group"]["Name"].as_str(),
        Some("guarded"),
        "the protected group must still read back, got {read}",
    );

    // The other group is not protected, so it deletes with its member in place.
    let deleted = emulator.ok("/delete-group", &json!({ "Group": "open" })).await;
    assert_eq!(
        deleted["Group"]["Name"].as_str(),
        Some("open"),
        "an unprotected group deletes while holding resources, got {deleted}",
    );
}
