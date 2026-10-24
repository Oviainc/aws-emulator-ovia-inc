// SPDX-License-Identifier: CC0-1.0

//! UpdateRouterNetworkInterface end to end over HTTP: CreateRouterNetworkInterface ->
//! UpdateRouterNetworkInterface -> BatchGetRouterNetworkInterface.
//!
//! The rule under test: an update replaces EXACTLY the members the request names on the
//! stored record, leaves every other member as it was, and is answered from the stored
//! record — so the implemented reader (BatchGetRouterNetworkInterface) reports afterwards
//! what the update wrote, not what the request said.
//!
//! The rows span what that rule generalises over: which of the two updatable members the
//! request names (`Name` alone, `Configuration` alone, both, neither), which configuration
//! union variant — and so which network interface type — the interface carries, and how the
//! named configuration differs from the stored one (more allow rules, none at all, a
//! different subnet, the modelled minimum and maximum security-group counts). The recorded
//! case is one row among them.
//!
//! Two refusals are asserted over the same class: a `Configuration` whose union variant
//! differs from the stored one ("changing the type of the configuration is not supported",
//! `smithy_query(query="doc",shape="UpdateRouterNetworkInterfaceRequest",member="Configuration")`),
//! in BOTH directions, and an `Arn` no interface exists for — which this operation answers
//! with `BadRequestException`, because the model declares no `NotFoundException` for it
//! (`smithy_query(query="operation",operation="UpdateRouterNetworkInterface")`).
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), and the
//! creates and reads go through the helpers `router_network_interfaces` owns, so every case
//! drives the same request path a MediaConnect client reaches.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::union_member;
use crate::router_network_interfaces::{
    batch_get_network_interfaces, create_network_interface, public_configuration,
    reported_timestamp, stored_count, vpc_configuration, vpc_configuration_in_subnet, FLOW_ARN,
    UNKNOWN_ARN,
};

/// The members an update must leave exactly as the create stored them: the interface's
/// identity, the Region it lives in, when it was created, and the two association counts,
/// which are counted from the router inputs and outputs that name it rather than updatable.
///
/// `networkInterfaceType` is in the list because the type IS the configuration's union
/// variant and changing that variant is refused, so an accepted update can never move it.
const UNTOUCHED_MEMBERS: [&str; 7] = [
    "arn",
    "id",
    "networkInterfaceType",
    "regionName",
    "createdAt",
    "associatedInputCount",
    "associatedOutputCount",
];

/// Send one UpdateRouterNetworkInterface over the wire.
///
/// `PUT /v1/routerNetworkInterface/{Arn}`
/// (`smithy_query(query="operation",operation="UpdateRouterNetworkInterface")`), with `Arn`
/// bound as an `@httpLabel` and the two updatable members in the body.
async fn update_network_interface(
    emulator: &Emulator,
    arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/routerNetworkInterface/{arn}"),
            Some(body),
        )
        .await
}

