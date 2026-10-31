// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeNetworkInterfaceAttribute`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation reports, for the ONE
//! `NetworkInterfaceAttribute` its request names, the value the named interface holds in
//! the store — and no other attribute, "you can specify only one attribute at a time" —
//! refusing a request that names no attribute, one whose interface id is not an interface
//! id at all, and one naming an interface the account does not hold.
//!
//! So the tables below span that class rather than one recorded request: every member of
//! the model's own `NetworkInterfaceAttribute` catalog, against interfaces that differ
//! along what each attribute is about — description (absent, one word, a sentence with
//! punctuation), security groups (the VPC default, one named, two named), address count,
//! interface type, whether an attachment holds the interface and at which device index,
//! and which owner the public IPv4 address it is associated with has. Each value is
//! cross-checked against what `DescribeNetworkInterfaces` reports for the same interface,
//! so the two reports of one interface cannot disagree, and each attribute is also read
//! back after the STORE is moved underneath it, so no reported value can be a constant.

use std::collections::BTreeMap;

use ec2::ops::describe_network_interface_attribute as attribute_op;
use ec2::ops::types::{NetworkInterfaceAssociation, NetworkInterfaceAttachment};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, indexed};
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region every call here is signed for: the default VPC and its per-zone subnets are
/// seeded for the emulator's default Region, and an interface is created in one of them.
const REGION: &str = "us-east-1";

/// `description`, the attribute a create sets and a describe reports.
const DESCRIPTION: &str = attribute_op::ATTRIBUTE_DESCRIPTION;

/// `groupSet`, the security groups of the interface.
const GROUP_SET: &str = attribute_op::ATTRIBUTE_GROUP_SET;

/// `sourceDestCheck`, the boolean attribute an interface is created with on.
const SOURCE_DEST_CHECK: &str = attribute_op::ATTRIBUTE_SOURCE_DEST_CHECK;

/// `attachment`, the attachment (if any) that holds the interface.
const ATTACHMENT: &str = attribute_op::ATTRIBUTE_ATTACHMENT;

/// `associatePublicIpAddress`, whether AWS assigns the interface a public IPv4 address.
const ASSOCIATE_PUBLIC_IP_ADDRESS: &str = attribute_op::ATTRIBUTE_ASSOCIATE_PUBLIC_IP_ADDRESS;

/// Every attribute a network interface has. Pinned against the model's own
/// `NetworkInterfaceAttribute` catalog by [`the_attribute_catalog_is_the_models_own`], so a
/// member the model gains cannot be left untested here.
const ATTRIBUTES: [&str; 5] = attribute_op::ATTRIBUTE_NAMES;

/// A well-formed network interface id, in the long form, that no interface of the account
/// has.
const ABSENT_INTERFACE: &str = "eni-0000000000000beef";

/// A well-formed interface id in the SHORT form (8 hex digits), which no interface of the
/// account has either — the id form is part of what "well formed" means here.
const ABSENT_SHORT_INTERFACE: &str = "eni-0badf00d";

/// A value that is not a network interface id at all.
const MALFORMED_INTERFACE: &str = "eni-not-an-id";

/// The error code the centralized request walk answers for an input that breaks a MODELLED
/// constraint — here an `Attribute` outside the `NetworkInterfaceAttribute` enum, which is
/// refused before any handler runs, and so carries the boundary's own code rather than one
/// a handler chose.
const BOUNDARY_CONSTRAINT_CODE: &str = "ValidationError";

/// The instant the seeded attachments were made, as a fixed point so `attachTime` has a
/// value to compare against. It is a seeded fact, not something the emulator mints.
const SEEDED_ATTACH_TIME_SECS: i64 = 1_767_225_600;

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The `SubnetId` of a subnet of the account's default VPC.
fn a_default_subnet(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .subnets
        .keys()
        .next()
        .cloned()
        .expect("a fresh account has the default VPC's subnets")
}

/// The `VpcId` of the account's default VPC.
fn default_vpc_of(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC")
}

/// One `CreateSecurityGroup` in `vpc_id`, as the group id it minted.
async fn create_group(router: &MultiServiceRouter, case: &str, vpc_id: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("VpcId", vpc_id),
            param("GroupName", name),
            param(
                "GroupDescription",
                "a group the interface attribute suite created",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} with \
         body {body}"
    );
    super::ec2_query::element(&body, "groupId")
        .unwrap_or_else(|| panic!("case {case:?}: no <groupId> in the CreateSecurityGroup {body}"))
}

