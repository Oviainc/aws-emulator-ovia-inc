// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyNetworkInterfaceAttribute` and
//! `ResetNetworkInterfaceAttribute`, driven over the wire the service actually speaks:
//! `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries `Action=…`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that a modification writes the ONE attribute its request names
//! onto the stored interface — "you can specify only one attribute at a time" — so the
//! value is what `DescribeNetworkInterfaceAttribute` and `DescribeNetworkInterfaces` then
//! report for it, every other attribute of that interface stays where it was, and a reset
//! puts the source/destination check back to the default a create leaves it at. A request
//! that names no attribute, names two, carries no value for the one it names, or names a
//! group, subnet, attachment, timeout or primary IPv6 the interface cannot take is refused
//! and writes nothing.
//!
//! So the tables below span that class rather than one recorded request: every modifiable
//! attribute, each over values that differ along what it is about — a description that is
//! empty, a word and a sentence with punctuation; one, two and a repeated security group; a
//! timeout at each published bound and one past it; a subnet of the interface's VPC and one
//! of another; an attachment id this service issued and one it did not; an interface type
//! that has a source/destination check and the one that has none. Each value is read back
//! through both describes, so the two reports of one interface cannot disagree, and the
//! attributes the request did NOT name are read back too, so no write can reach further
//! than the attribute it was asked for.

use std::collections::BTreeMap;

use ec2::ops::describe_network_interface_attribute as describe_attribute_op;
use ec2::ops::modify_network_interface_attribute as modify_op;
use ec2::ops::reset_network_interface_attribute as reset_op;
use ec2::ops::types::NetworkInterfaceIpv6Address;
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, param, set_items, Param};

/// The Region every call here is signed for: a fresh account's default VPC and its per-zone
/// subnets are seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// The image every launch names; which image an instance ran from has no bearing on the
/// attributes of its interfaces.
const AMI: &str = "ami-0123456789abcdef0";

/// The instance type every launch names: a type whose
/// `NetworkInfo$MaximumNetworkInterfaces` admits a secondary interface at device index 1.
const INSTANCE_TYPE: &str = "m5.4xlarge";

/// A well-formed network interface id, in the long form, that no interface of the account
/// has.
const ABSENT_INTERFACE: &str = "eni-0000000000000beef";

/// A well-formed interface id in the SHORT form (8 hex digits) that no interface of the
/// account has either — the id form is part of what "well formed" means here.
const ABSENT_SHORT_INTERFACE: &str = "eni-0badf00d";

/// A value that is not a network interface id at all.
const MALFORMED_INTERFACE: &str = "eni-not-an-id";

/// The error codes this suite asserts on. EC2's model declares no error shapes, so each is
/// matched as the wire string the service answers with.
const MISSING_PARAMETER: &str = "MissingParameter";
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";
const INVALID_NETWORK_INTERFACE_ID_MALFORMED: &str = "InvalidNetworkInterfaceId.Malformed";
const INVALID_NETWORK_INTERFACE_ID_NOT_FOUND: &str = "InvalidNetworkInterfaceID.NotFound";
const INVALID_GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";
const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";
const INVALID_ATTACHMENT_ID_MALFORMED: &str = "InvalidNetworkInterfaceAttachmentId.Malformed";
const INVALID_ATTACHMENT_ID_NOT_FOUND: &str = "InvalidAttachmentID.NotFound";
const DRY_RUN_OPERATION: &str = "DryRunOperation";

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call that must succeed, as its body.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The `SubnetId` of a subnet of the account's default VPC, and its VPC id.
fn a_default_subnet(engine: &engine::InMemoryEngine) -> (String, String) {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    let (subnet_id, subnet) = store
        .subnets
        .iter()
        .next()
        .expect("a fresh account has the default VPC's subnets");
    (
        subnet_id.clone(),
        subnet.vpc_id.clone().unwrap_or_default(),
    )
}

/// Another subnet of the account's default VPC — a second subnet an association may name.
fn another_default_subnet(engine: &engine::InMemoryEngine) -> String {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .subnets
        .keys()
        .nth(1)
        .cloned()
        .expect("a fresh account has a default subnet per Availability Zone")
}

/// One `CreateNetworkInterface` in `subnet_id` carrying `extra`, as the interface id it
/// minted.
async fn create_interface(
    router: &MultiServiceRouter,
    case: &str,
    subnet_id: &str,
    extra: &[Param],
) -> String {
    let mut params = vec![param("SubnetId", subnet_id)];
    params.extend_from_slice(extra);
    let body = ok(case, router, "CreateNetworkInterface", &params).await;
    element(&body, "networkInterfaceId")
        .unwrap_or_else(|| panic!("case {case:?}: no <networkInterfaceId> in {body}"))
}

/// One `CreateSecurityGroup` in `vpc_id`, as the group id it minted.
async fn create_group(router: &MultiServiceRouter, case: &str, vpc_id: &str, name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("VpcId", vpc_id),
            param("GroupName", name),
            param("GroupDescription", "a group this suite created"),
        ],
    )
    .await;
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// One `ModifyNetworkInterfaceAttribute` call for `network_interface_id` carrying `changes`.
async fn modify(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    changes: &[Param],
) -> (u16, String) {
    let mut params = vec![param("NetworkInterfaceId", network_interface_id)];
    params.extend_from_slice(changes);
    call(router, REGION, "ModifyNetworkInterfaceAttribute", &params).await
}

/// One `ModifyNetworkInterfaceAttribute` that must succeed.
async fn modified(case: &str, router: &MultiServiceRouter, eni: &str, changes: &[Param]) {
    let (status, body) = modify(router, eni, changes).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from a modification naming {changes:?}, got \
         {status} with body {body}"
    );
}

/// One `ResetNetworkInterfaceAttribute` call for `network_interface_id` carrying `extra`.
async fn reset(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![param("NetworkInterfaceId", network_interface_id)];
    params.extend_from_slice(extra);
    call(router, REGION, "ResetNetworkInterfaceAttribute", &params).await
}

/// The form field that names the attribute a reset asks for: the `SourceDestCheck`
/// parameter, which carries no value of its own.
fn reset_source_dest_check() -> Vec<Param> {
    vec![param("SourceDestCheck", "")]
}

