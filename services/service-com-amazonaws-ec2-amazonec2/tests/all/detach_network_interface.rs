// SPDX-License-Identifier: CC0-1.0

//! What `DetachNetworkInterface` takes down and `AttachNetworkInterface` makes, and what each
//! refuses — driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! Five rules are under test.
//!
//! 1. A DETACH IS A WRITE ON THE INTERFACE THE ATTACHMENT IS OF: "Detaches a network
//!    interface from an instance", so the attachment the `AttachmentId` names moves to the
//!    non-terminal `detaching` state and the clock owes its completion one
//!    [`ec2::state::TRANSITION_DELAY`] later — at which point the interface holds no
//!    attachment, is `available` again, and the instance that held it no longer reports it.
//!    The interface itself stays: detaching one does not delete it. The rows span the device
//!    index the attachment sits at and how many of one instance's interfaces are taken down,
//!    and the completion is asserted to be owed by the call that STARTED it, so a second
//!    detach of an attachment already on its way down does not postpone it.
//! 2. AN ID THIS SERVICE COULD NOT HAVE ISSUED IS REFUSED BEFORE ANYTHING IS LOOKED UP:
//!    `InvalidNetworkInterfaceAttachmentId.Malformed` with the service's one malformed-id
//!    sentence `Invalid id: "<value>"`, while an id it DID issue that names no attachment is
//!    `InvalidAttachmentID.NotFound` — so a caller can tell "fix the caller" from "that
//!    attachment is gone". The table spans the forms a value can take around the issued one:
//!    not an id at all, another resource's id, the documentation-shaped long ids, a leading
//!    digit above zero, the all-zero placeholder, the short form, upper-case digits, a digit
//!    too few and too many, and the very next value the mint has not issued yet.
//! 3. WHAT CANNOT BE DETACHED IS REFUSED AND NOTHING MOVES: the instance's PRIMARY interface
//!    (device index 0) and an interface another Amazon Web Services service manages are both
//!    `OperationNotPermitted`, whatever `Force` says, and a `DryRun` of a detach that would
//!    have succeeded answers `DryRunOperation` and writes nothing.
//! 4. AN ATTACH MAKES THE ATTACHMENT A DETACH TAKES DOWN: `AttachNetworkInterface` answers an
//!    attachment id the store minted, in the non-terminal `attaching` state with `attached`
//!    owed one span later, and everything it was asked for — the device index, the network
//!    card index and the ENA Express settings — round-trips through
//!    `DescribeNetworkInterfaces` and `DescribeInstances`. An interface the clock has finished
//!    detaching can be attached again.
//! 5. AN ATTACH THAT CANNOT BE MADE IS REFUSED AND WRITES NOTHING: an id of neither kind, an
//!    unknown interface or instance, an interface something already holds, a device index the
//!    instance already uses or one outside what its type can hold, an interface in another
//!    Availability Zone, and a dry run of an attach that would have succeeded.

use std::time::Duration;

use ec2::ops::types::NetworkInterfaceAttachment;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_ATTACHMENT_ID_NOT_FOUND,
    INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED, NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX,
    NETWORK_INTERFACE_TYPE_INTERFACE, NetworkInterfaceSpec, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for: the emulator's own default Region, whose
/// default VPC and subnets a fresh account already holds.
const REGION: &str = "us-east-1";

/// The image every launch names. `RunInstances` only has to accept the id; which image an
/// instance ran from has no bearing on its interfaces.
const AMI: &str = "ami-0123456789abcdef0";

/// The instance type every launch names, and the network interfaces an instance of it holds:
/// how many an instance can have attached is a fact of its TYPE, which
/// `DescribeInstanceTypes` publishes as `NetworkInfo$MaximumNetworkInterfaces`, so the type is
/// named rather than left to the launch default and the bound is read off it. Device index 0
/// is the primary interface's, so [`INTERFACE_LIMIT`] - 1 is the last index an attach can
/// name and [`INTERFACE_LIMIT`] is one past it.
const INSTANCE_TYPE: &str = "m5.4xlarge";

/// The interfaces an instance of [`INSTANCE_TYPE`] can hold, as `DescribeInstanceTypes`
/// reports it for that type.
const INTERFACE_LIMIT: i32 = 8;

/// The instant a seeded attachment was made, as a fixed point rather than a clock read: it
/// is a seeded fact of the fixture, not something the emulator mints.
const SEEDED_ATTACH_TIME_SECS: i64 = 1_767_225_600;

