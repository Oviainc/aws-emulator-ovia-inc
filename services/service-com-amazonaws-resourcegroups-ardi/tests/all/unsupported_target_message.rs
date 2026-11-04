// SPDX-License-Identifier: CC0-1.0

//! The refusal for an operation a group's FORM does not support carries ONE
//! fixed sentence, whatever the request names.
//!
//! Six operations of this service depend on the form of the group they address:
//! `GetGroupConfiguration` and `PutGroupConfiguration` need a group that
//! contains a service configuration, `GetGroupQuery` and `UpdateGroupQuery` one
//! that is driven by a resource query, and `GroupResources` and
//! `UngroupResources` one whose configuration serves explicit membership. Each
//! answers `BadRequestException` (400) with the single sentence
//! `This operation does not support the target` for a group whose form does not
//! support it, and that sentence is entirely FIXED: no group name, no
//! identifier, no query type and no service-configuration type taken from the
//! request appears in it.
//!
//! The sibling `rejection_contract` suite crosses every group FORM with every
//! one of those operations. This suite varies what that cross product holds
//! still, since a message built from the request would only show up there:
//!
//! * the VALUES the request carries — the group's name (its length and its
//!   character class), the query type, and the service-configuration type;
//! * the FORM OF THE IDENTIFIER the group is addressed by — its bare name and
//!   its ARN, the two forms `GroupString` admits.
//!
//! Every refusal is judged to answer the one sentence, to carry no part of the
//! request, and to leave the addressed group exactly as it was; and the
//! operations each form DOES support are judged to be served through both
//! identifier forms, so the refusal is not a blanket one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// The message every operation answers for a group whose form does not support
/// it. Entirely fixed text.
const UNSUPPORTED_TARGET: &str = "This operation does not support the target";

/// The region every request here is signed (in scope only) for.
const HOME: &str = "us-east-1";

/// A resource ARN a static-membership call can carry.
const MEMBER: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

/// The two values the modelled `QueryType` declares.
///
/// Source: `smithy_query(query="shape",shape="QueryType")`.
const TAG_QUERY: &str = "TAG_FILTERS_1_0";
const STACK_QUERY: &str = "CLOUDFORMATION_STACK_1_0";

/// A service-configuration type a static-membership operation serves, named by
/// the `GroupResources` documentation (`smithy_query(query="doc",shape="GroupResources")`).
const HOST_MANAGEMENT: &str = "AWS::EC2::HostManagement";

/// Two service-configuration types OUTSIDE the list `GroupResources` documents,
/// each of them a value the modelled `GroupConfigurationType` admits (pattern
/// `^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$`, length 0..40, from
/// `smithy_query(query="shape",shape="GroupConfigurationType")`), so a group
/// configured with one holds a configuration that serves no explicit membership.
const FIREWALL_RULE_GROUP: &str = "AWS::NetworkFirewall::RuleGroup";
const REGISTRY_APPLICATION: &str = "AWS::AppRegistry::Application";

/// One row of the table: a group of some form, created with some set of values,
/// and which of the form-dependent operations its form supports.
struct Case {
    /// How this row is named in an assertion message.
    label: &'static str,
    /// The group's name. Differs per row in length and character class, so a
    /// message that interpolated it would differ per row.
    name: String,
    /// The `CreateGroup` request that gives the group its form.
    request: Value,
    /// The request values, besides the identifier, that a message built from the
    /// request could carry.
    values: Vec<String>,
    /// Whether the group is driven by a resource query.
    queried: bool,
    /// Whether the group contains a service configuration.
    configured: bool,
    /// Whether the group's configuration is one explicit membership is served for.
    static_membership: bool,
}

/// A group name of `length` characters drawn from the class the modelled
/// `GroupName` admits (`^[a-zA-Z0-9_\.-]{1,300}|…$`, length 1..300, from
/// `smithy_query(query="shape",shape="GroupName")`), ending in `suffix` so the
/// name is distinctive.
fn padded_name(length: usize, suffix: &str) -> String {
    let mut name: String = "z9_.-"
        .chars()
        .cycle()
        .take(length.saturating_sub(suffix.len()))
        .collect();
    name.push_str(suffix);
    name
}

