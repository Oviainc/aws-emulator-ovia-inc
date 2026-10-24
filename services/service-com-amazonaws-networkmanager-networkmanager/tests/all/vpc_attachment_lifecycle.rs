// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateVpcAttachment`, driven over HTTP through the real router
//! (the harness lives in `crate::global_network_lifecycle`; the create and the reader this
//! suite drives are covered in their own right by `crate::attachment_lifecycle`).
//!
//! The rule under test: an update of a VPC attachment applies exactly the change it names —
//! the subnets it adds, the subnets it removes and the options it supplies — to the STORED
//! attachment and to nothing else, leaves the attachment `UPDATING` until the transition
//! the clock owes falls due, and is refused, storing nothing, when the change it names
//! cannot hold on that attachment.
//!
//! The tables below span that class: updates differing in how many subnets they add and
//! remove, in which order, whether they touch the options, and whether they ask for a
//! change at all; and refusals differing in which member is at fault and why. Nothing
//! reaches into `crate::state`: every attachment is read back through `GetVpcAttachment`.

use serde_json::{json, Value};

// The ARN shapes, the policy document and the global-network + core-network setup are the
// ones `crate::attachment_lifecycle` already owns, so the two suites cannot drift on them.
use crate::attachment_lifecycle::{core_network_at, subnet_arn, vpc_arn};
use crate::global_network_lifecycle::{error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE};

/// The Region the core networks here configure an edge location in, and so the Region the
/// attached VPCs and their subnets live in.
const REGION: &str = "us-west-2";

/// The members of `VpcOptions`, in the order [`options_of`] reports them (`smithy_query
/// shape=VpcOptions`: Ipv6Support, ApplianceModeSupport, DnsSupport,
/// SecurityGroupReferencingSupport, each a `Boolean` carrying a `@default` of `false`).
const OPTION_MEMBERS: [&str; 4] = [
    "Ipv6Support",
    "ApplianceModeSupport",
    "DnsSupport",
    "SecurityGroupReferencingSupport",
];

/// How many subnets one VPC attachment may hold: the bound
/// `state::MAX_SUBNETS_PER_VPC_ATTACHMENT` fixes, which is what keeps repeated updates from
/// growing one attachment's subnet list without end.
const MAX_SUBNETS: usize = 50;

/// An EC2 subnet ARN of [`REGION`]; see `crate::attachment_lifecycle::subnet_arn`.
fn subnet(index: usize) -> String {
    subnet_arn(REGION, index)
}

/// The subnet ARNs of `indices` inside the block `base` reserves for one case, so no two
/// cases name the same subnet.
fn block(base: usize, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| subnet(base + index)).collect()
}

/// The four `VpcOptions` flags an attachment reports. An absent member, like an absent
/// `Options`, is the modelled default (`false`), so both spellings of "off" read alike.
fn options_of(attachment: &Value) -> [bool; 4] {
    OPTION_MEMBERS.map(|member| attachment["Options"][member].as_bool().unwrap_or(false))
}

/// The `Options` body for the four flags, in the order [`OPTION_MEMBERS`] names them.
fn options_body(flags: [bool; 4]) -> Value {
    let mut body = json!({});
    for (member, flag) in OPTION_MEMBERS.iter().zip(flags) {
        body[*member] = json!(flag);
    }
    body
}

/// The subnet ARNs an attachment reports, in the order it reports them.
fn subnets_of(attachment: &Value) -> Vec<String> {
    attachment["SubnetArns"]
        .as_array()
        .map_or_else(Vec::new, |listed| {
            listed
                .iter()
                .map(|arn| arn.as_str().unwrap_or_default().to_string())
                .collect()
        })
}

/// The member a `ValidationException` blames, as this service reports it
/// (`ValidationException$Fields[0].Name`).
fn refused_member(body: &Value) -> &str {
    body["Fields"][0]["Name"].as_str().unwrap_or("<no Fields>")
}

/// A core network, in its own global network, whose policy configures one edge location in
/// [`REGION`], and the id it was minted with.
///
/// `crate::attachment_lifecycle::core_network_at` owns the setup (and the policy document
/// it configures the edge with); this wrapper keeps only the core network id, which is all
/// an update case needs. The document it builds carries no `attachment-policies`, so no
/// rule claims the attachments created on it and none of them waits for an acceptance.
async fn core_network_in_region(emulator: &Emulator, name: &str) -> String {
    let (_global_network, core_network) = core_network_at(emulator, name, &[REGION]).await;
    core_network
}