/// The `RequesterId` the seeded managed interface carries — an interface another Amazon Web
/// Services service created for a resource of the account, which is what makes it one "being
/// managed by Amazon Web Services" (`NetworkInterface$RequesterManaged`).
const MANAGING_SERVICE: &str = "amazon-elb";

/// The error code EC2 answers for an action the caller may not take on the resource it
/// named. EC2's model declares no error shapes at all, so the code is matched as the wire
/// string the service answers with.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due, with a
/// margin so a slow machine fails nothing.
async fn wait_for_the_clock() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
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

/// One `DetachNetworkInterface` call for `attachment_id`, with `extra` form fields.
async fn detach(
    router: &MultiServiceRouter,
    attachment_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![param("AttachmentId", attachment_id)];
    params.extend_from_slice(extra);
    call(router, REGION, "DetachNetworkInterface", &params).await
}

/// One network interface as this suite reads it back off `DescribeNetworkInterfaces`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    /// The INTERFACE's own status (`available` / `in-use`), read through the document so it
    /// is never confused with its attachment's.
    status: Option<String>,
    attachment_id: Option<String>,
    attachment_status: Option<String>,
    device_index: Option<String>,
}

/// The one interface `network_interface_id` names, as `DescribeNetworkInterfaces` reports it.
async fn reported(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
) -> Reported {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    let parsed = document(case, &body);
    let item = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no interface in {body}"));
    let attachment = child(item, "attachment");
    Reported {
        status: child_text(item, "status"),
        attachment_id: attachment.and_then(|attachment| child_text(attachment, "attachmentId")),
        attachment_status: attachment.and_then(|attachment| child_text(attachment, "status")),
        device_index: attachment.and_then(|attachment| child_text(attachment, "deviceIndex")),
    }
}

/// The network interface ids `DescribeInstances` reports for `instance_id`.
async fn interfaces_of_instance(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    let mut ids = elements(&body, "networkInterfaceId");
    ids.sort();
    ids.dedup();
    ids
}