/// The one interface `network_interface_id` names, as `DescribeNetworkInterfaces` reports
/// it: every element of the report, flattened to `path -> text`, so an assertion names the
/// member it means and an attribute the describe does not report at all reads as absent.
async fn described(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
) -> BTreeMap<String, String> {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    let parsed = document(case, &body);
    let item = set_items(parsed.root_element(), "networkInterfaceSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no interface in the describe body {body}"));
    let mut flattened = BTreeMap::new();
    flatten(item, String::new(), &mut flattened);
    flattened
}

/// Every element under `node`, as `a/b/c -> text`, with repeated siblings numbered from 1 so
/// a list's entries keep the order the service reported them in.
fn flatten(node: roxmltree::Node<'_, '_>, prefix: String, into: &mut BTreeMap<String, String>) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for element in node.children().filter(|child| child.is_element()) {
        let name = element.tag_name().name().to_string();
        let count = seen.entry(name.clone()).or_insert(0);
        *count += 1;
        let path = match (prefix.is_empty(), *count) {
            (true, 1) => name.clone(),
            (true, index) => format!("{name}.{index}"),
            (false, 1) => format!("{prefix}/{name}"),
            (false, index) => format!("{prefix}/{name}.{index}"),
        };
        if element.children().any(|child| child.is_element()) {
            flatten(element, path, into);
        } else {
            into.insert(path, element.text().unwrap_or_default().to_string());
        }
    }
}

/// What `DescribeNetworkInterfaceAttribute` reports for ONE attribute of one interface, as
/// the text of the member that carries it: the describe's own view of the value a
/// modification wrote, so the two reports of one interface are compared against each other.
async fn described_attribute(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
    attribute: &str,
) -> Option<String> {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaceAttribute",
        &[
            param("NetworkInterfaceId", network_interface_id),
            param("Attribute", attribute),
        ],
    )
    .await;
    let parsed = document(case, &body);
    let root = parsed.root_element();
    let carrier = child(root, attribute)?;
    // `description` and `sourceDestCheck` are `AttributeValue`-shaped (a `<value>` child);
    // `associatePublicIpAddress` is the boolean itself.
    match child_text(carrier, "value") {
        Some(value) => Some(value),
        None => carrier.text().map(str::to_string),
    }
}

/// The security groups the attribute describe reports for one interface, as
/// `(groupId, groupName)` pairs in the order it reported them.
async fn described_groups(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
) -> Vec<(String, String)> {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaceAttribute",
        &[
            param("NetworkInterfaceId", network_interface_id),
            param("Attribute", describe_attribute_op::ATTRIBUTE_GROUP_SET),
        ],
    )
    .await;
    let parsed = document(case, &body);
    let set = child(
        parsed.root_element(),
        describe_attribute_op::ATTRIBUTE_GROUP_SET,
    )
    .unwrap_or_else(|| panic!("case {case:?}: no <groupSet> in {body}"));
    set_items(set.parent().expect("the response envelope"), "groupSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "groupId").unwrap_or_default(),
                child_text(item, "groupName").unwrap_or_default(),
            )
        })
        .collect()
}

/// A refusal's code and message, for a call that must have been refused.
fn refused(case: &str, status: u16, body: &str, expected_status: u16) -> (String, String) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
    );
    (error_code(body), error_message(body))
}

// ---------------------------------------------------------------------------
// 1. Each attribute a request names is written, and nothing else moves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_description_is_written_as_the_request_names_it() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(
        &router,
        "the interface",
        &subnet_id,
        &[param("Description", "as created")],
    )
    .await;

    // The class the rule generalises over is the TEXT: a description cleared to nothing, one
    // word, a sentence with punctuation, and one at the length a caller may send.
    let long = "d".repeat(255);
    for description in ["", "nat", "routes, forwards; and translates", long.as_str()] {
        let case = format!("description {description:?}");
        modified(
            &case,
            &router,
            &eni,
            &[param("Description.Value", description)],
        )
        .await;
        let reported = described_attribute(
            &case,
            &router,
            &eni,
            describe_attribute_op::ATTRIBUTE_DESCRIPTION,
        )
        .await;
        assert_eq!(
            reported.as_deref().unwrap_or_default(),
            description,
            "case {case:?}: the attribute describe must report the description that was \
             written, got {reported:?}"
        );
        let interface = described(&case, &router, &eni).await;
        assert_eq!(
            interface.get("description").map(String::as_str).unwrap_or_default(),
            description,
            "case {case:?}: DescribeNetworkInterfaces must report the same description, got \
             {interface:?}"
        );
        // The attribute the request did NOT name: an interface is created with its
        // source/destination check on, and a description write must leave it there.
        assert_eq!(
            interface.get("sourceDestCheck").map(String::as_str),
            Some("true"),
            "case {case:?}: writing a description must not move sourceDestCheck, got \
             {interface:?}"
        );
    }
}

#[tokio::test]
async fn a_source_dest_check_is_written_as_the_request_names_it() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(
        &router,
        "the interface",
        &subnet_id,
        &[param("Description", "unchanged by a boolean write")],
    )
    .await;

    // Off, back on, and off again: the attribute is a boolean, so the class it generalises
    // over is both values AND a write that repeats the value the interface already holds.
    for value in [false, true, false, false] {
        let case = format!("sourceDestCheck {value}");
        modified(
            &case,
            &router,
            &eni,
            &[param("SourceDestCheck.Value", &value.to_string())],
        )
        .await;
        let reported = described_attribute(
            &case,
            &router,
            &eni,
            describe_attribute_op::ATTRIBUTE_SOURCE_DEST_CHECK,
        )
        .await;
        assert_eq!(
            reported.as_deref(),
            Some(value.to_string().as_str()),
            "case {case:?}: the attribute describe must report the check that was written, \
             got {reported:?}"
        );
        let interface = described(&case, &router, &eni).await;
        assert_eq!(
            interface.get("sourceDestCheck").map(String::as_str),
            Some(value.to_string().as_str()),
            "case {case:?}: DescribeNetworkInterfaces must report the same check, got \
             {interface:?}"
        );
        assert_eq!(
            interface.get("description").map(String::as_str),
            Some("unchanged by a boolean write"),
            "case {case:?}: writing sourceDestCheck must not move the description, got \
             {interface:?}"
        );
    }
}

