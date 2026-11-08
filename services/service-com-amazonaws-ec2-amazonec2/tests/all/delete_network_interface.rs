// SPDX-License-Identifier: CC0-1.0

//! What `DeleteNetworkInterface` takes out of the store, and what it refuses — end to end
//! over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteNetworkInterface`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the delete REMOVES THE INTERFACE THE REQUEST NAMES FROM THE
//! ACCOUNT'S STORE, whatever shape that interface has, and leaves every other interface
//! alone — so the id then names nothing and a second delete refuses it rather than
//! answering as a no-op — while a value that is not an interface id at all is refused
//! `InvalidNetworkInterfaceId.Malformed`, a well-formed id the account holds no interface
//! for `InvalidNetworkInterfaceID.NotFound`, and an interface an attachment still holds
//! `InvalidNetworkInterface.InUse` ("You must detach the network interface before you can
//! delete it" — `DeleteNetworkInterface`). A `DryRun` of a call that would have succeeded
//! stops at `DryRunOperation` and removes nothing.
//!
//! Every table row is a distinct input along what that generalises over — the shape of the
//! interface being deleted, the form of the id, the status of the attachment holding it —
//! so no single recorded call is the specification.
//!
//! What the store holds is read through the store itself, and what a LATER CALLER sees is
//! read over the wire, through the second delete's refusal — the two readings this suite
//! needs. What `DescribeNetworkInterfaces` reports out of that same store has its own
//! suite, `tests/all/network_interfaces.rs`.

use ec2::ops::types::{GroupIdentifier, NetworkInterfaceAttachment, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NetworkInterfaceSpec, ATTACHMENT_STATUS_ATTACHED,
    ATTACHMENT_STATUS_DETACHED, NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX,
    NETWORK_INTERFACE_STATUS_AVAILABLE, NETWORK_INTERFACE_STATUS_IN_USE,
    NETWORK_INTERFACE_TYPE_INTERFACE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One row of a refusal table: the case, the `NetworkInterfaceId` the request carries, and
/// the EC2 error code the service answers.
type RefusalCase = (&'static str, &'static str, &'static str);

/// Where a seeded interface sits: the account's default VPC, one of its default subnets and
/// that subnet's Availability Zone — so every fixture names a subnet the store really holds
/// rather than an invented one.
#[derive(Debug, Clone)]
struct Place {
    vpc_id: String,
    subnet_id: String,
    availability_zone: String,
    availability_zone_id: String,
}

/// The place a fresh store already offers: its default VPC's first default subnet.
fn place(engine: &engine::InMemoryEngine) -> Place {
    let store = engine.state::<Ec2State>();
    let store = store.lock();
    let vpc_id = store
        .default_vpc_id(REGION)
        .expect("a fresh account has a default VPC");
    let (subnet_id, subnet) = store
        .subnets
        .iter()
        .find(|(_, subnet)| subnet.vpc_id.as_deref() == Some(&vpc_id))
        .expect("the default VPC comes with default subnets");
    Place {
        vpc_id: vpc_id.clone(),
        subnet_id: subnet_id.clone(),
        availability_zone: subnet.availability_zone.clone().unwrap_or_default(),
        availability_zone_id: subnet.availability_zone_id.clone().unwrap_or_default(),
    }
}

/// The plainest interface of `place`: the members a creator must supply and nothing else.
fn plain(place: &Place) -> NetworkInterfaceSpec {
    NetworkInterfaceSpec {
        subnet_id: place.subnet_id.clone(),
        vpc_id: place.vpc_id.clone(),
        availability_zone: place.availability_zone.clone(),
        availability_zone_id: place.availability_zone_id.clone(),
        interface_type: NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
        ..NetworkInterfaceSpec::default()
    }
}

/// A tag, as the seeded interfaces carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// One attachment of a network interface, in `status` — `None` for an attachment that
/// reports no status at all, which still holds the interface.
fn attachment(suffix: &str, status: Option<&str>) -> NetworkInterfaceAttachment {
    NetworkInterfaceAttachment {
        attachment_id: Some(format!("{NETWORK_INTERFACE_ATTACHMENT_ID_PREFIX}-{suffix}")),
        device_index: Some(1),
        instance_id: Some("i-0aaaaaaaaaaaaaaaa".to_string()),
        status: status.map(str::to_string),
        ..NetworkInterfaceAttachment::default()
    }
}

/// Seed one interface through the store's writer seam, and answer the id it was given.
fn seed(engine: &engine::InMemoryEngine, spec: NetworkInterfaceSpec) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_network_interface(spec)
        .expect("the store holds the seeded network interface")
}

/// Whether the store still holds `network_interface_id`.
fn stored(engine: &engine::InMemoryEngine, network_interface_id: &str) -> bool {
    engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .contains_key(network_interface_id)
}

/// The `Status` the store reports for `network_interface_id`, or `None` when it holds no
/// such interface.
fn status_of(engine: &engine::InMemoryEngine, network_interface_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .get(network_interface_id)?
        .status
        .clone()
}

/// The caller-chosen bytes the store currently holds, so a delete can be held to giving
/// back exactly what the insert took.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// One `DeleteNetworkInterface` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DeleteNetworkInterface", params).await
}