/// The account's default subnets, in the order the describe reports them, each with the
/// Availability Zone it sits in.
async fn default_subnets(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let vpcs = ok(
        "the default network",
        router,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    let vpc_id = element(&vpcs, "vpcId").unwrap_or_default();
    let body = ok(
        "the default network",
        router,
        "DescribeSubnets",
        &filters(&[("vpc-id", &[vpc_id.as_str()])]),
    )
    .await;
    let ids = elements(&body, "subnetId");
    let zones = elements(&body, "availabilityZone");
    assert_eq!(
        ids.len(),
        zones.len(),
        "every reported subnet must name its Availability Zone, got {body}"
    );
    ids.into_iter().zip(zones).collect()
}

/// The account's first default subnet, over the wire.
async fn default_subnet(router: &MultiServiceRouter) -> String {
    default_subnets(router)
        .await
        .into_iter()
        .next()
        .expect("a fresh account has a default subnet per zone")
        .0
}

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

/// Seed one interface in `subnet_id` ALREADY attached to `instance_id` at `device_index`,
/// through the one writer seam every interface of this store is written by
/// (`insert_network_interface`, which is also how a launch mints the primary one), and answer
/// its interface id and attachment id.
///
/// Seeded rather than made over the wire so these suites stand on an attachment that EXISTS
/// however it came to exist — what a detach does to one is independent of the attach path,
/// which suites 4 and 5 drive on their own. The record is put in the store in the same shape
/// a launch puts its primary one in, including the `attached` status an attachment the clock
/// has finished sits in.
fn seed_attached_interface(
    engine: &engine::InMemoryEngine,
    instance_id: &str,
    subnet_id: &str,
    device_index: i32,
    requester: Option<&str>,
) -> (String, String) {
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let subnet = store
        .subnets
        .get(subnet_id)
        .cloned()
        .unwrap_or_else(|| panic!("the store holds the subnet {subnet_id}"));
    let cidr_block = subnet.cidr_block.clone().unwrap_or_default();
    let address = store
        .mint_private_ip_in_cidr(&cidr_block)
        .unwrap_or_else(|| panic!("the subnet {subnet_id} has a free address to seed with"));
    let attachment_id = store.next_id(NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX);
    let network_interface_id = store
        .insert_network_interface(NetworkInterfaceSpec {
            subnet_id: subnet_id.to_string(),
            vpc_id: subnet.vpc_id.clone().unwrap_or_default(),
            availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
            availability_zone_id: subnet.availability_zone_id.clone().unwrap_or_default(),
            region: REGION.to_string(),
            interface_type: NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
            private_ip_addresses: vec![address],
            attachment: Some(NetworkInterfaceAttachment {
                attach_time: Some(::aws_smithy_types::DateTime::from_secs(
                    SEEDED_ATTACH_TIME_SECS,
                )),
                attachment_id: Some(attachment_id.clone()),
                // A secondary interface an instance was given outlives it, unlike the
                // primary one a launch creates.
                delete_on_termination: Some(false),
                device_index: Some(device_index),
                network_card_index: Some(0),
                instance_id: Some(instance_id.to_string()),
                instance_owner_id: Some(engine::emulator_account_id().to_string()),
                status: Some("attached".to_string()),
                ena_srd_specification: None,
                ena_queue_count: None,
            }),
            requester: requester.map(str::to_string),
            ..NetworkInterfaceSpec::default()
        })
        .expect("the store holds the seeded network interface");
    (network_interface_id, attachment_id)
}

/// The attachment id of the PRIMARY interface the launch of `instance_id` created, and that
/// interface's own id.
fn primary_attachment(engine: &engine::InMemoryEngine, instance_id: &str) -> (String, String) {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store
        .network_interfaces
        .iter()
        .find_map(|(network_interface_id, interface)| {
            let attachment = interface.attachment.as_ref()?;
            if attachment.instance_id.as_deref() != Some(instance_id)
                || attachment.device_index != Some(0)
            {
                return None;
            }
            Some((
                network_interface_id.clone(),
                attachment.attachment_id.clone().unwrap_or_default(),
            ))
        })
        .unwrap_or_else(|| panic!("the launch of {instance_id} created a primary interface"))
}

/// Rule 1: a detach takes the attachment down on the clock — `detaching` straight away, and
/// one `TRANSITION_DELAY` later the interface holds nothing and is `available` again, while
/// the interface itself stays.
#[tokio::test]
async fn a_detach_takes_the_attachment_down_on_the_clock() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = default_subnet(&router).await;
    let instance_id = launch(&router, &subnet_id).await;

    // The rows differ in the device index the attachment sits at, and together in how many of
    // one instance's interfaces are taken down in one go.
    let rows: Vec<(&str, i32)> = vec![
        ("the interface at device index 1", 1),
        ("the interface at device index 2", 2),
        ("the interface at device index 7", 7),
    ];
    let seeded: Vec<(&str, i32, String, String)> = rows
        .into_iter()
        .map(|(case, device_index)| {
            let (network_interface_id, attachment_id) =
                seed_attached_interface(&engine, &instance_id, &subnet_id, device_index, None);
            (case, device_index, network_interface_id, attachment_id)
        })
        .collect();

    // Before: the instance holds its primary interface and every seeded one.
    let (primary_interface_id, _) = primary_attachment(&engine, &instance_id);
    let mut expected_held: Vec<String> = seeded
        .iter()
        .map(|(_, _, network_interface_id, _)| network_interface_id.clone())
        .collect();
    expected_held.push(primary_interface_id.clone());
    expected_held.sort();
    assert_eq!(
        interfaces_of_instance("before the detaches", &router, &instance_id).await,
        expected_held,
        "the instance must hold its primary interface and every seeded one before any detach"
    );

    for (case, device_index, network_interface_id, attachment_id) in &seeded {
        let before = reported(case, &router, network_interface_id).await;
        assert_eq!(
            before.attachment_status.as_deref(),
            Some("attached"),
            "case {case:?}: the seeded attachment must start attached, got {before:?}"
        );
        assert_eq!(
            before.device_index.as_deref(),
            Some(device_index.to_string().as_str()),
            "case {case:?}: the seeded attachment must sit at device index {device_index}, \
             got {before:?}"
        );

        let (status, body) = detach(&router, attachment_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the detach, got {status} with body {body}"
        );

        // Straight after the call: the move is NOT instantaneous, so the caller still sees the
        // transient state — the attachment is `detaching` and the interface is still in use.
        let detaching = reported(case, &router, network_interface_id).await;
        assert_eq!(
            detaching.attachment_status.as_deref(),
            Some("detaching"),
            "case {case:?}: expected the attachment detaching straight after the call, \
             got {detaching:?}"
        );
        assert_eq!(
            detaching.attachment_id.as_deref(),
            Some(attachment_id.as_str()),
            "case {case:?}: a detaching attachment is still the one the request named, \
             got {detaching:?}"
        );
        assert_eq!(
            detaching.status.as_deref(),
            Some("in-use"),
            "case {case:?}: an interface whose attachment is still coming down is in use, \
             got {detaching:?}"
        );

        // A second detach of an attachment already on its way down is admitted and must NOT
        // postpone the completion the first call owed: the assertion that it did not is the
        // single wait below, which every row shares.
        let (again_status, again_body) = detach(&router, attachment_id, &[]).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected HTTP 200 from a second detach of the same attachment, \
             got {again_status} with body {again_body}"
        );
    }

    wait_for_the_clock().await;

    for (case, _, network_interface_id, attachment_id) in &seeded {
        let after = reported(case, &router, network_interface_id).await;
        assert_eq!(
            after,
            Reported {
                status: Some("available".to_string()),
                attachment_id: None,
                attachment_status: None,
                device_index: None,
            },
            "case {case:?}: one TRANSITION_DELAY after the detach the interface must hold \
             nothing and be available, got {after:?}"
        );

        // The attachment is GONE, so the id that named it names nothing: a third detach is the
        // not-found half of the pair rather than a no-op.
        let (status, body) = detach(&router, attachment_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 detaching a completed attachment, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_ATTACHMENT_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_ATTACHMENT_ID_NOT_FOUND} for an attachment that \
             is gone, got {body}"
        );
    }

    // The instance holds only its primary interface now, and the detached interfaces are
    // still interfaces of the account: a detach is not a delete.
    assert_eq!(
        interfaces_of_instance("after the detaches", &router, &instance_id).await,
        vec![primary_interface_id],
        "after the detaches the instance must hold only its primary interface"
    );
    let described = ok("the detached interfaces", &router, "DescribeNetworkInterfaces", &[]).await;
    for (case, _, network_interface_id, _) in &seeded {
        assert!(
            elements(&described, "networkInterfaceId").contains(network_interface_id),
            "case {case:?}: a detached interface is still an interface of the account, \
             got {described}"
        );
    }
}