/// The rows: the two query types and three service-configuration types, over
/// group names of the shortest distinctive, a mid, and the longest length the
/// model admits.
fn cases() -> Vec<Case> {
    let longest = padded_name(300, "-tag-queried");
    let mid = padded_name(64, ".stack-queried");
    vec![
        Case {
            label: "a tag-queried group named at the modelled maximum length",
            name: longest.clone(),
            request: json!({
                "Name": longest,
                "ResourceQuery": { "Type": TAG_QUERY, "Query": "{\"TagFilters\":[]}" }
            }),
            values: vec![longest, TAG_QUERY.to_string()],
            queried: true,
            configured: false,
            static_membership: false,
        },
        Case {
            label: "a CloudFormation-stack-queried group",
            name: mid.clone(),
            request: json!({
                "Name": mid,
                "ResourceQuery": {
                    "Type": STACK_QUERY,
                    "Query": "{\"StackIdentifier\":\"demo-stack\"}"
                }
            }),
            values: vec![mid, STACK_QUERY.to_string()],
            queried: true,
            configured: false,
            static_membership: false,
        },
        Case {
            label: "a group configured as a firewall rule group",
            name: "Firewall_Rule.Group-3".to_string(),
            request: json!({
                "Name": "Firewall_Rule.Group-3",
                "Configuration": [{ "Type": FIREWALL_RULE_GROUP }]
            }),
            values: vec![
                "Firewall_Rule.Group-3".to_string(),
                FIREWALL_RULE_GROUP.to_string(),
            ],
            queried: false,
            configured: true,
            static_membership: false,
        },
        Case {
            label: "a group configured as a registry application",
            name: "Registry.App_Group-4".to_string(),
            request: json!({
                "Name": "Registry.App_Group-4",
                "Configuration": [{ "Type": REGISTRY_APPLICATION }]
            }),
            values: vec![
                "Registry.App_Group-4".to_string(),
                REGISTRY_APPLICATION.to_string(),
            ],
            queried: false,
            configured: true,
            static_membership: false,
        },
        Case {
            label: "a host-management group, whose configuration does serve explicit membership",
            name: "Host.Managed_Group-5".to_string(),
            request: json!({
                "Name": "Host.Managed_Group-5",
                "Configuration": [{ "Type": HOST_MANAGEMENT }]
            }),
            values: vec![
                "Host.Managed_Group-5".to_string(),
                HOST_MANAGEMENT.to_string(),
            ],
            queried: false,
            configured: true,
            static_membership: true,
        },
        Case {
            label: "a group holding neither a query nor a configuration",
            name: "Formless.Group_6".to_string(),
            request: json!({ "Name": "Formless.Group_6" }),
            values: vec!["Formless.Group_6".to_string()],
            queried: false,
            configured: false,
            static_membership: false,
        },
    ]
}