#[tokio::test]
async fn a_modification_that_names_no_value_to_write_is_refused() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;

    // The boolean attribute carried with nothing to write: `AttributeBooleanValue` has one
    // optional `Value`, so a request that carries the member and no value says nothing about
    // what to set the check to.
    let case = "sourceDestCheck with no value";
    let (status, body) = modify(&router, &eni, &[param("SourceDestCheck", "")]).await;
    let (code, message) = refused(case, status, &body, 400);
    assert_eq!(
        code, MISSING_PARAMETER,
        "case {case:?}: expected {MISSING_PARAMETER}, got {code} with message {message:?}"
    );
    let interface = described("after the refusal", &router, &eni).await;
    assert_eq!(
        interface.get("sourceDestCheck").map(String::as_str),
        Some("true"),
        "case {case:?}: a refused modification must write nothing, got {interface:?}"
    );
}

#[tokio::test]
async fn the_security_groups_a_request_names_replace_the_set_the_interface_held() {
    let (router, engine) = emulator();
    let (subnet_id, vpc_id) = a_default_subnet(&engine);
    let first = create_group(&router, "the first group", &vpc_id, "first-group").await;
    let second = create_group(&router, "the second group", &vpc_id, "second-group").await;
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;
    let created = described_groups("as created", &router, &eni).await;
    assert_eq!(
        created.len(),
        1,
        "case \"as created\": an interface created without groups holds its VPC's default \
         group, got {created:?}"
    );

    // One group, two groups, a repeated group (one set), and back to the VPC's default: the
    // class is the SET, so the rows differ in size and in repetition.
    let default_group = created[0].0.clone();
    let rows: [(&str, Vec<&str>, Vec<&str>); 4] = [
        ("one group", vec![first.as_str()], vec![first.as_str()]),
        (
            "two groups",
            vec![first.as_str(), second.as_str()],
            vec![first.as_str(), second.as_str()],
        ),
        (
            "a repeated group",
            vec![second.as_str(), second.as_str()],
            vec![second.as_str()],
        ),
        (
            "back to the default group",
            vec![default_group.as_str()],
            vec![default_group.as_str()],
        ),
    ];
    for (case, named, expected) in rows {
        modified(case, &router, &eni, &indexed("SecurityGroupId", &named)).await;
        let reported: Vec<String> = described_groups(case, &router, &eni)
            .await
            .into_iter()
            .map(|(group_id, _)| group_id)
            .collect();
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|group| (*group).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the new set of groups must replace the current set, got \
             {reported:?}"
        );
        let interface = described(case, &router, &eni).await;
        let described_ids: Vec<&str> = interface
            .iter()
            .filter(|(path, _)| path.starts_with("groupSet/item") && path.ends_with("groupId"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            described_ids.len(),
            expected.len(),
            "case {case:?}: DescribeNetworkInterfaces must report the same groups, got \
             {interface:?}"
        );
    }
}

#[tokio::test]
async fn a_group_the_interfaces_vpc_does_not_hold_is_refused() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;
    let held = described_groups("as created", &router, &eni).await;

    // A group of ANOTHER VPC is as unresolvable here as one that does not exist: a security
    // group only applies within its own VPC.
    let other_vpc = element(
        &ok(
            "another vpc",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.63.0.0/16")],
        )
        .await,
        "vpcId",
    )
    .expect("a created VPC reports its id");
    let foreign = create_group(&router, "a group of another vpc", &other_vpc, "foreign").await;
    for (case, group) in [
        ("a group that does not exist", "sg-0000000000000beef"),
        ("a group of another vpc", foreign.as_str()),
    ] {
        let (status, body) = modify(&router, &eni, &indexed("SecurityGroupId", &[group])).await;
        let (code, message) = refused(case, status, &body, 400);
        assert_eq!(
            code, INVALID_GROUP_NOT_FOUND,
            "case {case:?}: expected {INVALID_GROUP_NOT_FOUND}, got {code} with message \
             {message:?}"
        );
    }
    assert_eq!(
        described_groups("after the refusals", &router, &eni).await,
        held,
        "a refused group change must leave the groups the interface held"
    );
}

#[tokio::test]
async fn the_connection_tracking_timeouts_are_held_to_the_range_each_member_documents() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;

    // The three members, each with the inclusive bounds its own documentation publishes
    // (`ConnectionTrackingSpecificationRequest$TcpEstablishedTimeout` and the two beside
    // it), as the handler reads them.
    let members: [(&str, &str, (i32, i32)); 3] = [
        (
            "TcpEstablishedTimeout",
            "tcpEstablishedTimeout",
            modify_op::TCP_ESTABLISHED_TIMEOUT_SECONDS,
        ),
        (
            "UdpStreamTimeout",
            "udpStreamTimeout",
            modify_op::UDP_STREAM_TIMEOUT_SECONDS,
        ),
        ("UdpTimeout", "udpTimeout", modify_op::UDP_TIMEOUT_SECONDS),
    ];
    for (member, reported_as, (minimum, maximum)) in members {
        // Each published bound is accepted and read back; one past each is refused.
        for seconds in [minimum, maximum] {
            let case = format!("{member} at {seconds}");
            modified(
                &case,
                &router,
                &eni,
                &[param(
                    &format!("ConnectionTrackingSpecification.{member}"),
                    &seconds.to_string(),
                )],
            )
            .await;
            let interface = described(&case, &router, &eni).await;
            let path = format!("connectionTrackingConfiguration/{reported_as}");
            assert_eq!(
                interface.get(&path).map(String::as_str),
                Some(seconds.to_string().as_str()),
                "case {case:?}: the describe must report the timeout that was written at \
                 {path}, got {interface:?}"
            );
        }
        for seconds in [minimum - 1, maximum + 1] {
            let case = format!("{member} at {seconds}");
            let (status, body) = modify(
                &router,
                &eni,
                &[param(
                    &format!("ConnectionTrackingSpecification.{member}"),
                    &seconds.to_string(),
                )],
            )
            .await;
            let (code, message) = refused(&case, status, &body, 400);
            assert_eq!(
                code, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE} outside {minimum}..\
                 ={maximum}, got {code} with message {message:?}"
            );
            assert!(
                message.contains(&minimum.to_string()) && message.contains(&maximum.to_string()),
                "case {case:?}: the refusal must name the range it holds the member to, got \
                 {message:?}"
            );
        }
    }

    // All three at once: one member of the request, so one attribute — the specification is
    // written whole.
    let case = "every timeout at once";
    modified(
        case,
        &router,
        &eni,
        &[
            param("ConnectionTrackingSpecification.TcpEstablishedTimeout", "90"),
            param("ConnectionTrackingSpecification.UdpStreamTimeout", "120"),
            param("ConnectionTrackingSpecification.UdpTimeout", "45"),
        ],
    )
    .await;
    let interface = described(case, &router, &eni).await;
    for (path, expected) in [
        ("connectionTrackingConfiguration/tcpEstablishedTimeout", "90"),
        ("connectionTrackingConfiguration/udpStreamTimeout", "120"),
        ("connectionTrackingConfiguration/udpTimeout", "45"),
    ] {
        assert_eq!(
            interface.get(path).map(String::as_str),
            Some(expected),
            "case {case:?}: the describe must report {path} as {expected}, got {interface:?}"
        );
    }
}