/// Rule 2: an attachment id this service could not have issued is refused as malformed before
/// anything is looked up, while one it did issue that names no attachment is not found.
#[tokio::test]
async fn detach_tells_an_unissuable_attachment_id_from_one_that_names_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = default_subnet(&router).await;
    let instance_id = launch(&router, &subnet_id).await;
    let (network_interface_id, held_attachment_id) =
        seed_attached_interface(&engine, &instance_id, &subnet_id, 1, None);

    // Two values a digit apart around the mint's own position: one it HAS issued (so it is
    // looked up, and the attachment it named is not there) and the very next one, which it has
    // not issued and so could not have handed to any caller.
    let (issued_but_unheld, the_next_unissued) = {
        let state = engine.state::<Ec2State>();
        let mut store = state.lock();
        let issued = store.next_id(NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX);
        let unissued = format!(
            "{NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX}-{:017x}",
            store.id_counter + 1
        );
        (issued, unissued)
    };

    // The forms a value can take around the one this service issues (`eni-attach-` and 17
    // lowercase hexadecimal digits under a leading `0`, from the store's own mint).
    let malformed: Vec<(&str, String)> = vec![
        ("not an id at all", "not-an-attachment-id".to_string()),
        ("another resource's id", network_interface_id.clone()),
        (
            "the documentation-shaped long id",
            "eni-attach-0123456789abcdef0".to_string(),
        ),
        (
            "another documentation-shaped long id",
            "eni-attach-0fedcba987654321f".to_string(),
        ),
        (
            "a long id whose leading digit is above zero",
            "eni-attach-1234567890abcdef0".to_string(),
        ),
        (
            "the all-zero placeholder",
            "eni-attach-00000000000000000".to_string(),
        ),
        ("the short id form", "eni-attach-0a1b2c3d".to_string()),
        (
            "upper-case digits",
            "eni-attach-0123456789ABCDEF0".to_string(),
        ),
        (
            "one digit too few",
            "eni-attach-0000000000000001".to_string(),
        ),
        (
            "one digit too many",
            "eni-attach-000000000000000001".to_string(),
        ),
        ("the next value the mint has not issued", the_next_unissued),
    ];
    for (case, attachment_id) in &malformed {
        let (status, body) = detach(&router, attachment_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {attachment_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED,
            "case {case:?}: expected {INVALID_NETWORK_INTERFACE_ATTACHMENT_ID_MALFORMED} for \
             {attachment_id}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Invalid id: \"{attachment_id}\""),
            "case {case:?}: expected the service's malformed-id sentence naming the value the \
             request sent, got {body}"
        );
    }

    // An id the mint HAS issued is looked up, and the attachment it names not being there is
    // the other half of the pair.
    let (status, body) = detach(&router, &issued_but_unheld, &[]).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for an issued id naming no attachment, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_ATTACHMENT_ID_NOT_FOUND,
        "expected {INVALID_ATTACHMENT_ID_NOT_FOUND} for an issued id naming no attachment, \
         got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The attachment ID '{issued_but_unheld}' does not exist"),
        "expected the not-found sentence naming the attachment id from the request, got {body}"
    );

    // Every refusal above left the one attachment the account holds exactly as it was.
    let untouched = reported("the refusals", &router, &network_interface_id).await;
    assert_eq!(
        untouched.attachment_id.as_deref(),
        Some(held_attachment_id.as_str()),
        "a refused detach must leave the attachment the account holds alone, got {untouched:?}"
    );
    assert_eq!(
        untouched.attachment_status.as_deref(),
        Some("attached"),
        "a refused detach must move nothing, got {untouched:?}"
    );
}