/// One `CreateNetworkInterface` in `subnet_id` carrying `params`, as the interface id it
/// minted.
async fn create_interface(
    router: &MultiServiceRouter,
    case: &str,
    subnet_id: &str,
    params: &[Param],
) -> String {
    let mut fields = vec![param("SubnetId", subnet_id)];
    fields.extend(params.iter().cloned());
    let (status, body) = call(router, REGION, "CreateNetworkInterface", &fields).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateNetworkInterface, got {status} with \
         body {body}"
    );
    super::ec2_query::element(&body, "networkInterfaceId").unwrap_or_else(|| {
        panic!("case {case:?}: no <networkInterfaceId> in the CreateNetworkInterface {body}")
    })
}

/// One `DescribeNetworkInterfaceAttribute` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeNetworkInterfaceAttribute", params).await
}

/// The form fields of a describe of `attribute` of `network_interface_id`.
fn describe_params(network_interface_id: &str, attribute: &str) -> Vec<Param> {
    vec![
        param("NetworkInterfaceId", network_interface_id),
        param("Attribute", attribute),
    ]
}

/// The body of a successful describe of `attribute` of `network_interface_id`, required to
/// name the interface it describes and to carry NO attribute but the one asked for ("you
/// can specify only one attribute at a time").
async fn read_attribute(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
    attribute: &str,
) -> String {
    let (status, body) = describe(router, &describe_params(network_interface_id, attribute)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from a describe of {attribute}, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let root = parsed.root_element();
    assert_eq!(
        root.tag_name().name(),
        "DescribeNetworkInterfaceAttributeResponse",
        "case {case:?}: expected the operation's own response envelope, got {body}"
    );
    assert_eq!(
        child_text(root, "networkInterfaceId").as_deref(),
        Some(network_interface_id),
        "case {case:?}: a describe of {attribute} must name the interface it describes, \
         body {body}"
    );
    for other in ATTRIBUTES.iter().filter(|other| **other != attribute) {
        assert!(
            child(root, other).is_none(),
            "case {case:?}: a describe of {attribute} must not report {other}, body {body}"
        );
    }
    body
}

/// The `description` of an interface, as the describe reports it: `None` when the
/// attribute carries no `value` element at all.
async fn read_description(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Option<String> {
    let body = read_attribute(router, case, network_interface_id, DESCRIPTION).await;
    let parsed = document(case, &body);
    let attribute = child(parsed.root_element(), DESCRIPTION)
        .unwrap_or_else(|| panic!("case {case:?}: no <{DESCRIPTION}> in {body}"));
    child_text(attribute, "value")
}

/// The security groups of an interface, as `(groupId, groupName)` pairs in the order the
/// describe reported them.
async fn read_groups(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Vec<(String, String)> {
    let body = read_attribute(router, case, network_interface_id, GROUP_SET).await;
    let parsed = document(case, &body);
    let set = child(parsed.root_element(), GROUP_SET)
        .unwrap_or_else(|| panic!("case {case:?}: no <{GROUP_SET}> in {body}"));
    group_pairs(set)
}

/// The `(groupId, groupName)` pairs of a `groupSet` element, whichever response it came
/// from — so the attribute describe and `DescribeNetworkInterfaces` are read one way.
fn group_pairs(set: roxmltree::Node<'_, '_>) -> Vec<(String, String)> {
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "groupId").unwrap_or_default(),
                child_text(item, "groupName").unwrap_or_default(),
            )
        })
        .collect()
}