/// One `DeleteNetworkInterface` of `network_interface_id`.
async fn delete_id(router: &MultiServiceRouter, network_interface_id: &str) -> (u16, String) {
    delete(router, &[param("NetworkInterfaceId", network_interface_id)]).await
}

// ---------------------------------------------------------------------------
// What the delete takes out of the store
// ---------------------------------------------------------------------------

/// The rule: the delete removes the interface the request names, whatever shape that
/// interface has, leaves every other interface alone, gives the bytes it held back to the
/// store, and leaves the id naming nothing — so a second delete refuses it.
///
/// The rows differ in the shape of the interface being deleted: how many optional members
/// it carries, how many tags and security groups, whether it has a private address, its
/// `NetworkInterfaceType`, and whether it carries an attachment that has already been taken
/// away.
#[tokio::test]
async fn the_delete_removes_the_named_interface_whatever_shape_it_has() {
    let (router, engine) = super::ec2_query::emulator();
    let place = place(&engine);
    // A bystander interface that no row deletes: every delete must leave it exactly where
    // it was.
    let bystander = seed(&engine, plain(&place));

    // (case, the interface to seed and then delete)
    let cases: Vec<(&str, NetworkInterfaceSpec)> = vec![
        ("the plainest interface", plain(&place)),
        (
            "a description and three tags",
            NetworkInterfaceSpec {
                description: "the web tier's interface".to_string(),
                tags: vec![
                    tag("Name", "web"),
                    tag("Tier", "frontend"),
                    tag("Owner", "platform"),
                ],
                ..plain(&place)
            },
        ),
        (
            "a private address and two security groups",
            NetworkInterfaceSpec {
                private_ip_addresses: vec!["172.31.7.42".to_string()],
                groups: vec![
                    GroupIdentifier {
                        group_id: Some("sg-0aaaaaaaaaaaaaaaa".to_string()),
                        group_name: Some("default".to_string()),
                    },
                    GroupIdentifier {
                        group_id: Some("sg-0bbbbbbbbbbbbbbbb".to_string()),
                        group_name: Some("web".to_string()),
                    },
                ],
                ..plain(&place)
            },
        ),
        (
            // Another `NetworkInterfaceType` value the model publishes
            // (`smithy_query(query="shape",shape="NetworkInterfaceType")`), which the issue
            // did not cite: the type an interface carries changes nothing about its delete.
            "an efa interface",
            NetworkInterfaceSpec {
                interface_type: "efa".to_string(),
                ..plain(&place)
            },
        ),
        (
            // An attachment that has reached `detached` holds nothing any more, so the
            // interface is deletable: it is the detachment the delete requires.
            "an interface whose attachment has been taken away",
            NetworkInterfaceSpec {
                attachment: Some(attachment(
                    "0dddddddddddddddd",
                    Some(ATTACHMENT_STATUS_DETACHED),
                )),
                ..plain(&place)
            },
        ),
    ];

    for (case, spec) in cases {
        let bytes_before = stored_bytes(&engine);
        let network_interface_id = seed(&engine, spec);
        assert!(
            stored(&engine, &network_interface_id),
            "case {case:?}: the interface must be stored before it is deleted"
        );
        assert_eq!(
            status_of(&engine, &network_interface_id).as_deref(),
            Some(NETWORK_INTERFACE_STATUS_AVAILABLE),
            "case {case:?}: an interface nothing holds is {NETWORK_INTERFACE_STATUS_AVAILABLE}, \
             got {:?}",
            status_of(&engine, &network_interface_id)
        );

        let (status, body) = delete_id(&router, &network_interface_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteNetworkInterface, got {status} \
             with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: a successful delete carries the success flag, got {body}"
        );

        assert!(
            !stored(&engine, &network_interface_id),
            "case {case:?}: the deleted interface {network_interface_id} must be gone from \
             the store"
        );
        assert!(
            stored(&engine, &bystander),
            "case {case:?}: deleting {network_interface_id} must leave the bystander \
             {bystander} alone"
        );
        let bytes_after = stored_bytes(&engine);
        assert_eq!(
            bytes_after, bytes_before,
            "case {case:?}: the delete gives back exactly the bytes the insert took, \
             {bytes_before} became {bytes_after}"
        );

        // The id now names nothing, so a second delete refuses it rather than answering as
        // a no-op — which is what a LATER CALLER sees over the wire.
        let (again_status, again_body) = delete_id(&router, &network_interface_id).await;
        assert_eq!(
            again_status, 400,
            "case {case:?}: a delete is not idempotent; expected HTTP 400, got \
             {again_status} with body {again_body}"
        );
        assert_eq!(
            error_code(&again_body),
            "InvalidNetworkInterfaceID.NotFound",
            "case {case:?}: expected InvalidNetworkInterfaceID.NotFound from the second \
             delete, got {again_body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ids a delete refuses
// ---------------------------------------------------------------------------

/// The rule: a `NetworkInterfaceId` that is not an interface id at all is refused
/// `InvalidNetworkInterfaceId.Malformed`, a well-formed one the account holds no interface
/// for `InvalidNetworkInterfaceID.NotFound`, and either way nothing is deleted.
///
/// The rows differ in the form of the value: both published id lengths (8 and 17 lowercase
/// hex digits) for the well-formed ones — including the all-zero id, which is well formed
/// for an interface — and, for the malformed ones, a digit too few of each length, an
/// uppercase digit, another resource kind's id, text that is no id at all, and an empty
/// value.
#[tokio::test]
async fn an_id_that_names_no_interface_of_the_account_deletes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let place = place(&engine);
    let kept = seed(&engine, plain(&place));
    let bytes_before = stored_bytes(&engine);

    let cases: [RefusalCase; 9] = [
        (
            "a long id no interface has",
            "eni-0123456789abcdef0",
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            // The all-zero suffix is WELL FORMED for an interface, so it is answered
            // `NotFound` rather than `.Malformed`.
            "the all-zero long id",
            "eni-00000000000000000",
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "a short id no interface has",
            "eni-1a2b3c4d",
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "one digit short of the short form",
            "eni-0a1b2c3",
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "one digit short of the long form",
            "eni-0123456789abcdef",
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "an uppercase hex digit",
            "eni-0123456789ABCDEF0",
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "another resource kind's id",
            "subnet-0123456789abcdef0",
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "not an id at all",
            "not-an-id",
            "InvalidNetworkInterfaceId.Malformed",
        ),
        (
            "an empty networkInterfaceId",
            "",
            "InvalidNetworkInterfaceId.Malformed",
        ),
    ];

    for (case, network_interface_id, expected_code) in cases {
        let (status, body) = delete_id(&router, network_interface_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            stored(&engine, &kept),
            "case {case:?}: a refused delete must leave {kept} where it was"
        );
        let bytes_after = stored_bytes(&engine);
        assert_eq!(
            bytes_after, bytes_before,
            "case {case:?}: a refused delete changes nothing, {bytes_before} became \
             {bytes_after}"
        );
    }

    // A request that omits the member entirely never reaches the operation:
    // `NetworkInterfaceId` is required, so the boundary validator refuses it first.
    let (status, body) = delete(&router, &[]).await;
    assert_eq!(
        status, 400,
        "a request with no NetworkInterfaceId must be refused with HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "a request with no NetworkInterfaceId is refused by the boundary validator, \
         got {body}"
    );
    assert!(
        stored(&engine, &kept),
        "a refused delete must leave {kept} where it was"
    );
}

// ---------------------------------------------------------------------------
// An interface something still holds
// ---------------------------------------------------------------------------

/// The rule: "You must detach the network interface before you can delete it"
/// (`DeleteNetworkInterface`) — an interface an attachment still holds is refused
/// `InvalidNetworkInterface.InUse` and stays exactly where it was, and only an attachment
/// that has reached `detached` lets the delete through.
///
/// The rows differ in the `AttachmentStatus` of the attachment
/// (`smithy_query(query="shape",shape="AttachmentStatus")`), plus an attachment that reports
/// no status at all, which still holds the interface.
#[tokio::test]
async fn an_interface_an_attachment_still_holds_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let place = place(&engine);

    // (case, the attachment's status, whether the delete goes through)
    let cases: [(&str, Option<&str>, bool); 5] = [
        ("an attachment under way", Some("attaching"), false),
        ("a finished attachment", Some(ATTACHMENT_STATUS_ATTACHED), false),
        ("an attachment being taken away", Some("detaching"), false),
        ("an attachment that reports no status", None, false),
        ("an attachment already taken away", Some(ATTACHMENT_STATUS_DETACHED), true),
    ];

    for (index, (case, attachment_status, deletable)) in cases.into_iter().enumerate() {
        let network_interface_id = seed(
            &engine,
            NetworkInterfaceSpec {
                attachment: Some(attachment(
                    &format!("0{index:016x}"),
                    attachment_status,
                )),
                ..plain(&place)
            },
        );
        let expected_status = if deletable {
            NETWORK_INTERFACE_STATUS_AVAILABLE
        } else {
            NETWORK_INTERFACE_STATUS_IN_USE
        };
        assert_eq!(
            status_of(&engine, &network_interface_id).as_deref(),
            Some(expected_status),
            "case {case:?}: expected the stored status {expected_status}, got {:?}",
            status_of(&engine, &network_interface_id)
        );

        let (status, body) = delete_id(&router, &network_interface_id).await;
        if deletable {
            assert_eq!(
                status, 200,
                "case {case:?}: an interface nothing holds is deletable; expected HTTP 200, \
                 got {status} with body {body}"
            );
            assert!(
                !stored(&engine, &network_interface_id),
                "case {case:?}: the deleted interface must be gone from the store"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidNetworkInterface.InUse",
                "case {case:?}: expected InvalidNetworkInterface.InUse, got {body}"
            );
            assert!(
                stored(&engine, &network_interface_id),
                "case {case:?}: a refused delete must leave the interface where it was"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// The rule: a `DryRun` of a call that WOULD have succeeded answers `DryRunOperation` and
/// removes nothing, while a call that would have been refused is refused with its own reason
/// whether or not `DryRun` is set — the dry run reports the permission check, not the
/// outcome of the lookup.
///
/// The rows differ in what the request names: a deletable interface, an interface an
/// attachment holds, and an id the account holds no interface for.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check_and_removes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let place = place(&engine);
    let deletable = seed(&engine, plain(&place));
    let held = seed(
        &engine,
        NetworkInterfaceSpec {
            attachment: Some(attachment("0eeeeeeeeeeeeeeee", Some(ATTACHMENT_STATUS_ATTACHED))),
            ..plain(&place)
        },
    );
    let unknown = "eni-0ffffffffffffffff".to_string();

    // (case, the id the dry run names, the status it answers, the error code it answers)
    let cases: [(&str, &str, u16, &str); 3] = [
        (
            "an interface the delete would have removed",
            &deletable,
            412,
            "DryRunOperation",
        ),
        (
            "an interface an attachment still holds",
            &held,
            400,
            "InvalidNetworkInterface.InUse",
        ),
        (
            "an id the account holds no interface for",
            &unknown,
            400,
            "InvalidNetworkInterfaceID.NotFound",
        ),
    ];

    for (case, network_interface_id, expected_status, expected_code) in cases {
        let (status, body) = delete(
            &router,
            &[
                param("NetworkInterfaceId", network_interface_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    for (case, network_interface_id) in [("deletable", &deletable), ("held", &held)] {
        assert!(
            stored(&engine, network_interface_id),
            "case {case:?}: a dry run removes nothing, {network_interface_id} must still be \
             stored"
        );
    }

    // And with the flag off the same call goes through, so the dry run was the only thing
    // standing between the request and the deletion.
    let (status, body) = delete(
        &router,
        &[
            param("NetworkInterfaceId", &deletable),
            param("DryRun", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 with DryRun off, got {status} with body {body}"
    );
    assert!(
        !stored(&engine, &deletable),
        "with DryRun off the interface {deletable} must be gone from the store"
    );
}

// ---------------------------------------------------------------------------
// The same store, read by the other operation that names an interface
// ---------------------------------------------------------------------------

/// The rule reaches every operation that resolves a `NetworkInterfaceId`, not only the
/// delete: `AssociateAddress` reads THE SAME store, so a malformed value is refused
/// `InvalidNetworkInterfaceId.Malformed`, a well-formed id the account holds no interface
/// for `InvalidNetworkInterfaceID.NotFound`, and an id the store DOES hold names the
/// interface the Elastic IP address is then bound to — the association `AssociateAddress`
/// records on it and on the address.
///
/// This lives beside the store it exercises rather than in `super::associate_address`,
/// whose own table covers that operation's address and instance rules; the rows here differ
/// only in what the named interface id is: well formed but unheld, or no interface id at
/// all, beside the one the store holds.
#[tokio::test]
async fn associate_address_resolves_a_network_interface_id_against_the_same_store() {
    let (router, engine) = super::ec2_query::emulator();
    let place = place(&engine);
    let stored_interface = seed(&engine, plain(&place));

    // (case, the NetworkInterfaceId the request carries, the status, the error code)
    let cases: [(&str, &str, u16, &str); 2] = [
        (
            "a well-formed id the store holds no interface for",
            "eni-0123456789abcdef0",
            400,
            "InvalidNetworkInterfaceID.NotFound",
        ),
        (
            "a value that is no interface id at all",
            "not-an-id",
            400,
            "InvalidNetworkInterfaceId.Malformed",
        ),
    ];

    for (case, network_interface_id, expected_status, expected_code) in cases {
        // One Elastic IP address per row, so no row depends on another's allocation.
        let (allocate_status, allocate_body) =
            super::ec2_query::call(&router, REGION, "AllocateAddress", &[]).await;
        assert_eq!(
            allocate_status, 200,
            "case {case:?}: AllocateAddress must succeed, got {allocate_status} with body \
             {allocate_body}"
        );
        let allocation_id = element(&allocate_body, "allocationId").unwrap_or_else(|| {
            panic!("case {case:?}: AllocateAddress must mint an allocation id: {allocate_body}")
        });

        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "AssociateAddress",
            &[
                param("AllocationId", &allocation_id),
                param("NetworkInterfaceId", network_interface_id),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            stored(&engine, &stored_interface),
            "case {case:?}: a refused association must leave the interface where it was"
        );
    }

    // The interface the store DOES hold is served: the association is recorded on it, which
    // is the same record this suite's delete reads.
    let case = "an interface the store holds";
    let (_, allocate_body) =
        super::ec2_query::call(&router, REGION, "AllocateAddress", &[]).await;
    let allocation_id = element(&allocate_body, "allocationId").unwrap_or_else(|| {
        panic!("case {case:?}: AllocateAddress must mint an allocation id: {allocate_body}")
    });
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "AssociateAddress",
        &[
            param("AllocationId", &allocation_id),
            param("NetworkInterfaceId", &stored_interface),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let association_id = element(&body, "associationId").unwrap_or_default();
    assert!(
        !association_id.is_empty(),
        "case {case:?}: the association must be minted, got {body}"
    );
    let recorded = engine
        .state::<Ec2State>()
        .lock()
        .network_interfaces
        .get(&stored_interface)
        .and_then(|interface| interface.association.clone())
        .and_then(|association| association.association_id);
    assert_eq!(
        recorded.as_deref(),
        Some(association_id.as_str()),
        "case {case:?}: the interface must hold the association the call answered, \
         got {recorded:?}"
    );
}