/// Rule 3: the primary interface and an interface Amazon Web Services manages cannot be
/// detached at all, and a dry run of a detach that would have succeeded writes nothing.
#[tokio::test]
async fn detach_refuses_what_cannot_be_detached_and_writes_nothing_for_a_dry_run() {
    let (router, engine) = super::ec2_query::emulator();
    let subnet_id = default_subnet(&router).await;
    let instance_id = launch(&router, &subnet_id).await;
    let (primary_interface_id, primary_attachment_id) = primary_attachment(&engine, &instance_id);
    let (managed_interface_id, managed_attachment_id) = seed_attached_interface(
        &engine,
        &instance_id,
        &subnet_id,
        1,
        Some(MANAGING_SERVICE),
    );
    let (dry_run_interface_id, dry_run_attachment_id) =
        seed_attached_interface(&engine, &instance_id, &subnet_id, 2, None);

    // The primary interface goes with its instance, so it is refused whatever else the
    // request says — a forced detach and a dry run of one included.
    let primary_rows: Vec<(&str, Vec<Param>)> = vec![
        ("a plain detach of the primary interface", Vec::new()),
        (
            "a forced detach of the primary interface",
            vec![param("Force", "true")],
        ),
        (
            "a dry run of a detach of the primary interface",
            vec![param("DryRun", "true")],
        ),
    ];
    for (case, extra) in &primary_rows {
        let (status, body) = detach(&router, &primary_attachment_id, extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            OPERATION_NOT_PERMITTED,
            "case {case:?}: expected {OPERATION_NOT_PERMITTED}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "The network interface at device index 0 and networkcard index 0 cannot be \
             detached.",
            "case {case:?}: expected the refusal naming the attachment's two indexes, \
             got {body}"
        );
    }

    // An interface another service manages is not the account's to take down, forced or not.
    for (case, extra) in [
        ("a plain detach of a managed interface", Vec::new()),
        (
            "a forced detach of a managed interface",
            vec![param("Force", "true")],
        ),
    ] {
        let (status, body) = detach(&router, &managed_attachment_id, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            OPERATION_NOT_PERMITTED,
            "case {case:?}: expected {OPERATION_NOT_PERMITTED}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "You are not allowed to manage '{NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX}' \
                 attachments."
            ),
            "case {case:?}: expected the refusal naming the kind of attachment the caller may \
             not manage, got {body}"
        );
    }

    // A dry run of a detach that WOULD have succeeded: the permission answer, and no write.
    let (status, body) = detach(&router, &dry_run_attachment_id, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry run of a detach that would have succeeded answers 412, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        DRY_RUN_OPERATION,
        "a dry run of a detach that would have succeeded answers {DRY_RUN_OPERATION}, \
         got {body}"
    );

    // Nothing above moved: every attachment reads exactly as it was stored, and the clock owes
    // none of them anything.
    wait_for_the_clock().await;
    for (case, network_interface_id, attachment_id, device_index) in [
        (
            "the primary interface",
            &primary_interface_id,
            &primary_attachment_id,
            "0",
        ),
        (
            "the managed interface",
            &managed_interface_id,
            &managed_attachment_id,
            "1",
        ),
        (
            "the dry-run interface",
            &dry_run_interface_id,
            &dry_run_attachment_id,
            "2",
        ),
    ] {
        let after = reported(case, &router, network_interface_id).await;
        assert_eq!(
            after,
            Reported {
                status: Some("in-use".to_string()),
                attachment_id: Some(attachment_id.clone()),
                attachment_status: Some("attached".to_string()),
                device_index: Some(device_index.to_string()),
            },
            "case {case:?}: a refused or dry-run detach must leave the attachment attached, \
             got {after:?}"
        );
    }
}