/// The two forms `GroupString` admits for naming one group: its bare name and
/// its ARN (`smithy_query(query="shape",shape="GroupString")`).
fn identifiers(case: &Case) -> [(&'static str, String); 2] {
    [
        ("its name", case.name.clone()),
        (
            "its ARN",
            format!(
                "arn:aws:resource-groups:{HOME}:{ACCOUNT}:group/{}",
                case.name
            ),
        ),
    ]
}

/// Every form-dependent operation addressed at `identifier`: its name, its path,
/// its request, and whether THIS row's form supports it.
fn form_dependent_calls(case: &Case, identifier: &str) -> [(&'static str, &'static str, Value, bool); 6] {
    [
        (
            "GetGroupConfiguration",
            "/get-group-configuration",
            json!({ "Group": identifier }),
            case.configured,
        ),
        (
            "PutGroupConfiguration",
            "/put-group-configuration",
            json!({
                "Group": identifier,
                "Configuration": [{ "Type": HOST_MANAGEMENT }]
            }),
            // A group that is not queried takes a configuration, whether or not
            // it already holds one.
            !case.queried,
        ),
        (
            "GetGroupQuery",
            "/get-group-query",
            json!({ "Group": identifier }),
            case.queried,
        ),
        (
            "UpdateGroupQuery",
            "/update-group-query",
            json!({
                "Group": identifier,
                "ResourceQuery": { "Type": TAG_QUERY, "Query": "{\"TagFilters\":[]}" }
            }),
            // A group that contains no configuration takes a query, whether or
            // not it already holds one.
            !case.configured,
        ),
        (
            "GroupResources",
            "/group-resources",
            json!({ "Group": identifier, "ResourceArns": [MEMBER] }),
            case.static_membership,
        ),
        (
            "UngroupResources",
            "/ungroup-resources",
            json!({ "Group": identifier, "ResourceArns": [MEMBER] }),
            case.static_membership,
        ),
    ]
}

/// Every refusal of an operation a group's form does not support answers 400
/// `BadRequestException` with the one fixed sentence, carries no value the
/// request named, and leaves the group alone — over the table of request values
/// crossed with both forms of identifier.
///
/// The sentences answered across the whole cross product are collected and
/// judged to be ONE string: a message assembled from the request would give as
/// many as there are rows.
#[tokio::test]
async fn the_not_supported_target_refusal_is_one_fixed_sentence() {
    let mut sentences: BTreeSet<String> = BTreeSet::new();
    let mut refusals = 0_usize;
    for case in cases() {
        // Every refusal in this row is judged on one store, since a refused call
        // changes nothing a later caller sees — which the round trip below is
        // what proves.
        let emulator = Emulator::new();
        emulator.ok("/groups", &case.request).await;
        let before = emulator
            .ok("/get-group", &json!({ "Group": case.name }))
            .await;
        for (form, identifier) in identifiers(&case) {
            for (operation, path, body, supported) in form_dependent_calls(&case, &identifier) {
                if supported {
                    continue;
                }
                let label = format!("{operation} on {}, addressed by {form}", case.label);
                let (status, answered) = emulator.call(path, &body).await;
                assert_eq!(
                    (status, error_code(&answered)),
                    (400, "BadRequestException"),
                    "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
                );
                let message = answered["Message"].as_str().unwrap_or("<none>");
                assert_eq!(
                    message, UNSUPPORTED_TARGET,
                    "case {label:?}: expected the message {UNSUPPORTED_TARGET:?}, got {message:?}",
                );
                for value in case.values.iter().chain(std::iter::once(&identifier)) {
                    assert!(
                        !message.contains(value.as_str()),
                        "case {label:?}: expected the message to carry no part of the \
                         request, but it carries {value:?}: got {message:?}",
                    );
                }
                sentences.insert(message.to_string());
                refusals += 1;
            }
        }
        let after = emulator
            .ok("/get-group", &json!({ "Group": case.name }))
            .await;
        assert_eq!(
            after,
            before,
            "case {:?}: expected the refusals to leave the group alone, got {after} after \
             {before}",
            case.label,
        );
    }
    assert!(
        refusals > 0,
        "the table defines no refusal to judge, so nothing was asserted",
    );
    assert_eq!(
        sentences.iter().cloned().collect::<Vec<String>>(),
        vec![UNSUPPORTED_TARGET.to_string()],
        "expected one sentence across all {refusals} refusals, got {sentences:?}",
    );
}

/// The other side of the rule: an operation the addressed group's form DOES
/// support is served through either form of identifier, so the fixed sentence
/// above is a refusal of the TARGET and not of the operation.
///
/// Each pair is judged on its own store, since a served call can change the form
/// of the group it is applied to and the next pair must judge the form its row
/// declares.
#[tokio::test]
async fn an_operation_the_group_form_supports_is_served_by_either_identifier() {
    for case in cases() {
        for (form, identifier) in identifiers(&case) {
            for (operation, path, body, supported) in form_dependent_calls(&case, &identifier) {
                if !supported {
                    continue;
                }
                let label = format!("{operation} on {}, addressed by {form}", case.label);
                let emulator = Emulator::new();
                emulator.ok("/groups", &case.request).await;
                let (status, answered) = emulator.call(path, &body).await;
                assert!(
                    (200..300).contains(&status),
                    "case {label:?}: this group's form supports the operation, so expected \
                     a success, got {status} {answered}",
                );
            }
        }
    }
}