/// One row of the update table: what the attachment is created with, what the update asks
/// for, and what the attachment must then hold.
struct UpdateCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The subnets the create attaches, as indices into this row's subnet block.
    created: &'static [usize],
    /// The `Options` the create carries, as the four flags, or `None` for a create that
    /// names none.
    created_options: Option<[bool; 4]>,
    /// The subnet indices `AddSubnetArns` names, in order.
    add: &'static [usize],
    /// The subnet indices `RemoveSubnetArns` names, in order.
    remove: &'static [usize],
    /// The `Options` the update carries, or `None` for an update that names none.
    options: Option<[bool; 4]>,
    /// The subnet indices the attachment must hold afterwards, in order.
    expected: &'static [usize],
    /// The four flags it must then report.
    expected_options: [bool; 4],
}

/// The table: one row per form an accepted update takes.
const UPDATE_CASES: &[UpdateCase] = &[
    UpdateCase {
        name: "add one subnet to one",
        created: &[0],
        created_options: None,
        add: &[1],
        remove: &[],
        options: None,
        expected: &[0, 1],
        expected_options: [false, false, false, false],
    },
    UpdateCase {
        name: "remove the first of two",
        created: &[0, 1],
        created_options: None,
        add: &[],
        remove: &[0],
        options: None,
        expected: &[1],
        expected_options: [false, false, false, false],
    },
    UpdateCase {
        name: "add two and remove one, order preserved",
        created: &[0, 1],
        created_options: Some([true, false, false, false]),
        add: &[2, 3],
        remove: &[1],
        options: None,
        expected: &[0, 2, 3],
        expected_options: [true, false, false, false],
    },
    UpdateCase {
        name: "options only, every flag on",
        created: &[0],
        created_options: None,
        add: &[],
        remove: &[],
        options: Some([true, true, true, true]),
        expected: &[0],
        expected_options: [true, true, true, true],
    },
    UpdateCase {
        name: "options replace all four flags, not only the ones named",
        created: &[0, 1],
        created_options: Some([true, true, true, true]),
        add: &[],
        remove: &[],
        options: Some([false, false, true, false]),
        expected: &[0, 1],
        expected_options: [false, false, true, false],
    },
    UpdateCase {
        name: "subnets and options in one call",
        created: &[0, 1, 2],
        created_options: Some([false, true, false, false]),
        add: &[3],
        remove: &[0, 2],
        options: Some([false, false, false, true]),
        expected: &[1, 3],
        expected_options: [false, false, false, true],
    },
    UpdateCase {
        name: "empty add and remove lists, options named",
        created: &[0],
        created_options: None,
        add: &[],
        remove: &[],
        options: Some([false, true, false, false]),
        expected: &[0],
        expected_options: [false, true, false, false],
    },
];