/// One `CreateNetworkInterface` in `subnet_id`, as the interface id it minted.
async fn create_interface(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateNetworkInterface",
        &[param("SubnetId", subnet_id)],
    )
    .await;
    element(&body, "networkInterfaceId")
        .unwrap_or_else(|| panic!("case {case:?}: no interface in {body}"))
}

/// One `AttachNetworkInterface` call, with `extra` form fields beyond the three required
/// members.
async fn attach(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    instance_id: &str,
    device_index: i32,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![
        param("NetworkInterfaceId", network_interface_id),
        param("InstanceId", instance_id),
        param("DeviceIndex", &device_index.to_string()),
    ];
    params.extend_from_slice(extra);
    call(router, REGION, "AttachNetworkInterface", &params).await
}

/// The text of the member `path` names inside the `attachment` of the interface
/// `network_interface_id`, as `DescribeNetworkInterfaces` reports it.
async fn attachment_member(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
    path: &[&str],
) -> Option<String> {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    let parsed = document(case, &body);
    let item = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no interface in {body}"));
    let mut node = child(item, "attachment")?;
    let (last, parents) = path.split_last().expect("a member path names a member");
    for step in parents {
        node = child(node, step)?;
    }
    child_text(node, last)
}

/// One row of the attach table: what the request names beyond the interface it creates for
/// the row, and the ENA queue count the attachment must report back when it asked for one.
///
/// A struct rather than a tuple so each column is named at the row that fills it.
struct AttachRow {
    case: &'static str,
    device_index: i32,
    extra: Vec<Param>,
    /// The `enaQueueCount` the describe must report, for the row that asked for one.
    queue_count: Option<&'static str>,
}

/// One row of the attach-refusal table: what the request names, and the error code EC2
/// refuses it with.
struct AttachRefusal {
    case: &'static str,
    network_interface_id: String,
    instance_id: String,
    device_index: i32,
    extra: Vec<Param>,
    expected_code: &'static str,
}

impl AttachRefusal {
    /// A row that names nothing beyond the three required members.
    fn new(
        case: &'static str,
        network_interface_id: &str,
        instance_id: &str,
        device_index: i32,
        expected_code: &'static str,
    ) -> Self {
        Self {
            case,
            network_interface_id: network_interface_id.to_string(),
            instance_id: instance_id.to_string(),
            device_index,
            extra: Vec::new(),
            expected_code,
        }
    }

    /// The same row with `extra` form fields beside those members.
    fn naming(mut self, extra: Vec<Param>) -> Self {
        self.extra = extra;
        self
    }
}