#[tokio::test]
async fn the_subnets_a_request_names_are_associated_with_the_interface() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let other_subnet = another_default_subnet(&engine);
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;

    // One subnet, then two: the new set replaces the stored one, so the rows differ in size.
    let rows: [(&str, Vec<&str>); 3] = [
        ("its own subnet", vec![subnet_id.as_str()]),
        (
            "two subnets of its VPC",
            vec![subnet_id.as_str(), other_subnet.as_str()],
        ),
        ("back to one subnet", vec![other_subnet.as_str()]),
    ];
    for (case, named) in rows {
        modified(case, &router, &eni, &indexed("AssociatedSubnetId", &named)).await;
        let interface = described(case, &router, &eni).await;
        let reported: Vec<&str> = interface
            .iter()
            .filter(|(path, _)| path.starts_with("associatedSubnetSet/item"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            reported, named,
            "case {case:?}: the describe must report the subnets that were associated, got \
             {interface:?}"
        );
    }

    // A subnet the account does not hold, and one of another VPC: an interface cannot be
    // associated with a network it is not in.
    let foreign_vpc = element(
        &ok(
            "another vpc",
            &router,
            "CreateVpc",
            &[param("CidrBlock", "10.64.0.0/16")],
        )
        .await,
        "vpcId",
    )
    .expect("a created VPC reports its id");
    let foreign_subnet = element(
        &ok(
            "a subnet of another vpc",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &foreign_vpc),
                param("CidrBlock", "10.64.1.0/24"),
            ],
        )
        .await,
        "subnetId",
    )
    .expect("a created subnet reports its id");
    for (case, subnet, expected) in [
        (
            "a subnet that does not exist",
            "subnet-0000000000000beef",
            INVALID_SUBNET_ID_NOT_FOUND,
        ),
        (
            "a subnet of another vpc",
            foreign_subnet.as_str(),
            INVALID_PARAMETER_VALUE,
        ),
    ] {
        let (status, body) = modify(&router, &eni, &indexed("AssociatedSubnetId", &[subnet])).await;
        let (code, message) = refused(case, status, &body, 400);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with message {message:?}"
        );
    }
    let interface = described("after the refusals", &router, &eni).await;
    let reported: Vec<&str> = interface
        .iter()
        .filter(|(path, _)| path.starts_with("associatedSubnetSet/item"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(
        reported,
        vec![other_subnet.as_str()],
        "a refused association must leave the subnets the interface held, got {interface:?}"
    );
}

#[tokio::test]
async fn a_public_ipv4_address_is_assigned_and_taken_away_as_the_attribute_names() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;

    let case = "as created";
    assert_eq!(
        described_attribute(
            case,
            &router,
            &eni,
            describe_attribute_op::ATTRIBUTE_ASSOCIATE_PUBLIC_IP_ADDRESS
        )
        .await
        .as_deref(),
        Some("false"),
        "case {case:?}: an interface created without one associates no public IPv4 address"
    );

    // Enabled, enabled again (the address the first call assigned stays), then disabled: the
    // class is the boolean, and the repeat is what makes the write idempotent.
    let case = "enabled";
    modified(case, &router, &eni, &[param("AssociatePublicIpAddress", "true")]).await;
    let interface = described(case, &router, &eni).await;
    let assigned = interface
        .get("association/publicIp")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no association/publicIp in {interface:?}"));
    assert_eq!(
        interface.get("association/ipOwnerId").map(String::as_str),
        Some(describe_attribute_op::AMAZON_ASSIGNED_IP_OWNER_ID),
        "case {case:?}: an address AWS assigned is owned by AWS, got {interface:?}"
    );
    assert_eq!(
        described_attribute(
            case,
            &router,
            &eni,
            describe_attribute_op::ATTRIBUTE_ASSOCIATE_PUBLIC_IP_ADDRESS
        )
        .await
        .as_deref(),
        Some("true"),
        "case {case:?}: the attribute describe must report the address the interface now has"
    );
    assert!(
        interface
            .get("association/publicDnsName")
            .is_some_and(|name| name.contains(&assigned.replace('.', "-"))),
        "case {case:?}: the public DNS name must name the address assigned, got {interface:?}"
    );

    let case = "enabled twice";
    modified(case, &router, &eni, &[param("AssociatePublicIpAddress", "true")]).await;
    let interface = described(case, &router, &eni).await;
    assert_eq!(
        interface.get("association/publicIp"),
        Some(&assigned),
        "case {case:?}: enabling the attribute again must keep the address already assigned, \
         got {interface:?}"
    );

    let case = "disabled";
    modified(
        case,
        &router,
        &eni,
        &[param("AssociatePublicIpAddress", "false")],
    )
    .await;
    let interface = described(case, &router, &eni).await;
    assert_eq!(
        interface.get("association/publicIp"),
        None,
        "case {case:?}: disabling the attribute must take the AWS-assigned address away, got \
         {interface:?}"
    );
    assert_eq!(
        described_attribute(
            case,
            &router,
            &eni,
            describe_attribute_op::ATTRIBUTE_ASSOCIATE_PUBLIC_IP_ADDRESS
        )
        .await
        .as_deref(),
        Some("false"),
        "case {case:?}: the attribute describe must report that no address is associated"
    );
}