/// The `sourceDestCheck` of an interface, as the describe reports it.
async fn read_source_dest_check(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Option<bool> {
    let body = read_attribute(router, case, network_interface_id, SOURCE_DEST_CHECK).await;
    let parsed = document(case, &body);
    let attribute = child(parsed.root_element(), SOURCE_DEST_CHECK)
        .unwrap_or_else(|| panic!("case {case:?}: no <{SOURCE_DEST_CHECK}> in {body}"));
    child_text(attribute, "value").map(|value| as_boolean(case, &value))
}

/// The members of the `attachment` an interface reports, or `None` when it reports no
/// attachment at all — "the attachment (if any) of the network interface".
async fn read_attachment(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Option<BTreeMap<String, String>> {
    let body = read_attribute(router, case, network_interface_id, ATTACHMENT).await;
    let parsed = document(case, &body);
    let attachment = child(parsed.root_element(), ATTACHMENT)?;
    Some(
        attachment
            .children()
            .filter(|node| node.is_element())
            .map(|node| {
                (
                    node.tag_name().name().to_string(),
                    node.text().unwrap_or_default().to_string(),
                )
            })
            .collect(),
    )
}

/// The `associatePublicIpAddress` of an interface, as the describe reports it.
async fn read_associate_public_ip_address(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> Option<bool> {
    let body = read_attribute(router, case, network_interface_id, ASSOCIATE_PUBLIC_IP_ADDRESS).await;
    let parsed = document(case, &body);
    child_text(parsed.root_element(), ASSOCIATE_PUBLIC_IP_ADDRESS)
        .map(|value| as_boolean(case, &value))
}

/// A boolean as the ec2Query wire carries one.
fn as_boolean(case: &str, value: &str) -> bool {
    match value {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: expected a boolean on the wire, got {other:?}"),
    }
}

/// What `DescribeNetworkInterfaces` reports for ONE interface: its description, its
/// security groups and its source/destination check, as the attribute describe must agree
/// with them.
async fn described_interface(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
) -> (Option<String>, Vec<(String, String)>, Option<bool>) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeNetworkInterfaces, got {status} \
         with body {body}"
    );
    let parsed = document(case, &body);
    let item = set_items(parsed.root_element(), "networkInterfaceSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no interface in the describe body {body}"));
    let groups = child(item, "groupSet").map(group_pairs).unwrap_or_default();
    (
        child_text(item, "description"),
        groups,
        child_text(item, "sourceDestCheck").map(|value| as_boolean(case, &value)),
    )
}