/// Rule 4: an attach makes the attachment a detach takes down, in the non-terminal
/// `attaching` state with `attached` owed one span later, and everything it was asked for
/// round-trips through the describes.
#[tokio::test]
async fn an_attach_makes_the_attachment_a_detach_takes_down() {
    let (router, _engine) = super::ec2_query::emulator();
    let subnet_id = default_subnet(&router).await;
    let instance_id = launch(&router, &subnet_id).await;

    // The rows differ in the device index, in whether the request named the network card
    // index, and in whether it asked for ENA Express and a queue count.
    let rows = vec![
        AttachRow {
            case: "an attach that named only the three required members",
            device_index: 1,
            extra: Vec::new(),
            queue_count: None,
        },
        AttachRow {
            case: "an attach that named the network card index",
            device_index: 2,
            extra: vec![param("NetworkCardIndex", "0")],
            queue_count: None,
        },
        AttachRow {
            case: "an attach at the last device index the instance type admits, asking for ENA \
                   Express and a queue count",
            device_index: INTERFACE_LIMIT - 1,
            extra: vec![
                param("EnaSrdSpecification.EnaSrdEnabled", "true"),
                param(
                    "EnaSrdSpecification.EnaSrdUdpSpecification.EnaSrdUdpEnabled",
                    "true",
                ),
                param("EnaQueueCount", "4"),
            ],
            queue_count: Some("4"),
        },
    ];

    let mut attached: Vec<(&str, i32, String, String)> = Vec::new();
    for AttachRow {
        case,
        device_index,
        extra,
        queue_count,
    } in rows
    {
        let network_interface_id = create_interface(case, &router, &subnet_id).await;
        let before = reported(case, &router, &network_interface_id).await;
        assert_eq!(
            before.status.as_deref(),
            Some("available"),
            "case {case:?}: a created interface holds nothing, got {before:?}"
        );

        let (status, body) = attach(
            &router,
            &network_interface_id,
            &instance_id,
            device_index,
            &extra,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the attach, got {status} with body {body}"
        );
        let attachment_id = element(&body, "attachmentId")
            .unwrap_or_else(|| panic!("case {case:?}: the attach must answer an id: {body}"));
        assert!(
            super::wire::is_long_id("eni-attach", &attachment_id),
            "case {case:?}: expected an attachment id the store minted, got {attachment_id:?}"
        );
        assert_eq!(
            element(&body, "networkCardIndex").as_deref(),
            Some("0"),
            "case {case:?}: the attach reports the network card the attachment sits on, \
             got {body}"
        );

        // Straight after the call the attachment is still coming up, and the interface is in
        // use from this call on.
        let attaching = reported(case, &router, &network_interface_id).await;
        assert_eq!(
            attaching,
            Reported {
                status: Some("in-use".to_string()),
                attachment_id: Some(attachment_id.clone()),
                attachment_status: Some("attaching".to_string()),
                device_index: Some(device_index.to_string()),
            },
            "case {case:?}: expected the attachment attaching straight after the call, \
             got {attaching:?}"
        );

        // What the request asked for, read back out of the store.
        if let Some(expected_queue_count) = queue_count {
            assert_eq!(
                attachment_member(case, &router, &network_interface_id, &["enaQueueCount"])
                    .await
                    .as_deref(),
                Some(expected_queue_count),
                "case {case:?}: the queue count the attach asked for must round-trip"
            );
            assert_eq!(
                attachment_member(
                    case,
                    &router,
                    &network_interface_id,
                    &["enaSrdSpecification", "enaSrdEnabled"]
                )
                .await
                .as_deref(),
                Some("true"),
                "case {case:?}: the ENA Express setting the attach asked for must round-trip"
            );
            assert_eq!(
                attachment_member(
                    case,
                    &router,
                    &network_interface_id,
                    &[
                        "enaSrdSpecification",
                        "enaSrdUdpSpecification",
                        "enaSrdUdpEnabled"
                    ]
                )
                .await
                .as_deref(),
                Some("true"),
                "case {case:?}: the ENA Express UDP setting must round-trip"
            );
        }

        // The instance reports the interface it now holds.
        assert!(
            interfaces_of_instance(case, &router, &instance_id)
                .await
                .contains(&network_interface_id),
            "case {case:?}: the instance must report the interface it was given"
        );
        attached.push((case, device_index, network_interface_id, attachment_id));
    }

    wait_for_the_clock().await;

    for (case, device_index, network_interface_id, attachment_id) in &attached {
        let after = reported(case, &router, network_interface_id).await;
        assert_eq!(
            after,
            Reported {
                status: Some("in-use".to_string()),
                attachment_id: Some(attachment_id.clone()),
                attachment_status: Some("attached".to_string()),
                device_index: Some(device_index.to_string()),
            },
            "case {case:?}: one TRANSITION_DELAY after the attach the attachment must be \
             attached, got {after:?}"
        );

        // And the detach takes down exactly what the attach made.
        let (status, body) = detach(&router, attachment_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 detaching what the attach made, \
             got {status} with body {body}"
        );
    }

    wait_for_the_clock().await;

    // An interface the clock has finished detaching holds nothing, so it can be attached
    // again — at the same device index the first attachment used.
    for (case, device_index, network_interface_id, _) in &attached {
        let detached = reported(case, &router, network_interface_id).await;
        assert_eq!(
            detached.status.as_deref(),
            Some("available"),
            "case {case:?}: a detached interface is available again, got {detached:?}"
        );
        let (status, body) = attach(
            &router,
            network_interface_id,
            &instance_id,
            *device_index,
            &[],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an interface that holds nothing can be attached again, \
             got {status} with body {body}"
        );
    }
}