/// Create one interface over the wire, wait out the transition its create owes, and answer
/// `(ARN, the created interface)`.
///
/// The wait is part of the setup rather than part of a case: an update MOVES the interface's
/// state, so it is refused while the clock still owes it one (asserted on its own in
/// [`an_update_is_refused_while_a_state_change_is_in_flight`]).
async fn settled_interface(emulator: &Emulator, name: &str, configuration: Value) -> (String, Value) {
    let (status, code, created) = create_network_interface(
        emulator,
        json!({ "name": name, "configuration": configuration }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "creating {name:?} expected 201, got {status} {code:?}: {created}"
    );
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create of {name:?} answers an ARN: {created}"))
        .to_string();
    (arn, created["routerNetworkInterface"].clone())
}

/// One update, and what the stored interface must report once it has been applied.
struct UpdateCase {
    case: &'static str,
    /// The name the create gives the interface.
    created_name: String,
    /// The configuration the create stores.
    created_configuration: Value,
    /// The update request's body.
    update: Value,
    /// `RouterNetworkInterface$Name` afterwards.
    expected_name: String,
    /// The configuration afterwards, in the same union form the request carries — the
    /// request's where it named one, the created one where it did not.
    expected_configuration: Value,
    /// The union member the configuration must be read under, as
    /// `(Smithy member name, @jsonName)`.
    configuration_member: (&'static str, &'static str),
    /// Whether this request changes the record at all: a request that names NEITHER
    /// updatable member asks for no change, so it must move neither `UpdatedAt` nor `State`.
    changes_the_record: bool,
}

/// `Name` has a modelled maximum of 128 characters
/// (`smithy_query(query="operation",operation="UpdateRouterNetworkInterface")`: `Name`
/// length 1..128, pattern `^[a-zA-Z0-9]([a-zA-Z0-9\-_]*[a-zA-Z0-9])?$`), so one row renames
/// an interface to a name of exactly that length and the refusal table names one past it.
fn name_of_length(length: usize) -> String {
    "n".repeat(length)
}

fn update_cases() -> Vec<UpdateCase> {
    let public_one = public_configuration(1);
    let public_ten = public_configuration(10);
    let public_none = public_configuration(0);
    let public_three = public_configuration(3);
    let vpc_one = vpc_configuration(1);
    let vpc_five = vpc_configuration(5);
    let public_two = public_configuration(2);
    let vpc_two = vpc_configuration(2);
    let other_subnet = vpc_configuration_in_subnet(1, "subnet-00000000000000abc");

    vec![
        UpdateCase {
            case: "Name alone, on a PUBLIC interface",
            created_name: "rni-update-name".to_string(),
            created_configuration: public_one.clone(),
            update: json!({ "name": "rni-update-name-renamed" }),
            expected_name: "rni-update-name-renamed".to_string(),
            // The request named no Configuration, so the stored one stays.
            expected_configuration: public_one.clone(),
            configuration_member: ("Public", "public"),
            changes_the_record: true,
        },
        UpdateCase {
            case: "Configuration alone, a PUBLIC interface grown to the modelled ten allow rules",
            created_name: "rni-update-grow".to_string(),
            created_configuration: public_one.clone(),
            update: json!({ "configuration": public_ten.clone() }),
            expected_name: "rni-update-grow".to_string(),
            expected_configuration: public_ten.clone(),
            configuration_member: ("Public", "public"),
            changes_the_record: true,
        },
        UpdateCase {
            case: "Configuration alone, a PUBLIC interface emptied to no allow rules",
            created_name: "rni-update-empty".to_string(),
            created_configuration: public_three.clone(),
            update: json!({ "configuration": public_none.clone() }),
            expected_name: "rni-update-empty".to_string(),
            expected_configuration: public_none.clone(),
            configuration_member: ("Public", "public"),
            changes_the_record: true,
        },
        UpdateCase {
            case: "both members, a VPC interface grown to the modelled five security groups",
            created_name: "rni-update-vpc".to_string(),
            created_configuration: vpc_one.clone(),
            update: json!({ "name": "rni-update-vpc-renamed", "configuration": vpc_five.clone() }),
            expected_name: "rni-update-vpc-renamed".to_string(),
            expected_configuration: vpc_five.clone(),
            configuration_member: ("Vpc", "vpc"),
            changes_the_record: true,
        },
        UpdateCase {
            case: "Configuration alone, a VPC interface moved to another subnet at the modelled \
                   minimum of one security group",
            created_name: "rni-update-subnet".to_string(),
            created_configuration: vpc_five.clone(),
            update: json!({ "configuration": other_subnet.clone() }),
            expected_name: "rni-update-subnet".to_string(),
            expected_configuration: other_subnet.clone(),
            configuration_member: ("Vpc", "vpc"),
            changes_the_record: true,
        },
        UpdateCase {
            case: "Name alone, at the modelled 128-character maximum",
            created_name: "rni-update-long-name".to_string(),
            created_configuration: vpc_two.clone(),
            update: json!({ "name": name_of_length(128) }),
            expected_name: name_of_length(128),
            expected_configuration: vpc_two.clone(),
            configuration_member: ("Vpc", "vpc"),
            changes_the_record: true,
        },
        UpdateCase {
            case: "neither member named: the request asks for no change",
            created_name: "rni-update-nothing".to_string(),
            created_configuration: public_two.clone(),
            update: json!({}),
            expected_name: "rni-update-nothing".to_string(),
            expected_configuration: public_two.clone(),
            configuration_member: ("Public", "public"),
            changes_the_record: false,
        },
    ]
}

#[tokio::test]
async fn update_applies_the_members_it_names_and_leaves_the_rest() {
    let emulator = Emulator::new();

    for UpdateCase {
        case,
        created_name,
        created_configuration,
        update,
        expected_name,
        expected_configuration,
        configuration_member,
        changes_the_record,
    } in update_cases()
    {
        let (arn, created) = settled_interface(&emulator, &created_name, created_configuration).await;

        let (status, code, answer) = update_network_interface(&emulator, &arn, update).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateRouterNetworkInterface expected 202, got {status} {code:?}: \
             {answer}"
        );
        let updated = answer["routerNetworkInterface"].clone();

        assert_eq!(
            updated["name"].as_str(),
            Some(expected_name.as_str()),
            "case {case:?}: Name must be {expected_name:?} after the update, got {:?}",
            updated["name"]
        );
        let (member_name, json_name) = configuration_member;
        assert_eq!(
            union_member(&updated["configuration"], member_name, json_name),
            union_member(&expected_configuration, member_name, json_name),
            "case {case:?}: the stored Configuration must be the expected {member_name} \
             payload, got {:?}",
            updated["configuration"]
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                updated[member], created[member],
                "case {case:?}: an update leaves {member} as the create stored it, got {:?}",
                updated[member]
            );
        }
        assert_eq!(
            updated["tags"], created["tags"],
            "case {case:?}: an update names no Tags, so the stored tags stay, got {:?}",
            updated["tags"]
        );

        let created_at = reported_timestamp(case, "createdAt", &created["createdAt"]);
        let updated_at = reported_timestamp(case, "updatedAt", &updated["updatedAt"]);
        let expected_state = if changes_the_record {
            mc::ROUTER_NETWORK_INTERFACE_STATE_UPDATING
        } else {
            mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE
        };
        assert_eq!(
            updated["state"].as_str(),
            Some(expected_state),
            "case {case:?}: the interface's State must be {expected_state} right after the \
             update, got {:?}",
            updated["state"]
        );
        if changes_the_record {
            assert!(
                updated_at > created_at,
                "case {case:?}: an update that changes the record moves UpdatedAt past \
                 CreatedAt, got {updated_at} and {created_at}"
            );
        } else {
            assert_eq!(
                updated_at, created_at,
                "case {case:?}: a request that changes nothing leaves UpdatedAt where it was, \
                 got {updated_at} and {created_at}"
            );
        }

        // And the implemented reader reports what the update WROTE, for that ARN: the
        // response came from the store, so a read after the clock has settled the update
        // answers the same name and configuration in ACTIVE.
        tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
        let (status, code, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the read-back expected 200, got {status} {code:?}: {read}"
        );
        let reported = read["routerNetworkInterfaces"][0].clone();
        assert_eq!(
            reported["name"].as_str(),
            Some(expected_name.as_str()),
            "case {case:?}: BatchGetRouterNetworkInterface reports the updated Name, got {:?}",
            reported["name"]
        );
        assert_eq!(
            union_member(&reported["configuration"], member_name, json_name),
            union_member(&expected_configuration, member_name, json_name),
            "case {case:?}: BatchGetRouterNetworkInterface reports the updated Configuration, \
             got {:?}",
            reported["configuration"]
        );
        assert_eq!(
            reported["state"].as_str(),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
            "case {case:?}: the clock settles an updated interface back into ACTIVE, got {:?}",
            reported["state"]
        );
        assert_eq!(
            reported["updatedAt"], updated["updatedAt"],
            "case {case:?}: settling the state does not move UpdatedAt, got {:?}",
            reported["updatedAt"]
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                reported[member], created[member],
                "case {case:?}: the read-back reports the untouched {member}, got {:?}",
                reported[member]
            );
        }
    }
}