/// Every attribute of one interface, asserted against the values the caller expects and
/// against what `DescribeNetworkInterfaces` reports for the same interface.
async fn assert_attributes(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
    description: &str,
    source_dest_check: bool,
    associate_public_ip_address: bool,
    attached: bool,
) {
    let (described_description, described_groups, described_check) =
        described_interface(router, case, network_interface_id).await;

    let actual = read_description(router, case, network_interface_id).await;
    assert_eq!(
        actual.as_deref(),
        Some(description),
        "case {case:?}: expected the {DESCRIPTION} of {network_interface_id} to be \
         {description:?}, got {actual:?}"
    );
    assert_eq!(
        actual, described_description,
        "case {case:?}: the {DESCRIPTION} attribute must agree with the one \
         DescribeNetworkInterfaces reports, got {actual:?} against {described_description:?}"
    );

    let actual = read_groups(router, case, network_interface_id).await;
    assert_eq!(
        actual, described_groups,
        "case {case:?}: the {GROUP_SET} attribute must be the groups \
         DescribeNetworkInterfaces reports for {network_interface_id}, got {actual:?} \
         against {described_groups:?}"
    );

    let actual = read_source_dest_check(router, case, network_interface_id).await;
    assert_eq!(
        actual,
        Some(source_dest_check),
        "case {case:?}: expected the {SOURCE_DEST_CHECK} of {network_interface_id} to be \
         {source_dest_check}, got {actual:?}"
    );
    assert_eq!(
        actual, described_check,
        "case {case:?}: the {SOURCE_DEST_CHECK} attribute must agree with the one \
         DescribeNetworkInterfaces reports, got {actual:?} against {described_check:?}"
    );

    let actual = read_associate_public_ip_address(router, case, network_interface_id).await;
    assert_eq!(
        actual,
        Some(associate_public_ip_address),
        "case {case:?}: expected the {ASSOCIATE_PUBLIC_IP_ADDRESS} of \
         {network_interface_id} to be {associate_public_ip_address}, got {actual:?}"
    );

    let actual = read_attachment(router, case, network_interface_id).await;
    assert_eq!(
        actual.is_some(),
        attached,
        "case {case:?}: expected {network_interface_id} to report an {ATTACHMENT} only \
         when something holds it (attached: {attached}), got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// The catalog of attributes
// ---------------------------------------------------------------------------

/// The attribute names this suite and the handler use are the model's own
/// `NetworkInterfaceAttribute` members — no more and no fewer.
#[tokio::test]
async fn the_attribute_catalog_is_the_models_own() {
    let mut used = ATTRIBUTES.to_vec();
    used.sort_unstable();
    let mut modelled = ec2::ops::types::NETWORK_INTERFACE_ATTRIBUTE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        used, modelled,
        "case \"NetworkInterfaceAttribute catalog\": the attributes under test must be \
         the model's own members, got {used:?} against {modelled:?}"
    );
}

// ---------------------------------------------------------------------------
// What a created interface reports
// ---------------------------------------------------------------------------

/// Every attribute of an interface a caller created is the value the STORE holds for it:
/// the description the create asked for (the empty one AWS gives an interface created
/// without one), the security groups it resolved, source/destination checking on ("the
/// default value is true"), no attachment, and no AWS-assigned public IPv4 address.
///
/// The rows differ along what the attributes are about — the description's length and
/// content, how many security groups the interface references, how many private addresses
/// it holds and which interface type it is — so no row is the specification.
#[tokio::test]
async fn each_attribute_reports_what_the_created_interface_holds() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);
    let vpc_id = default_vpc_of(&engine);
    let first = create_group(&router, "the group table", &vpc_id, "attribute-suite-one").await;
    let second = create_group(&router, "the group table", &vpc_id, "attribute-suite-two").await;

    // (case, the extra form fields of the create, the description it must report)
    let cases: [(&str, Vec<Param>, &str); 8] = [
        ("no optional member", Vec::new(), ""),
        (
            "a one-word description",
            vec![param("Description", "web")],
            "web",
        ),
        (
            "a description with spaces and punctuation",
            vec![param("Description", "eth0: the batch tier (primary)")],
            "eth0: the batch tier (primary)",
        ),
        (
            "one named security group",
            vec![param("SecurityGroupId.1", &first)],
            "",
        ),
        (
            "two named security groups",
            vec![
                param("SecurityGroupId.1", &first),
                param("SecurityGroupId.2", &second),
                param("Description", "two groups"),
            ],
            "two groups",
        ),
        (
            "three secondary addresses",
            vec![param("SecondaryPrivateIpAddressCount", "3")],
            "",
        ),
        (
            "an efa interface",
            vec![param("InterfaceType", "efa"), param("Description", "efa")],
            "efa",
        ),
        (
            // "If you specify efa-only, do not assign any IP addresses to the network
            // interface" (`CreateNetworkInterfaceRequest$InterfaceType`), so this row holds
            // no private address at all.
            "an efa-only interface",
            vec![param("InterfaceType", "efa-only")],
            "",
        ),
    ];

    for (case, params, description) in cases {
        let network_interface_id = create_interface(&router, case, &subnet_id, &params).await;
        assert_attributes(
            &router,
            case,
            &network_interface_id,
            description,
            true,
            false,
            false,
        )
        .await;
    }
}

