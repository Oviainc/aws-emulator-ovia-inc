// SPDX-License-Identifier: CC0-1.0

//! The service-configuration projection every operation that reports one answers
//! with, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of configuration documents whose
//! rows differ along the number of items, which item declares parameters, and how many
//! parameters and values each one carries:
//!
//! * a reported configuration document carries every item it holds with the parameters
//!   attached to that item, and restates those parameters flattened across the items in
//!   configuration order (`GroupParameters`), so the flat list and the items it is read
//!   off can never disagree; a document that sets no parameter reports none;
//! * an operation that reports ONE group's configuration also reports which group the
//!   configuration belongs to — the group's name and its stored ARN — whether the
//!   request addressed the group by name or by ARN;
//! * applying a configuration answers with that same projection, computed from the
//!   state the call has just written, rather than with an empty document.

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{
    ALLOWED_RESOURCE_TYPES_PARAMETER, CONFIGURATION_IN_EFFECT, CONFIGURATION_UPDATING,
    DELETION_PROTECTION_PARAMETER, DELETION_PROTECTION_UNLESS_EMPTY,
};

use super::put_group_configuration::make_update_due;
use super::resource_group_lifecycle::Emulator;

/// One row: a `Configuration` document, and the parameters a response that reports it
/// must restate beside the items.
struct ConfigurationCase {
    label: &'static str,
    /// The `Configuration` list, exactly as a request carries it.
    configuration: Value,
    /// The flattened parameter list the projection reports, or `Value::Null` for a
    /// document whose items set no parameter at all.
    group_parameters: Value,
}

/// Every configuration document the rules are asserted over.
///
/// The configuration item types and the parameter names are AWS service-configuration
/// vocabulary from the "Service configurations for resource groups" reference, which is
/// what `GroupConfigurationItem$Type` and `GroupConfigurationParameter$Name` are
/// documented against; two of the names are the ones this service already decides rules
/// by (`ALLOWED_RESOURCE_TYPES_PARAMETER`, `DELETION_PROTECTION_PARAMETER`), and
/// `allowed-host-families` is a third from the same reference. A document holds at most
/// two items (`smithy_query(query="shape",shape="GroupConfigurationList")`).
fn configuration_cases() -> Vec<ConfigurationCase> {
    let allowed = ALLOWED_RESOURCE_TYPES_PARAMETER;
    let protection = DELETION_PROTECTION_PARAMETER;
    let unless_empty = DELETION_PROTECTION_UNLESS_EMPTY;
    vec![
        ConfigurationCase {
            label: "one item, no parameters at all",
            configuration: json!([{ "Type": "AWS::EC2::CapacityReservationPool" }]),
            group_parameters: Value::Null,
        },
        ConfigurationCase {
            label: "one item, one parameter with one value",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": protection, "Values": [unless_empty] }]
            }]),
            group_parameters: json!([{ "Name": protection, "Values": [unless_empty] }]),
        },
        ConfigurationCase {
            label: "two items, the parameters on the second",
            configuration: json!([
                { "Type": "AWS::EC2::CapacityReservationPool" },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [
                        { "Name": allowed, "Values": ["AWS::EC2::CapacityReservation"] }
                    ]
                }
            ]),
            group_parameters: json!([
                { "Name": allowed, "Values": ["AWS::EC2::CapacityReservation"] }
            ]),
        },
        ConfigurationCase {
            label: "two items, the parameters on the first, two values",
            configuration: json!([
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [{
                        "Name": allowed,
                        "Values": ["AWS::EC2::CapacityReservation", "AWS::EC2::Instance"]
                    }]
                },
                { "Type": "AWS::EC2::CapacityReservationPool" }
            ]),
            group_parameters: json!([{
                "Name": allowed,
                "Values": ["AWS::EC2::CapacityReservation", "AWS::EC2::Instance"]
            }]),
        },
        ConfigurationCase {
            label: "one item carrying two parameters",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [
                    { "Name": allowed, "Values": ["AWS::EC2::CapacityReservation"] },
                    { "Name": protection, "Values": [unless_empty] }
                ]
            }]),
            group_parameters: json!([
                { "Name": allowed, "Values": ["AWS::EC2::CapacityReservation"] },
                { "Name": protection, "Values": [unless_empty] }
            ]),
        },
        ConfigurationCase {
            label: "both items carrying parameters, flattened in configuration order",
            configuration: json!([
                {
                    "Type": "AWS::EC2::HostManagement",
                    "Parameters": [{ "Name": "allowed-host-families", "Values": ["c5", "m5"] }]
                },
                {
                    "Type": "AWS::ResourceGroups::Generic",
                    "Parameters": [{ "Name": protection, "Values": [unless_empty] }]
                }
            ]),
            group_parameters: json!([
                { "Name": "allowed-host-families", "Values": ["c5", "m5"] },
                { "Name": protection, "Values": [unless_empty] }
            ]),
        },
        ConfigurationCase {
            label: "a parameter that names no value",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": allowed }]
            }]),
            group_parameters: json!([{ "Name": allowed }]),
        },
        ConfigurationCase {
            label: "a parameter whose value list is empty",
            configuration: json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": allowed, "Values": [] }]
            }]),
            group_parameters: json!([{ "Name": allowed, "Values": [] }]),
        },
    ]
}