/// One request this operation must refuse, and the modelled failure it answers with.
struct RefusalCase {
    case: &'static str,
    /// The ARN to address, or `None` for the control interface this test created.
    arn: Option<&'static str>,
    body: Value,
    status: u16,
    code: &'static str,
    /// Which of this service's two 400 envelopes the message carries: the VALIDATION one for
    /// a request whose CONTENT is unacceptable, the BAD REQUEST one for a well-formed request
    /// the addressed interface itself does not admit (`crate::state`).
    prefix: &'static str,
}

#[tokio::test]
async fn refusals_name_the_modelled_error_and_write_nothing() {
    let emulator = Emulator::new();
    // One interface of each configuration variant, so the type-change refusal is asserted in
    // BOTH directions rather than only out of a PUBLIC interface.
    let (public_arn, public_created) =
        settled_interface(&emulator, "rni-refuse-public", public_configuration(2)).await;
    let (vpc_arn, vpc_created) =
        settled_interface(&emulator, "rni-refuse-vpc", vpc_configuration(2)).await;

    let cases = vec![
        RefusalCase {
            case: "a VPC Configuration on a PUBLIC interface changes the type",
            arn: None,
            body: json!({ "configuration": vpc_configuration(1) }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::BAD_REQUEST_PREFIX,
        },
        RefusalCase {
            case: "a VPC Configuration on a PUBLIC interface, named beside a new Name",
            arn: None,
            body: json!({ "name": "rni-refuse-renamed", "configuration": vpc_configuration(3) }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::BAD_REQUEST_PREFIX,
        },
        RefusalCase {
            case: "an Arn the modelled pattern accepts that no interface exists for",
            arn: Some(UNKNOWN_ARN),
            body: json!({ "name": "rni-refuse-absent" }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::BAD_REQUEST_PREFIX,
        },
        RefusalCase {
            case: "an Arn of this service that is not a router network interface ARN",
            arn: Some(FLOW_ARN),
            body: json!({ "name": "rni-refuse-flow-arn" }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::VALIDATION_FAILED_PREFIX,
        },
        RefusalCase {
            case: "a Name outside the modelled pattern",
            arn: None,
            body: json!({ "name": "-rni-leading-dash" }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::VALIDATION_FAILED_PREFIX,
        },
        RefusalCase {
            case: "a Name one character past the modelled 128-character maximum",
            arn: None,
            body: json!({ "name": name_of_length(129) }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::VALIDATION_FAILED_PREFIX,
        },
        RefusalCase {
            case: "a PUBLIC Configuration past the modelled ten allow rules",
            arn: None,
            body: json!({ "configuration": public_configuration(11) }),
            status: 400,
            code: "BadRequestException",
            prefix: mc::VALIDATION_FAILED_PREFIX,
        },
    ];

    for RefusalCase {
        case,
        arn,
        body,
        status: expected_status,
        code: expected_code,
        prefix,
    } in cases
    {
        let addressed = arn.unwrap_or(public_arn.as_str());
        let (status, code, answer) = update_network_interface(&emulator, addressed, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {answer}"
        );
        assert!(
            answer["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(prefix),
            "case {case:?}: the refusal carries the {prefix:?} envelope, got {answer}"
        );
    }

    // The other direction of the type change: a PUBLIC Configuration on the VPC interface.
    let (status, code, answer) = update_network_interface(
        &emulator,
        &vpc_arn,
        json!({ "configuration": public_configuration(1) }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "a PUBLIC Configuration on a VPC interface changes the type, expected 400 \
         BadRequestException, got {status} {code:?}: {answer}"
    );

    // A refused update writes NOTHING: both interfaces still report what their creates
    // stored, still in the settled state no update moved, and no interface was minted.
    let (_, _, read) = batch_get_network_interfaces(
        &emulator,
        &[public_arn.as_str(), vpc_arn.as_str()],
    )
    .await;
    for (case, created, reported) in [
        ("public", &public_created, &read["routerNetworkInterfaces"][0]),
        ("vpc", &vpc_created, &read["routerNetworkInterfaces"][1]),
    ] {
        for member in ["name", "configuration", "networkInterfaceType", "updatedAt"]
            .into_iter()
            .chain(UNTOUCHED_MEMBERS)
        {
            assert_eq!(
                reported[member], created[member],
                "case {case:?}: a refused update leaves {member} as it was, got {:?}",
                reported[member]
            );
        }
        assert_eq!(
            reported["state"].as_str(),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
            "case {case:?}: a refused update starts no transition, got {:?}",
            reported["state"]
        );
    }
    assert_eq!(
        stored_count(&emulator),
        2,
        "a refused update mints nothing, got {} stored",
        stored_count(&emulator)
    );
}

#[tokio::test]
async fn an_update_is_refused_while_a_state_change_is_in_flight() {
    let emulator = Emulator::new();
    let (status, _, created) = create_network_interface(
        &emulator,
        json!({ "name": "rni-in-flight", "configuration": public_configuration(1) }),
    )
    .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let arn = created["routerNetworkInterface"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // The create owes the interface a transition into ACTIVE, so an update asked for before
    // it has settled conflicts with the state change already in progress.
    let (status, code, answer) =
        update_network_interface(&emulator, &arn, json!({ "name": "rni-in-flight-early" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "an interface still being created cannot be updated, expected 409 ConflictException, \
         got {status} {code:?}: {answer}"
    );

    // Once the create has settled the same update is accepted, and reports the update in
    // flight.
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let (status, code, accepted) =
        update_network_interface(&emulator, &arn, json!({ "name": "rni-in-flight-renamed" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "a settled interface is updated, expected 202, got {status} {code:?}: {accepted}"
    );
    assert_eq!(
        accepted["routerNetworkInterface"]["state"].as_str(),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_UPDATING),
        "an accepted update reports the interface UPDATING, got {accepted}"
    );

    // And a second update while THAT one is in flight conflicts in turn, leaving the name the
    // accepted update wrote.
    let (status, code, answer) =
        update_network_interface(&emulator, &arn, json!({ "name": "rni-in-flight-again" })).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "an interface with an update in flight cannot be updated again, expected 409 \
         ConflictException, got {status} {code:?}: {answer}"
    );

    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;
    let (_, _, read) = batch_get_network_interfaces(&emulator, &[&arn]).await;
    assert_eq!(
        read["routerNetworkInterfaces"][0]["name"].as_str(),
        Some("rni-in-flight-renamed"),
        "the refused updates wrote nothing: the accepted one's Name stands, got {read}"
    );
    assert_eq!(
        read["routerNetworkInterfaces"][0]["state"].as_str(),
        Some(mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE),
        "the clock settles the updated interface back into ACTIVE, got {read}"
    );
    assert_eq!(
        read["routerNetworkInterfaces"][0]["regionName"].as_str(),
        Some(REGION),
        "the interface stays in the Region its create minted it in, got {read}"
    );
}

#[tokio::test]
async fn an_update_changes_nothing_another_interface_reports() {
    let emulator = Emulator::new();
    let (first, _) = settled_interface(&emulator, "rni-isolated-one", public_configuration(1)).await;
    let (second, second_created) =
        settled_interface(&emulator, "rni-isolated-two", public_configuration(2)).await;

    let (status, _, answer) = update_network_interface(
        &emulator,
        &first,
        json!({ "name": "rni-isolated-one-renamed", "configuration": public_configuration(4) }),
    )
    .await;
    assert_eq!(status, 202, "the update expected 202, got {status}: {answer}");

    let (_, _, read) = batch_get_network_interfaces(&emulator, &[second.as_str()]).await;
    let reported = read["routerNetworkInterfaces"][0].clone();
    for member in ["name", "configuration", "updatedAt"]
        .into_iter()
        .chain(UNTOUCHED_MEMBERS)
    {
        assert_eq!(
            reported[member], second_created[member],
            "updating one interface leaves another's {member} alone, got {:?}",
            reported[member]
        );
    }
    assert_eq!(
        stored_count(&emulator),
        2,
        "an update mints no interface, got {} stored",
        stored_count(&emulator)
    );
}