/// The attributes of one interface are ITS OWN: interfaces created side by side each
/// report their own description and groups, and reading one says nothing about another.
#[tokio::test]
async fn each_interface_reports_its_own_attributes() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);
    let vpc_id = default_vpc_of(&engine);
    let group = create_group(&router, "the neighbours", &vpc_id, "attribute-suite-neighbour").await;

    // (case, the description the interface is created with, the groups it names)
    let cases: [(&str, &str, Vec<Param>); 3] = [
        ("the first neighbour", "first", Vec::new()),
        (
            "the second neighbour",
            "second",
            vec![param("SecurityGroupId.1", &group)],
        ),
        ("the third neighbour", "", Vec::new()),
    ];

    let mut created: Vec<(&str, &str, String)> = Vec::new();
    for (case, description, groups) in cases {
        let mut params = groups;
        if !description.is_empty() {
            params.push(param("Description", description));
        }
        let network_interface_id = create_interface(&router, case, &subnet_id, &params).await;
        created.push((case, description, network_interface_id));
    }

    for (case, description, network_interface_id) in &created {
        assert_attributes(
            &router,
            case,
            network_interface_id,
            description,
            true,
            false,
            false,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// The attributes follow the store
// ---------------------------------------------------------------------------

/// `sourceDestCheck` is read from the interface the store holds, not fixed: an interface
/// whose stored check is moved reports the moved value, and the interface beside it does
/// not move with it.
///
/// The rows are both values the attribute can hold, applied to one of two interfaces, with
/// the expectation for BOTH computed from the steps so far.
#[tokio::test]
async fn source_dest_check_is_the_value_the_stored_interface_holds() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);
    let interfaces = [
        create_interface(&router, "the first interface", &subnet_id, &[]).await,
        create_interface(&router, "the second interface", &subnet_id, &[]).await,
    ];

    /// Move the stored `sourceDestCheck` of one interface, the way a modification would.
    fn store_check(engine: &engine::InMemoryEngine, network_interface_id: &str, value: bool) {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let interface = store
            .network_interfaces
            .get_mut(network_interface_id)
            .expect("the interface the test created");
        interface.source_dest_check = Some(value);
    }

    // (case, which interface the step moves, the value it moves it to)
    let steps = [
        ("the second interface off", 1, false),
        ("the second interface off again", 1, false),
        ("the first interface off", 0, false),
        ("the second interface back on", 1, true),
    ];

    // "Indicates whether source/destination checking is enabled […] The default value is
    // true" (`ModifyNetworkInterfaceAttributeRequest$SourceDestCheck`), which is what both
    // interfaces start at.
    let mut expected = [true, true];
    for (case, index, value) in steps {
        store_check(&engine, &interfaces[index], value);
        expected[index] = value;
        for (position, network_interface_id) in interfaces.iter().enumerate() {
            let case = format!("{case}: interface {position}");
            let actual = read_source_dest_check(&router, &case, network_interface_id).await;
            assert_eq!(
                actual,
                Some(expected[position]),
                "case {case:?}: expected the {SOURCE_DEST_CHECK} of {network_interface_id} \
                 to be {}, got {actual:?}",
                expected[position]
            );
        }
    }
}

/// The `attachment` attribute reports the attachment that holds the interface, member for
/// member, and reports nothing at all for an interface attached to nothing.
///
/// The rows differ along the attachment's device index, its status, whether it is deleted
/// on termination and which instance holds it, so no member of the one attachment is the
/// specification.
#[tokio::test]
async fn the_attachment_attribute_reports_the_attachment_that_holds_the_interface() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);

    // (case, device index, attachment status, delete on termination, network card index)
    let cases: [(&str, i32, &str, bool, i32); 3] = [
        ("the primary attachment", 0, "attached", true, 0),
        ("a secondary attachment", 1, "attached", false, 0),
        ("an attachment still attaching", 2, "attaching", true, 1),
    ];

    for (case, device_index, status, delete_on_termination, network_card_index) in cases {
        let network_interface_id = create_interface(&router, case, &subnet_id, &[]).await;

        // An interface nothing holds reports no attachment: "the attachment (if any)".
        let before = read_attachment(&router, case, &network_interface_id).await;
        assert!(
            before.is_none(),
            "case {case:?}: an interface attached to nothing must report no {ATTACHMENT}, \
             got {before:?}"
        );

        // Attach it the way an attaching operation would: the attachment on the record,
        // and the `in-use` status that follows from something holding the interface.
        let (attachment_id, instance_id, owner_id) = {
            let state = engine.state::<Ec2State>();
            let mut store = state.lock();
            let attachment_id = store.next_id("eni-attach");
            let instance_id = store.mint_instance_id();
            let owner_id = engine::emulator_account_id().to_string();
            let interface = store
                .network_interfaces
                .get_mut(&network_interface_id)
                .expect("the interface the test created");
            interface.status = Some("in-use".to_string());
            interface.attachment = Some(NetworkInterfaceAttachment {
                attach_time: Some(::aws_smithy_types::DateTime::from_secs(
                    SEEDED_ATTACH_TIME_SECS,
                )),
                attachment_id: Some(attachment_id.clone()),
                delete_on_termination: Some(delete_on_termination),
                device_index: Some(device_index),
                network_card_index: Some(network_card_index),
                instance_id: Some(instance_id.clone()),
                instance_owner_id: Some(owner_id.clone()),
                status: Some(status.to_string()),
                ena_srd_specification: None,
                ena_queue_count: None,
            });
            (attachment_id, instance_id, owner_id)
        };

        let reported = read_attachment(&router, case, &network_interface_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: an attached interface must report its {ATTACHMENT}"));
        // (the member, the value the store holds for it)
        let expected = [
            ("attachmentId", attachment_id.clone()),
            ("instanceId", instance_id.clone()),
            ("instanceOwnerId", owner_id.clone()),
            ("deviceIndex", device_index.to_string()),
            ("networkCardIndex", network_card_index.to_string()),
            ("status", status.to_string()),
            (
                "deleteOnTermination",
                delete_on_termination.to_string(),
            ),
        ];
        for (member, value) in expected {
            let actual = reported.get(member);
            assert_eq!(
                actual,
                Some(&value),
                "case {case:?}: expected the {ATTACHMENT}'s {member} to be {value:?}, \
                 got {actual:?} in {reported:?}"
            );
        }
        let attach_time = reported.get("attachTime").cloned().unwrap_or_default();
        let parsed = super::ec2_query::as_timestamp(&attach_time);
        assert_eq!(
            parsed,
            Some(SEEDED_ATTACH_TIME_SECS as f64),
            "case {case:?}: expected the {ATTACHMENT}'s attachTime to be the instant the \
             store holds, got {attach_time:?}"
        );
    }
}