#[tokio::test]
async fn an_update_applies_exactly_the_change_it_names_to_the_stored_attachment() {
    let emulator = Emulator::new();
    // Each entry: the case, its subnet block, the attachment it created and that create's
    // `CreatedAt`.
    let mut created: Vec<(&UpdateCase, usize, String, f64)> = Vec::new();

    for (row, case) in UPDATE_CASES.iter().enumerate() {
        let base = row * 100;
        let core_network_id = core_network_in_region(&emulator, case.name).await;
        let vpc = vpc_arn(REGION, base);
        let mut request = json!({
            "CoreNetworkId": core_network_id,
            "VpcArn": vpc,
            "SubnetArns": block(base, case.created),
            "Tags": [{ "Key": "case", "Value": case.name }],
        });
        if let Some(flags) = case.created_options {
            request["Options"] = options_body(flags);
        }
        let (status, body) = emulator.post("/vpc-attachments", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateVpcAttachment expected 200, got {status} with {body}",
            case.name
        );
        let created_attachment = &body["VpcAttachment"];
        let attachment = &created_attachment["Attachment"];
        let attachment_id = attachment["AttachmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no AttachmentId in {body}", case.name))
            .to_string();
        assert!(
            is_minted_id(&attachment_id, "attachment"),
            "case {:?}: expected an attachment-<hex> id, got {attachment_id:?}",
            case.name
        );
        assert_eq!(
            attachment["CoreNetworkArn"],
            json!(expected_arn("core-network", &core_network_id)),
            "case {:?}: expected the core network's ARN, got {attachment}",
            case.name
        );
        assert_eq!(
            attachment["State"],
            json!("CREATING"),
            "case {:?}: a just-created attachment is CREATING, got {attachment}",
            case.name
        );
        assert_eq!(
            subnets_of(created_attachment),
            block(base, case.created),
            "case {:?}: the create must store the subnets it was given, got {created_attachment}",
            case.name
        );
        let created_at = attachment["CreatedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: no CreatedAt in {attachment}", case.name));
        assert_eq!(
            attachment["UpdatedAt"].as_f64(),
            Some(created_at),
            "case {:?}: an attachment nothing has changed reports UpdatedAt = CreatedAt, got {attachment}",
            case.name
        );

        // The attachment's own creation is still in flight, so a change to it now would
        // leave it inconsistent: the modelled conflict, which clears by itself.
        let (status, refused) = emulator
            .patch(
                &format!("/vpc-attachments/{attachment_id}"),
                json!({ "AddSubnetArns": block(base, &[90]) }),
            )
            .await;
        assert_eq!(
            status, 409,
            "case {:?}: updating a CREATING attachment is a conflict, got {status} with {refused}",
            case.name
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {:?}: expected ConflictException, got {refused}",
            case.name
        );

        created.push((case, base, attachment_id, created_at));
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (case, base, attachment_id, created_at) in &created {
        // The transition the clock owed has fallen due, so the attachment is AVAILABLE and
        // the refused update above left it exactly as the create stored it.
        let (status, body) = emulator
            .get(&format!("/vpc-attachments/{attachment_id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetVpcAttachment expected 200, got {status} with {body}",
            case.name
        );
        let read = &body["VpcAttachment"];
        assert_eq!(
            read["Attachment"]["State"],
            json!("AVAILABLE"),
            "case {:?}: past the settle span the attachment is AVAILABLE, got {read}",
            case.name
        );
        assert_eq!(
            subnets_of(read),
            block(*base, case.created),
            "case {:?}: the refused update must have stored nothing, got {read}",
            case.name
        );

        let mut request = json!({});
        if !case.add.is_empty() {
            request["AddSubnetArns"] = json!(block(*base, case.add));
        }
        if !case.remove.is_empty() {
            request["RemoveSubnetArns"] = json!(block(*base, case.remove));
        }
        if let Some(flags) = case.options {
            request["Options"] = options_body(flags);
        }
        let (status, body) = emulator
            .patch(&format!("/vpc-attachments/{attachment_id}"), request)
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateVpcAttachment expected 200, got {status} with {body}",
            case.name
        );
        let updated = &body["VpcAttachment"];
        assert_eq!(
            subnets_of(updated),
            block(*base, case.expected),
            "case {:?}: expected the subnets the update leaves, got {updated}",
            case.name
        );
        assert_eq!(
            options_of(updated),
            case.expected_options,
            "case {:?}: expected the options the update leaves, got {updated}",
            case.name
        );
        assert_eq!(
            updated["Attachment"]["State"],
            json!("UPDATING"),
            "case {:?}: an accepted change leaves the attachment UPDATING, got {updated}",
            case.name
        );
        assert_eq!(
            updated["Attachment"]["CreatedAt"].as_f64(),
            Some(*created_at),
            "case {:?}: an update must not rewrite CreatedAt, got {updated}",
            case.name
        );
        assert!(
            updated["Attachment"]["UpdatedAt"]
                .as_f64()
                .is_some_and(|at| at > *created_at),
            "case {:?}: an accepted change advances UpdatedAt past CreatedAt, got {updated}",
            case.name
        );
        // Everything the update did not name is untouched.
        assert_eq!(
            updated["Attachment"]["ResourceArn"],
            json!(vpc_arn(REGION, *base)),
            "case {:?}: an update must not change the attached VPC, got {updated}",
            case.name
        );
        assert_eq!(
            updated["Attachment"]["EdgeLocation"],
            json!(REGION),
            "case {:?}: an update must not change the edge location, got {updated}",
            case.name
        );
        assert_eq!(
            updated["Attachment"]["Tags"],
            json!([{ "Key": "case", "Value": case.name }]),
            "case {:?}: an update must not change the tags, got {updated}",
            case.name
        );

        // The change was STORED, not echoed: the reader answers it, and twice in a row.
        let (_, first) = emulator
            .get(&format!("/vpc-attachments/{attachment_id}"))
            .await;
        let (_, second) = emulator
            .get(&format!("/vpc-attachments/{attachment_id}"))
            .await;
        assert_eq!(
            first, second,
            "case {:?}: GetVpcAttachment must be a pure read, got {first} then {second}",
            case.name
        );
        assert_eq!(
            subnets_of(&first["VpcAttachment"]),
            block(*base, case.expected),
            "case {:?}: the reader must answer the stored subnets, got {first}",
            case.name
        );
        assert_eq!(
            options_of(&first["VpcAttachment"]),
            case.expected_options,
            "case {:?}: the reader must answer the stored options, got {first}",
            case.name
        );
        assert_eq!(
            first["VpcAttachment"]["Attachment"]["State"],
            json!("UPDATING"),
            "case {:?}: the attachment is UPDATING until the transition falls due, got {first}",
            case.name
        );
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (case, base, attachment_id, _) in &created {
        let (status, body) = emulator
            .get(&format!("/vpc-attachments/{attachment_id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetVpcAttachment expected 200, got {status} with {body}",
            case.name
        );
        assert_eq!(
            body["VpcAttachment"]["Attachment"]["State"],
            json!("AVAILABLE"),
            "case {:?}: past the settle span the updated attachment is AVAILABLE, got {body}",
            case.name
        );
        assert_eq!(
            subnets_of(&body["VpcAttachment"]),
            block(*base, case.expected),
            "case {:?}: the settled attachment holds the updated subnets, got {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_update_that_asks_for_no_change_changes_nothing() {
    let emulator = Emulator::new();
    let core_network_id = core_network_in_region(&emulator, "no change").await;
    let (status, body) = emulator
        .post(
            "/vpc-attachments",
            json!({
                "CoreNetworkId": core_network_id,
                "VpcArn": vpc_arn(REGION, 1),
                "SubnetArns": [subnet(1)],
                "Options": options_body([true, false, true, false]),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {body}"
    );
    let attachment_id = body["VpcAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {body}"))
        .to_string();
    tokio::time::sleep(PAST_SETTLE).await;

    let (_, settled) = emulator
        .get(&format!("/vpc-attachments/{attachment_id}"))
        .await;

    // Each row asks for the state the attachment is already in: nothing to apply, so the
    // attachment is not moved to UPDATING and `UpdatedAt` does not advance.
    for (case, request) in [
        ("no member at all", json!({})),
        (
            "empty add and remove lists",
            json!({ "AddSubnetArns": [], "RemoveSubnetArns": [] }),
        ),
        (
            "the options it already has",
            json!({ "Options": options_body([true, false, true, false]) }),
        ),
    ] {
        let (status, body) = emulator
            .patch(&format!("/vpc-attachments/{attachment_id}"), request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateVpcAttachment expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["VpcAttachment"], settled["VpcAttachment"],
            "case {case:?}: an update that changes nothing must answer the settled attachment, got {body}"
        );
        let (_, read) = emulator
            .get(&format!("/vpc-attachments/{attachment_id}"))
            .await;
        assert_eq!(
            read["VpcAttachment"], settled["VpcAttachment"],
            "case {case:?}: an update that changes nothing must store nothing, got {read}"
        );
    }
}

#[tokio::test]
async fn an_update_that_cannot_hold_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let core_network_id = core_network_in_region(&emulator, "refusals").await;
    let held = vec![subnet(200), subnet(201)];
    let (status, body) = emulator
        .post(
            "/vpc-attachments",
            json!({
                "CoreNetworkId": core_network_id,
                "VpcArn": vpc_arn(REGION, 2),
                "SubnetArns": held,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {body}"
    );
    let attachment_id = body["VpcAttachment"]["Attachment"]["AttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no AttachmentId in {body}"))
        .to_string();
    tokio::time::sleep(PAST_SETTLE).await;

    // The create judges its own `SubnetArns` by the SAME shared rule, so a create naming
    // one subnet twice is refused on its own member and stores nothing.
    let (status, repeated) = emulator
        .post(
            "/vpc-attachments",
            json!({
                "CoreNetworkId": core_network_id,
                "VpcArn": vpc_arn(REGION, 3),
                "SubnetArns": [subnet(210), subnet(210)],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a create naming one subnet twice expected 400, got {status} with {repeated}"
    );
    assert_eq!(
        refused_member(&repeated),
        "subnetArns",
        "the create refusal must name its own member, got {repeated}"
    );

    // An attachment id no VPC attachment carries: the modelled 404, whatever the change
    // asks for.
    let (status, missing) = emulator
        .patch(
            "/vpc-attachments/attachment-0000bad1",
            json!({ "AddSubnetArns": [subnet(300)] }),
        )
        .await;
    assert_eq!(
        status, 404,
        "an unknown attachment id expected 404, got {status} with {missing}"
    );
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {missing}"
    );
    assert_eq!(
        missing["ResourceType"],
        json!("attachment"),
        "the refusal must name the resource type, got {missing}"
    );

    // Each row: the change, and the member the refusal must name. Every one breaks a rule
    // the stored attachment fixes, so every one must store nothing.
    let too_many: Vec<String> = (0..MAX_SUBNETS).map(|index| subnet(400 + index)).collect();
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "adding a subnet the attachment already holds",
            json!({ "AddSubnetArns": [subnet(201)] }),
            "addSubnetArns",
        ),
        (
            "removing a subnet the attachment does not hold",
            json!({ "RemoveSubnetArns": [subnet(202)] }),
            "removeSubnetArns",
        ),
        (
            "adding and removing the same subnet",
            json!({ "AddSubnetArns": [subnet(203)], "RemoveSubnetArns": [subnet(203)] }),
            "addSubnetArns",
        ),
        (
            "naming the same subnet twice in one list",
            json!({ "AddSubnetArns": [subnet(204), subnet(204)] }),
            "addSubnetArns",
        ),
        (
            "removing every subnet it holds",
            json!({ "RemoveSubnetArns": [subnet(200), subnet(201)] }),
            "removeSubnetArns",
        ),
        (
            "a subnet in another Region than the VPC",
            json!({ "AddSubnetArns": [subnet_arn("eu-west-1", 205)] }),
            "addSubnetArns",
        ),
        (
            "an empty subnet ARN, which the modelled pattern admits",
            json!({ "AddSubnetArns": [""] }),
            "addSubnetArns",
        ),
        (
            "one subnet past the bound on how many an attachment holds",
            json!({ "AddSubnetArns": too_many }),
            "addSubnetArns",
        ),
    ];

    for (case, request, member) in cases {
        let (status, refused) = emulator
            .patch(&format!("/vpc-attachments/{attachment_id}"), request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused_member(&refused),
            member,
            "case {case:?}: the refusal must name the offending member, got {refused}"
        );
        let (_, read) = emulator
            .get(&format!("/vpc-attachments/{attachment_id}"))
            .await;
        assert_eq!(
            subnets_of(&read["VpcAttachment"]),
            held,
            "case {case:?}: a refused update must store nothing, got {read}"
        );
        assert_eq!(
            read["VpcAttachment"]["Attachment"]["State"],
            json!("AVAILABLE"),
            "case {case:?}: a refused update must not move the attachment, got {read}"
        );
    }

    // The bound itself is reachable: the row above minus the subnets the attachment already
    // holds fills it exactly to the bound, and is accepted.
    let to_the_bound: Vec<String> = (0..MAX_SUBNETS - held.len())
        .map(|index| subnet(400 + index))
        .collect();
    let (status, accepted) = emulator
        .patch(
            &format!("/vpc-attachments/{attachment_id}"),
            json!({ "AddSubnetArns": to_the_bound }),
        )
        .await;
    assert_eq!(
        status, 200,
        "filling the attachment exactly to the bound is accepted, got {status} with {accepted}"
    );
    assert_eq!(
        subnets_of(&accepted["VpcAttachment"]).len(),
        MAX_SUBNETS,
        "the attachment must hold exactly the bound, got {accepted}"
    );
}