#[tokio::test]
async fn the_first_ipv6_address_the_interface_holds_becomes_its_primary_one() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);

    // One interface holding one IPv6 address and one holding two — "the first IPv6 GUA
    // address associated with the ENI becomes the primary IPv6 address", so the row with two
    // is what tells a rule that names the FIRST from one that marks every address.
    let rows: [(&str, Vec<&str>); 2] = [
        ("one ipv6 address", vec!["2600:1f18::1"]),
        (
            "two ipv6 addresses",
            vec!["2600:1f18::2", "2600:1f18::3"],
        ),
    ];
    for (case, addresses) in rows {
        let eni = create_interface(&router, case, &subnet_id, &[]).await;
        seed_ipv6_addresses(&engine, &eni, &addresses);
        modified(case, &router, &eni, &[param("EnablePrimaryIpv6", "true")]).await;
        let interface = described(case, &router, &eni).await;
        let primary: Vec<&str> = (1..=addresses.len())
            .map(|index| {
                let path = if index == 1 {
                    "ipv6AddressesSet/item/isPrimaryIpv6".to_string()
                } else {
                    format!("ipv6AddressesSet/item.{index}/isPrimaryIpv6")
                };
                interface.get(&path).map(String::as_str).unwrap_or_default()
            })
            .collect();
        let mut expected = vec!["true"];
        expected.extend(std::iter::repeat_n("false", addresses.len() - 1));
        assert_eq!(
            primary, expected,
            "case {case:?}: exactly the first IPv6 address must be the primary one, got \
             {interface:?}"
        );

        // "Once you enable an IPv6 GUA address to be a primary IPv6, you cannot disable it".
        let (status, body) = modify(&router, &eni, &[param("EnablePrimaryIpv6", "false")]).await;
        let (code, message) = refused(case, status, &body, 400);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} disabling a primary IPv6, got \
             {code} with message {message:?}"
        );
        let interface = described(case, &router, &eni).await;
        assert_eq!(
            interface
                .get("ipv6AddressesSet/item/isPrimaryIpv6")
                .map(String::as_str),
            Some("true"),
            "case {case:?}: a refused disable must leave the primary IPv6 as it was, got \
             {interface:?}"
        );
    }

    // An interface in a subnet with no IPv6 CIDR block has no IPv6 GUA address to make
    // primary: "if you're modifying a network interface in a dual-stack or IPv6-only subnet".
    let case = "no ipv6 address at all";
    let eni = create_interface(&router, case, &subnet_id, &[]).await;
    let (status, body) = modify(&router, &eni, &[param("EnablePrimaryIpv6", "true")]).await;
    let (code, message) = refused(case, status, &body, 400);
    assert_eq!(
        code, INVALID_PARAMETER_VALUE,
        "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code} with message \
         {message:?}"
    );
}

/// Seed `addresses` onto the interface `network_interface_id` as the IPv6 addresses it
/// holds, the way `AssignIpv6Addresses` leaves them: carried by the record, none of them
/// primary.
///
/// Seeded rather than assigned over the wire so this suite stands on an interface that HOLDS
/// IPv6 addresses however they came to be assigned — which address an assignment picks out
/// of a subnet's range is that operation's own rule, driven by its own suite.
fn seed_ipv6_addresses(
    engine: &engine::InMemoryEngine,
    network_interface_id: &str,
    addresses: &[&str],
) {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let interface = store
        .network_interfaces
        .get_mut(network_interface_id)
        .unwrap_or_else(|| panic!("the store holds the interface {network_interface_id}"));
    interface.ipv6_addresses = Some(
        addresses
            .iter()
            .map(|address| NetworkInterfaceIpv6Address {
                ipv6_address: Some((*address).to_string()),
                public_ipv6_dns_name: None,
                is_primary_ipv6: None,
            })
            .collect(),
    );
}

// ---------------------------------------------------------------------------
// 2. The attachment that holds the interface
// ---------------------------------------------------------------------------

/// One instance launched into `subnet_id`, as its id.
async fn launch(router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        "the launch",
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("InstanceType", INSTANCE_TYPE),
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("SubnetId", subnet_id),
        ],
    )
    .await;
    element(&body, "instanceId").expect("a launch reports the instance it minted")
}

/// One interface attached to a fresh instance at device index 1, as its interface id and the
/// attachment id the attach minted.
async fn an_attached_interface(
    router: &MultiServiceRouter,
    subnet_id: &str,
) -> (String, String) {
    let eni = create_interface(router, "the attached interface", subnet_id, &[]).await;
    let instance = launch(router, subnet_id).await;
    let body = ok(
        "the attach",
        router,
        "AttachNetworkInterface",
        &[
            param("NetworkInterfaceId", &eni),
            param("InstanceId", &instance),
            param("DeviceIndex", "1"),
        ],
    )
    .await;
    let attachment_id = element(&body, "attachmentId").expect("an attach reports its attachment");
    (eni, attachment_id)
}