/// `associatePublicIpAddress` follows the public IPv4 address the interface holds: an
/// interface associated with an address AWS assigned reports `true`, and one holding an
/// Elastic IP of the account, or no public address at all, reports `false`.
///
/// The rows are those three states of one interface, applied in sequence, so each report
/// is the store's current answer rather than the state it was created in.
#[tokio::test]
async fn associate_public_ip_address_follows_the_public_address_the_interface_holds() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);
    let network_interface_id = create_interface(&router, "the addressed interface", &subnet_id, &[])
        .await;
    let account = engine::emulator_account_id().to_string();

    /// Put the public IPv4 association of one interface where `ip_owner_id` says.
    fn store_association(
        engine: &engine::InMemoryEngine,
        network_interface_id: &str,
        ip_owner_id: Option<&str>,
    ) {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let interface = store
            .network_interfaces
            .get_mut(network_interface_id)
            .expect("the interface the test created");
        interface.association = ip_owner_id.map(|owner| NetworkInterfaceAssociation {
            allocation_id: None,
            association_id: Some("eipassoc-00000000000000001".to_string()),
            ip_owner_id: Some(owner.to_string()),
            public_dns_name: None,
            public_ip: Some("198.51.100.7".to_string()),
            customer_owned_ip: None,
            carrier_ip: None,
        });
    }

    // (case, the owner of the public address the interface holds, what it must report)
    let cases: [(&str, Option<&str>, bool); 5] = [
        ("no public address at all", None, false),
        (
            "a public address AWS assigned",
            Some(attribute_op::AMAZON_ASSIGNED_IP_OWNER_ID),
            true,
        ),
        ("an Elastic IP of the account", Some(&account), false),
        (
            "an AWS-assigned address again",
            Some(attribute_op::AMAZON_ASSIGNED_IP_OWNER_ID),
            true,
        ),
        ("the address released again", None, false),
    ];

    for (case, owner, expected) in cases {
        store_association(&engine, &network_interface_id, owner);
        let actual = read_associate_public_ip_address(&router, case, &network_interface_id).await;
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: expected the {ASSOCIATE_PUBLIC_IP_ADDRESS} of \
             {network_interface_id} to be {expected}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A describe is a read
// ---------------------------------------------------------------------------

/// Describing an attribute changes nothing a later caller sees: every attribute read in
/// turn, three times over, reports the same value, and the interface the store holds is
/// untouched by any of it.
#[tokio::test]
async fn describing_an_attribute_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);
    let case = "an interface read repeatedly";
    let network_interface_id =
        create_interface(&router, case, &subnet_id, &[param("Description", "read")]).await;

    let before = format!(
        "{:?}",
        engine
            .state::<Ec2State>()
            .lock()
            .network_interfaces
            .get(&network_interface_id)
    );
    for round in 0..3 {
        let case = format!("{case}: round {round}");
        assert_attributes(&router, &case, &network_interface_id, "read", true, false, false).await;
    }
    let after = format!(
        "{:?}",
        engine
            .state::<Ec2State>()
            .lock()
            .network_interfaces
            .get(&network_interface_id)
    );
    assert_eq!(
        before, after,
        "case {case:?}: describing an attribute must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// Every refusal this operation makes, each with the EC2 error code and HTTP status it
/// answers.
///
/// The rows differ along what makes the request bad: no attribute at all, an attribute
/// outside the model's `NetworkInterfaceAttribute` enum, one belonging to another
/// resource, an interface id that is not shaped like one, a well-formed id (long and
/// short form) no interface of the account has, and a dry run of a request that would
/// have succeeded.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let (router, engine) = emulator();
    let subnet_id = a_default_subnet(&engine);
    let case = "the refusal table's interface";
    let network_interface_id =
        create_interface(&router, case, &subnet_id, &[param("Description", "kept")]).await;

    // (case, the form fields, the HTTP status, the EC2 error code)
    //
    // The two rows naming an attribute outside the model's enum expect
    // [`BOUNDARY_CONSTRAINT_CODE`]: a modelled constraint is refused by the centralized
    // request walk (deserialize -> validate -> handle), before the handler runs.
    let cases: [(&str, Vec<Param>, u16, &str); 10] = [
        (
            "no attribute at all",
            vec![param("NetworkInterfaceId", &network_interface_id)],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "an attribute the model does not declare",
            describe_params(&network_interface_id, "sourcedestcheck"),
            400,
            BOUNDARY_CONSTRAINT_CODE,
        ),
        (
            "an attribute of another resource",
            describe_params(
                &network_interface_id,
                ec2::state::VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT,
            ),
            400,
            BOUNDARY_CONSTRAINT_CODE,
        ),
        (
            "an interface id that is not shaped like one",
            describe_params(MALFORMED_INTERFACE, DESCRIPTION),
            400,
            ec2::state::INVALID_NETWORK_INTERFACE_ID_MALFORMED,
        ),
        (
            "an empty interface id",
            describe_params("", GROUP_SET),
            400,
            ec2::state::INVALID_NETWORK_INTERFACE_ID_MALFORMED,
        ),
        (
            "a long-form id no interface of the account has",
            describe_params(ABSENT_INTERFACE, DESCRIPTION),
            400,
            ec2::state::INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        ),
        (
            "a short-form id no interface of the account has",
            describe_params(ABSENT_SHORT_INTERFACE, ATTACHMENT),
            400,
            ec2::state::INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        ),
        (
            "a dry run of a request that would have succeeded",
            {
                let mut params = describe_params(&network_interface_id, SOURCE_DEST_CHECK);
                params.push(param("DryRun", "true"));
                params
            },
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        // A dry run is the PERMISSION question, answered for a request that would have
        // succeeded — so a request naming an interface that is not there, or an id that is
        // not an id, is still refused for THAT, the ordering `DeleteNetworkInterface`
        // keeps for an interface id.
        (
            "a dry run naming an interface that is not there",
            {
                let mut params = describe_params(ABSENT_INTERFACE, DESCRIPTION);
                params.push(param("DryRun", "true"));
                params
            },
            400,
            ec2::state::INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        ),
        (
            "a dry run naming an id that is not an id",
            {
                let mut params = describe_params(MALFORMED_INTERFACE, DESCRIPTION);
                params.push(param("DryRun", "true"));
                params
            },
            400,
            ec2::state::INVALID_NETWORK_INTERFACE_ID_MALFORMED,
        ),
    ];

    for (case, params, status_expected, code_expected) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?}, got {code:?} with \
             body {body}"
        );
    }

    // None of those refusals wrote anything: the interface they named still reports every
    // attribute it was created with.
    assert_attributes(
        &router,
        "after every refusal",
        &network_interface_id,
        "kept",
        true,
        false,
        false,
    )
    .await;
}

/// A well-formed id no interface of the account has is refused naming THAT id, so a caller
/// can tell which of the ids it sent was not found.
///
/// The rows are ids of both well-formed lengths, so the wording is the template and not
/// one id's sentence.
#[tokio::test]
async fn a_missing_interface_is_refused_naming_the_id_the_request_sent() {
    let (router, _engine) = emulator();
    for network_interface_id in [ABSENT_INTERFACE, ABSENT_SHORT_INTERFACE, "eni-00000000"] {
        let case = format!("the absent interface {network_interface_id}");
        let (status, body) = describe(
            &router,
            &describe_params(network_interface_id, DESCRIPTION),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = error_message(&body);
        let expected =
            format!("The networkInterface ID '{network_interface_id}' does not exist");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}