/// The `GroupIdentifier` a configuration projection reports for the group `name`
/// stored under `arn`.
fn identifier(name: &str, arn: &str) -> Value {
    json!({ "GroupName": name, "GroupArn": arn })
}

/// The `GroupParameters` member of a reported configuration document, read as the
/// absent list it is when the document reports none.
fn group_parameters(configuration: &Value) -> Value {
    configuration
        .get("GroupParameters")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The group a response created, and the ARN it was stored under.
async fn create(emulator: &Emulator, request: &Value) -> (Value, String) {
    let created = emulator.ok("/groups", request).await;
    let arn = created["Group"]["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a created group reports its ARN, got {created}"))
        .to_string();
    (created, arn)
}

/// A create echoes the whole configuration document — every item with its own
/// parameters, and the parameters of the document flattened beside them — and a read
/// of that group reports the same document plus the identity of the group it belongs
/// to, by name and by ARN alike.
#[tokio::test]
async fn a_reported_configuration_carries_its_parameters_and_the_group_it_belongs_to() {
    for case in configuration_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let name = "configuration-projection";
        let (created, arn) = create(
            &emulator,
            &json!({ "Name": name, "Configuration": case.configuration }),
        )
        .await;
        let echoed = created["GroupConfiguration"].clone();
        assert_eq!(
            echoed["Configuration"], case.configuration,
            "case {label:?}: expected the create to echo every configuration item with its \
             own parameters, got {echoed}",
        );
        assert_eq!(
            group_parameters(&echoed),
            case.group_parameters,
            "case {label:?}: expected GroupParameters {}, got {echoed}",
            case.group_parameters,
        );
        assert_eq!(
            echoed["Status"],
            json!(CONFIGURATION_IN_EFFECT),
            "case {label:?}: the configuration a create stores is the one in effect, got \
             {echoed}",
        );

        // The same document, and which group it is, on every read of it — whichever
        // form the request addressed the group in.
        for (addressed, identifying) in [("by name", name), ("by ARN", arn.as_str())] {
            let read = emulator
                .ok("/get-group-configuration", &json!({ "Group": identifying }))
                .await;
            assert_eq!(
                read["GroupIdentifier"],
                identifier(name, &arn),
                "case {label:?} ({addressed}): expected the read to report the group the \
                 configuration belongs to, got {read}",
            );
            assert_eq!(
                read["GroupConfiguration"], echoed,
                "case {label:?} ({addressed}): expected the read to report the document the \
                 create reported, got {read}",
            );
        }
    }
}

/// A put answers with the projection it has just written: which group took the
/// configuration, and the configuration now reported for that group — the accepted
/// document as the proposed one while the update is in flight, and the one in effect,
/// with its parameters flattened beside it, once the clock owes the update.
#[tokio::test]
async fn applying_a_configuration_answers_with_the_projection_it_wrote() {
    for case in configuration_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let name = "configuration-applied";
        // A group created with neither a configuration nor a query: it holds none in
        // effect until this row's put attaches one.
        let (_, arn) = create(&emulator, &json!({ "Name": name })).await;

        let (status, answered) = emulator
            .call(
                "/put-group-configuration",
                &json!({ "Group": name, "Configuration": case.configuration }),
            )
            .await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected an accepted put to answer 202, got {status} {answered}",
        );
        assert_eq!(
            answered["GroupIdentifier"],
            identifier(name, &arn),
            "case {label:?}: expected the put to report the group it configured, got \
             {answered}",
        );
        let applied = answered["GroupConfiguration"].clone();
        assert_eq!(
            applied["ProposedConfiguration"], case.configuration,
            "case {label:?}: expected the put to report the configuration it accepted as the \
             proposed one, got {answered}",
        );
        assert_eq!(
            applied["Status"],
            json!(CONFIGURATION_UPDATING),
            "case {label:?}: expected the put to report the update as in flight, got \
             {answered}",
        );
        assert_eq!(
            group_parameters(&applied),
            Value::Null,
            "case {label:?}: a group that holds no configuration in effect sets no \
             parameter, got {answered}",
        );

        // Once the clock owes the update, the document it carried is the one reported,
        // parameters and all, beside the identity of the group that took it.
        make_update_due(&emulator, name);
        let read = emulator
            .ok("/get-group-configuration", &json!({ "Group": name }))
            .await;
        assert_eq!(
            read["GroupIdentifier"],
            identifier(name, &arn),
            "case {label:?}: expected the read to report the group the configuration \
             belongs to, got {read}",
        );
        assert_eq!(
            read["GroupConfiguration"]["Configuration"], case.configuration,
            "case {label:?}: expected the applied configuration in effect, got {read}",
        );
        assert_eq!(
            group_parameters(&read["GroupConfiguration"]),
            case.group_parameters,
            "case {label:?}: expected GroupParameters {} once the update is in effect, got \
             {read}",
            case.group_parameters,
        );
    }
}