#[tokio::test]
async fn an_attachment_change_is_written_onto_the_attachment_that_holds_the_interface() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let (eni, attachment_id) = an_attached_interface(&router, &subnet_id).await;
    let attached = described("as attached", &router, &eni).await;
    assert_eq!(
        attached
            .get("attachment/deleteOnTermination")
            .map(String::as_str),
        Some("false"),
        "an interface attached to an existing instance does not go with it, got {attached:?}"
    );

    // Each member of the attachment change, over the values it takes: the flag both ways,
    // an explicit queue count, and the default count that leaves the attachment with none.
    for value in [true, false] {
        let case = format!("deleteOnTermination {value}");
        modified(
            &case,
            &router,
            &eni,
            &[
                param("Attachment.AttachmentId", &attachment_id),
                param("Attachment.DeleteOnTermination", &value.to_string()),
            ],
        )
        .await;
        let interface = described(&case, &router, &eni).await;
        assert_eq!(
            interface
                .get("attachment/deleteOnTermination")
                .map(String::as_str),
            Some(value.to_string().as_str()),
            "case {case:?}: the describe must report the flag that was written, got \
             {interface:?}"
        );
        assert_eq!(
            interface.get("attachment/attachmentId").map(String::as_str),
            Some(attachment_id.as_str()),
            "case {case:?}: a change to an attachment must not replace it, got {interface:?}"
        );
    }

    for count in ["2", "8"] {
        let case = format!("enaQueueCount {count}");
        modified(
            &case,
            &router,
            &eni,
            &[
                param("Attachment.AttachmentId", &attachment_id),
                param("Attachment.EnaQueueCount", count),
            ],
        )
        .await;
        let interface = described(&case, &router, &eni).await;
        assert_eq!(
            interface.get("attachment/enaQueueCount").map(String::as_str),
            Some(count),
            "case {case:?}: the describe must report the queue count that was written, got \
             {interface:?}"
        );
    }

    let case = "defaultEnaQueueCount";
    modified(
        case,
        &router,
        &eni,
        &[
            param("Attachment.AttachmentId", &attachment_id),
            param("Attachment.DefaultEnaQueueCount", "true"),
        ],
    )
    .await;
    let interface = described(case, &router, &eni).await;
    assert_eq!(
        interface.get("attachment/enaQueueCount"),
        None,
        "case {case:?}: the default queue count is the attachment carrying none, got \
         {interface:?}"
    );
}

#[tokio::test]
async fn an_attachment_change_names_the_attachment_it_changes() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let (eni, attachment_id) = an_attached_interface(&router, &subnet_id).await;
    let (other_eni, other_attachment) = an_attached_interface(&router, &subnet_id).await;
    assert_ne!(
        attachment_id, other_attachment,
        "the two attachments of this fixture must differ"
    );

    // No id at all, a value this service never issued as one, and the id of ANOTHER
    // interface's attachment: the id has to name the attachment holding the interface the
    // request modifies.
    let rows: [(&str, Vec<Param>, &str); 4] = [
        (
            "no attachment id",
            vec![param("Attachment.DeleteOnTermination", "true")],
            MISSING_PARAMETER,
        ),
        (
            "an id of no kind",
            vec![
                param("Attachment.AttachmentId", "eni-attach-not-an-id"),
                param("Attachment.DeleteOnTermination", "true"),
            ],
            INVALID_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "an id this service never issued",
            vec![
                param("Attachment.AttachmentId", "eni-attach-0123456789abcdef0"),
                param("Attachment.DeleteOnTermination", "true"),
            ],
            INVALID_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "another interface's attachment",
            vec![
                param("Attachment.AttachmentId", &other_attachment),
                param("Attachment.DeleteOnTermination", "true"),
            ],
            INVALID_ATTACHMENT_ID_NOT_FOUND,
        ),
    ];
    for (case, changes, expected) in rows {
        let (status, body) = modify(&router, &eni, &changes).await;
        let (code, message) = refused(case, status, &body, 400);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with message {message:?}"
        );
    }
    for (case, interface_id) in [("the interface", &eni), ("the other interface", &other_eni)] {
        let interface = described(case, &router, interface_id).await;
        assert_eq!(
            interface
                .get("attachment/deleteOnTermination")
                .map(String::as_str),
            Some("false"),
            "case {case:?}: a refused attachment change must write nothing, got {interface:?}"
        );
    }

    // An attachment change that names no change to make, and the two queue members that
    // cannot be given together.
    let rows: [(&str, Vec<Param>, &str); 2] = [
        (
            "no change to make",
            vec![param("Attachment.AttachmentId", &attachment_id)],
            MISSING_PARAMETER,
        ),
        (
            "both queue members",
            vec![
                param("Attachment.AttachmentId", &attachment_id),
                param("Attachment.EnaQueueCount", "4"),
                param("Attachment.DefaultEnaQueueCount", "true"),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
    ];
    for (case, changes, expected) in rows {
        let (status, body) = modify(&router, &eni, &changes).await;
        let (code, message) = refused(case, status, &body, 400);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} with message {message:?}"
        );
    }
}