/// Rule 5: an attach that cannot be made is refused and writes nothing.
#[tokio::test]
async fn attach_refuses_what_it_cannot_attach_and_writes_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let subnets = default_subnets(&router).await;
    let (subnet_id, zone) = subnets
        .first()
        .cloned()
        .expect("a fresh account has a default subnet per zone");
    let (other_subnet_id, other_zone) = subnets
        .get(1)
        .cloned()
        .expect("a fresh account has a default subnet in every zone of the Region");
    let instance_id = launch(&router, &subnet_id).await;
    let free = create_interface("the free interface", &router, &subnet_id).await;
    let elsewhere = create_interface("the interface elsewhere", &router, &other_subnet_id).await;
    let held = create_interface("the held interface", &router, &subnet_id).await;
    let (status, body) = attach(&router, &held, &instance_id, 1, &[]).await;
    assert_eq!(
        status, 200,
        "the interface this suite holds must attach first, got {status} with body {body}"
    );

    // Each row: what the request names, and the refusal it must get. The device index is
    // bounded by what the instance's own type can hold, and index 0 is the primary
    // interface's.
    let rows = vec![
        AttachRefusal::new(
            "a value that is not an interface id",
            "not-an-interface",
            &instance_id,
            1,
            "InvalidNetworkInterfaceId.Malformed",
        ),
        AttachRefusal::new(
            "a value that is not an instance id",
            &free,
            "not-an-instance",
            1,
            "InvalidInstanceID.Malformed",
        ),
        AttachRefusal::new(
            "an interface id the account holds nothing for",
            "eni-00000000000000000",
            &instance_id,
            1,
            "InvalidNetworkInterfaceID.NotFound",
        ),
        AttachRefusal::new(
            "an instance id the account holds nothing for",
            &free,
            "i-0ffffffffffffffff",
            1,
            "InvalidInstanceID.NotFound",
        ),
        AttachRefusal::new(
            "an interface an attachment already holds",
            &held,
            &instance_id,
            4,
            "InvalidNetworkInterface.InUse",
        ),
        AttachRefusal::new(
            "a device index the instance already has an interface at",
            &free,
            &instance_id,
            0,
            "InvalidParameterValue",
        ),
        AttachRefusal::new(
            "the first device index past what the instance type can hold",
            &free,
            &instance_id,
            INTERFACE_LIMIT,
            "AttachmentLimitExceeded",
        ),
        AttachRefusal::new(
            "a device index far past what the instance type can hold",
            &free,
            &instance_id,
            64,
            "AttachmentLimitExceeded",
        ),
        AttachRefusal::new(
            "a negative device index",
            &free,
            &instance_id,
            -1,
            "InvalidParameterValue",
        ),
        AttachRefusal::new(
            "a negative network card index",
            &free,
            &instance_id,
            2,
            "InvalidParameterValue",
        )
        .naming(vec![param("NetworkCardIndex", "-1")]),
        AttachRefusal::new(
            "an interface in another Availability Zone",
            &elsewhere,
            &instance_id,
            2,
            "InvalidParameterValue",
        ),
    ];
    for row in &rows {
        let AttachRefusal {
            case,
            network_interface_id,
            instance_id: named_instance,
            device_index,
            extra,
            expected_code,
        } = row;
        let (status, body) = attach(
            &router,
            network_interface_id,
            named_instance,
            *device_index,
            extra,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // The zone mismatch names both zones, which is what tells the caller what to fix.
    let (_, body) = attach(&router, &elsewhere, &instance_id, 2, &[]).await;
    assert_eq!(
        error_message(&body),
        format!(
            "Value ({elsewhere}) for parameter networkInterfaceId is invalid. The network \
             interface is in the Availability Zone {other_zone} and the instance is in {zone}"
        ),
        "the zone mismatch must name the interface's zone and the instance's, got {body}"
    );

    // The limit refusal names the interfaces the instance WOULD then hold and the type whose
    // limit that is — a device index is a position, so an attach at index N is the N+1st
    // interface.
    let (_, body) = attach(&router, &free, &instance_id, INTERFACE_LIMIT, &[]).await;
    assert_eq!(
        error_message(&body),
        format!(
            "Interface count {} exceeds the limit for {INSTANCE_TYPE}",
            INTERFACE_LIMIT + 1
        ),
        "the limit refusal must name the count and the instance type, got {body}"
    );

    // A dry run of an attach that WOULD have succeeded: the permission answer, and no write.
    let (status, body) = attach(&router, &free, &instance_id, 2, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry run of an attach that would have succeeded answers 412, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        DRY_RUN_OPERATION,
        "a dry run of an attach that would have succeeded answers {DRY_RUN_OPERATION}, \
         got {body}"
    );

    // Nothing above wrote anything: the free interface is still free, and the one this suite
    // attached is still attached where it was.
    wait_for_the_clock().await;
    let untouched = reported("the refused attaches", &router, &free).await;
    assert_eq!(
        untouched,
        Reported {
            status: Some("available".to_string()),
            attachment_id: None,
            attachment_status: None,
            device_index: None,
        },
        "a refused or dry-run attach must leave the interface holding nothing, \
         got {untouched:?}"
    );
    let still_held = reported("the refused attaches", &router, &held).await;
    assert_eq!(
        still_held.attachment_status.as_deref(),
        Some("attached"),
        "a refused attach must leave the attachment the account holds alone, \
         got {still_held:?}"
    );
    assert_eq!(
        still_held.device_index.as_deref(),
        Some("1"),
        "a refused attach must not move an attachment to another device index, \
         got {still_held:?}"
    );
}