#[tokio::test]
async fn ena_express_is_configured_on_the_attachment_and_refused_without_one() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let (eni, _) = an_attached_interface(&router, &subnet_id).await;

    // Both settings the specification carries, each way round.
    let rows: [(&str, &str, &str); 3] = [
        ("enabled", "true", "false"),
        ("enabled for udp too", "true", "true"),
        ("disabled", "false", "false"),
    ];
    for (case, enabled, udp) in rows {
        modified(
            case,
            &router,
            &eni,
            &[
                param("EnaSrdSpecification.EnaSrdEnabled", enabled),
                param(
                    "EnaSrdSpecification.EnaSrdUdpSpecification.EnaSrdUdpEnabled",
                    udp,
                ),
            ],
        )
        .await;
        let interface = described(case, &router, &eni).await;
        assert_eq!(
            interface
                .get("attachment/enaSrdSpecification/enaSrdEnabled")
                .map(String::as_str),
            Some(enabled),
            "case {case:?}: the describe must report the ENA Express setting that was \
             written, got {interface:?}"
        );
        assert_eq!(
            interface
                .get("attachment/enaSrdSpecification/enaSrdUdpSpecification/enaSrdUdpEnabled")
                .map(String::as_str),
            Some(udp),
            "case {case:?}: the describe must report the UDP setting that was written, got \
             {interface:?}"
        );
    }

    // ENA Express is a setting of the ATTACHMENT, so an interface nothing holds has none.
    let case = "an interface nothing holds";
    let unattached = create_interface(&router, case, &subnet_id, &[]).await;
    let (status, body) = modify(
        &router,
        &unattached,
        &[param("EnaSrdSpecification.EnaSrdEnabled", "true")],
    )
    .await;
    let (code, message) = refused(case, status, &body, 400);
    assert_eq!(
        code, INVALID_PARAMETER_COMBINATION,
        "case {case:?}: expected {INVALID_PARAMETER_COMBINATION}, got {code} with message \
         {message:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. One attribute at a time, and the ids a change names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_must_name_exactly_one_attribute() {
    let (router, engine) = emulator();
    let (subnet_id, vpc_id) = a_default_subnet(&engine);
    let group = create_group(&router, "a group", &vpc_id, "one-at-a-time").await;
    let eni = create_interface(
        &router,
        "the interface",
        &subnet_id,
        &[param("Description", "as created")],
    )
    .await;

    let case = "no attribute at all";
    let (status, body) = modify(&router, &eni, &[]).await;
    let (code, message) = refused(case, status, &body, 400);
    assert_eq!(
        code, MISSING_PARAMETER,
        "case {case:?}: expected {MISSING_PARAMETER}, got {code} with message {message:?}"
    );
    for attribute in modify_op::MODIFIABLE_ATTRIBUTES {
        assert!(
            message.contains(attribute),
            "case {case:?}: the refusal must name the attributes a request may modify, \
             {attribute} is missing from {message:?}"
        );
    }

    // Pairs spanning the members: two plain attributes, a list beside a structure, and a
    // boolean beside a list.
    let pairs: [(&str, Vec<Param>); 3] = [
        (
            "a description and a check",
            vec![
                param("Description.Value", "two at once"),
                param("SourceDestCheck.Value", "false"),
            ],
        ),
        (
            "groups and a tracking specification",
            vec![
                param("SecurityGroupId.1", &group),
                param("ConnectionTrackingSpecification.UdpTimeout", "40"),
            ],
        ),
        (
            "a public address and a subnet association",
            vec![
                param("AssociatePublicIpAddress", "true"),
                param("AssociatedSubnetId.1", &subnet_id),
            ],
        ),
    ];
    for (case, changes) in pairs {
        let (status, body) = modify(&router, &eni, &changes).await;
        let (code, message) = refused(case, status, &body, 400);
        assert_eq!(
            code, INVALID_PARAMETER_COMBINATION,
            "case {case:?}: expected {INVALID_PARAMETER_COMBINATION}, got {code} with \
             message {message:?}"
        );
        assert_eq!(
            message,
            modify_op::ONE_ATTRIBUTE_AT_A_TIME,
            "case {case:?}: expected the one-attribute-at-a-time refusal, got {message:?}"
        );
    }

    let interface = described("after the refusals", &router, &eni).await;
    assert_eq!(
        interface.get("description").map(String::as_str),
        Some("as created"),
        "a refused modification must write nothing, got {interface:?}"
    );
    assert_eq!(
        interface.get("sourceDestCheck").map(String::as_str),
        Some("true"),
        "a refused modification must write nothing, got {interface:?}"
    );
    assert_eq!(
        interface.get("association/publicIp"),
        None,
        "a refused modification must write nothing, got {interface:?}"
    );
}

#[tokio::test]
async fn an_interface_id_is_read_as_a_form_first_and_looked_up_second() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    // One real interface, so the store is not empty when an absent id is looked up.
    create_interface(&router, "the interface", &subnet_id, &[]).await;

    let rows: [(&str, &str, &str); 3] = [
        (
            "not an interface id at all",
            MALFORMED_INTERFACE,
            INVALID_NETWORK_INTERFACE_ID_MALFORMED,
        ),
        (
            "a long id the account does not hold",
            ABSENT_INTERFACE,
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        ),
        (
            "a short id the account does not hold",
            ABSENT_SHORT_INTERFACE,
            INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        ),
    ];
    for (case, interface_id, expected) in rows {
        let (status, body) = modify(
            &router,
            interface_id,
            &[param("SourceDestCheck.Value", "false")],
        )
        .await;
        let (code, message) = refused(&format!("modify {case}"), status, &body, 400);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected} from a modification, got {code} with message \
             {message:?}"
        );
        let (status, body) = reset(&router, interface_id, &reset_source_dest_check()).await;
        let (code, message) = refused(&format!("reset {case}"), status, &body, 400);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected} from a reset, got {code} with message \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn an_interface_type_without_a_source_dest_check_cannot_have_one_changed() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);

    // The interface types a create may ask for (`NetworkInterfaceCreationType`: efa |
    // efa-only | branch | trunk) and the type an interface created without one carries, of
    // which only the EFA-only type has no source/destination check: "EFA-only network
    // interfaces do not support IP addresses", and the check is about the IP traffic an
    // interface carries.
    let rows: [(&str, Option<&str>, bool); 4] = [
        ("interface", None, true),
        ("efa", Some("efa"), true),
        ("branch", Some("branch"), true),
        (
            ec2::state::NETWORK_INTERFACE_TYPE_EFA_ONLY,
            Some(ec2::state::NETWORK_INTERFACE_TYPE_EFA_ONLY),
            false,
        ),
    ];
    for (interface_type, asked_for, has_check) in rows {
        let case = format!("an {interface_type} interface");
        let asked: Vec<Param> = asked_for
            .map(|interface_type| vec![param("InterfaceType", interface_type)])
            .unwrap_or_default();
        let eni = create_interface(&router, &case, &subnet_id, &asked).await;
        let modified_result = modify(&router, &eni, &[param("SourceDestCheck.Value", "false")]).await;
        let reset_result = reset(&router, &eni, &reset_source_dest_check()).await;
        if has_check {
            assert_eq!(
                modified_result.0, 200,
                "case {case:?}: a type with a source/destination check must take a change to \
                 it, got {} with body {}",
                modified_result.0, modified_result.1
            );
            assert_eq!(
                reset_result.0, 200,
                "case {case:?}: a type with a source/destination check must take a reset of \
                 it, got {} with body {}",
                reset_result.0, reset_result.1
            );
            continue;
        }
        for (action, (status, body)) in [("modify", modified_result), ("reset", reset_result)] {
            let (code, message) = refused(&case, status, &body, 400);
            assert_eq!(
                code, INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE} from a {action}, got \
                 {code} with message {message:?}"
            );
            assert_eq!(
                message,
                format!(
                    "Interface type: {interface_type} cannot modify source destination check"
                ),
                "case {case:?}: the refusal must name the interface's type, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. The reset, and the dry run that writes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_reset_puts_the_source_dest_check_back_to_its_default() {
    let (router, engine) = emulator();
    let (subnet_id, vpc_id) = a_default_subnet(&engine);
    let group = create_group(&router, "a group", &vpc_id, "reset-group").await;
    let default = ec2::state::DEFAULT_SOURCE_DEST_CHECK.to_string();

    // Whatever the attribute was left at — disabled, enabled, or never modified at all — a
    // reset leaves it at the default a create leaves it at.
    for start in [Some(false), Some(true), None] {
        let case = format!("starting from {start:?}");
        let eni = create_interface(
            &router,
            &case,
            &subnet_id,
            &[param("Description", "kept across a reset")],
        )
        .await;
        modified(&case, &router, &eni, &indexed("SecurityGroupId", &[&group])).await;
        if let Some(value) = start {
            modified(
                &case,
                &router,
                &eni,
                &[param("SourceDestCheck.Value", &value.to_string())],
            )
            .await;
        }
        let (status, body) = reset(&router, &eni, &reset_source_dest_check()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from a reset, got {status} with body {body}"
        );
        let interface = described(&case, &router, &eni).await;
        assert_eq!(
            interface.get("sourceDestCheck").map(String::as_str),
            Some(default.as_str()),
            "case {case:?}: a reset must put the check back to the default, got {interface:?}"
        );
        assert_eq!(
            described_attribute(
                &case,
                &router,
                &eni,
                describe_attribute_op::ATTRIBUTE_SOURCE_DEST_CHECK
            )
            .await
            .as_deref(),
            Some(default.as_str()),
            "case {case:?}: the attribute describe must report the same check"
        );
        // Every other attribute is left exactly as it stands.
        assert_eq!(
            interface.get("description").map(String::as_str),
            Some("kept across a reset"),
            "case {case:?}: a reset must leave the description it did not name, got \
             {interface:?}"
        );
        let groups: Vec<&str> = interface
            .iter()
            .filter(|(path, _)| path.starts_with("groupSet/item") && path.ends_with("groupId"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            groups,
            vec![group.as_str()],
            "case {case:?}: a reset must leave the groups it did not name, got {interface:?}"
        );
    }
}

#[tokio::test]
async fn a_reset_names_the_attribute_and_carries_no_value_for_it() {
    let (router, engine) = emulator();
    let (subnet_id, _) = a_default_subnet(&engine);
    let eni = create_interface(&router, "the interface", &subnet_id, &[]).await;
    modified(
        "disabled first",
        &router,
        &eni,
        &[param("SourceDestCheck.Value", "false")],
    )
    .await;

    // A request naming no attribute at all resets nothing.
    let case = "no attribute named";
    let (status, body) = reset(&router, &eni, &[]).await;
    let (code, message) = refused(case, status, &body, 400);
    assert_eq!(
        code, MISSING_PARAMETER,
        "case {case:?}: expected {MISSING_PARAMETER}, got {code} with message {message:?}"
    );

    // The parameter names the attribute by being there, so any value it carries is one the
    // parameter does not take — the attribute's own name included.
    for value in [
        describe_attribute_op::ATTRIBUTE_SOURCE_DEST_CHECK,
        describe_attribute_op::ATTRIBUTE_DESCRIPTION,
        "true",
        "bogus",
    ] {
        let case = format!("a value of {value:?}");
        let (status, body) = reset(&router, &eni, &[param("SourceDestCheck", value)]).await;
        let (code, message) = refused(&case, status, &body, 400);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {code} with message \
             {message:?}"
        );
        assert_eq!(
            message,
            reset_op::takes_no_value_message(value),
            "case {case:?}: expected the refusal this parameter words for a value, got \
             {message:?}"
        );
    }

    let interface = described("after the refusals", &router, &eni).await;
    assert_eq!(
        interface.get("sourceDestCheck").map(String::as_str),
        Some("false"),
        "a refused reset must leave the attribute where the modification left it, got \
         {interface:?}"
    );
}

#[tokio::test]
async fn a_dry_run_of_a_request_that_would_have_succeeded_writes_nothing() {
    let (router, engine) = emulator();
    let (subnet_id, vpc_id) = a_default_subnet(&engine);
    let group = create_group(&router, "a group", &vpc_id, "dry-run-group").await;
    let eni = create_interface(
        &router,
        "the interface",
        &subnet_id,
        &[param("Description", "as created")],
    )
    .await;
    let before = described("before the dry runs", &router, &eni).await;

    // One dry run per kind of change: text, a boolean, a set, a structure and a reset.
    let rows: [(&str, &str, Vec<Param>); 5] = [
        (
            "a description",
            "ModifyNetworkInterfaceAttribute",
            vec![param("Description.Value", "a dry run wrote this")],
        ),
        (
            "a check",
            "ModifyNetworkInterfaceAttribute",
            vec![param("SourceDestCheck.Value", "false")],
        ),
        (
            "a group set",
            "ModifyNetworkInterfaceAttribute",
            vec![param("SecurityGroupId.1", &group)],
        ),
        (
            "a tracking specification",
            "ModifyNetworkInterfaceAttribute",
            vec![param("ConnectionTrackingSpecification.UdpTimeout", "45")],
        ),
        (
            "a reset",
            "ResetNetworkInterfaceAttribute",
            vec![param("SourceDestCheck", "")],
        ),
    ];
    for (case, action, changes) in rows {
        let mut params = vec![
            param("NetworkInterfaceId", &eni),
            param("DryRun", "true"),
        ];
        params.extend(changes);
        let (status, body) = call(&router, REGION, action, &params).await;
        let (code, message) = refused(case, status, &body, 412);
        assert_eq!(
            code, DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION} from {action}, got {code} with \
             message {message:?}"
        );
    }

    assert_eq!(
        described("after the dry runs", &router, &eni).await,
        before,
        "a dry run must leave the interface exactly as it was"
    );
}
