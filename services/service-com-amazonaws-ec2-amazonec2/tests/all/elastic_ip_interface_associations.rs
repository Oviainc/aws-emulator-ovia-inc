// SPDX-License-Identifier: CC0-1.0

//! What an Elastic IP ASSOCIATION binds to, what both sides of it report, and what ending
//! it lets a caller do — end to end over the wire the service actually speaks, `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test is that an Elastic IP association BINDS THE ADDRESS TO A NETWORK
//! INTERFACE — the interface the request names, or the single interface attached to the
//! instance it names, because an instance carries a public address only through one — and
//! that the service records that one fact on both sides it is read from: the address record
//! `DescribeAddresses` reports (its `associationId`, the interface, the interface's owning
//! account, the private address the Elastic IP is bound to and the instance holding the
//! interface) and the interface's own `association`, which `DescribeNetworkInterfaces`
//! reports. `DisassociateAddress` ends exactly that association, leaving the address
//! allocated, after which `ReleaseAddress` gives the allocation up — where an address that
//! still carries an association cannot be released at all ("you must disassociate the
//! Elastic IP address before you can release it. Otherwise, Amazon EC2 returns an error
//! (InvalidIPAddress.InUse)" — `ReleaseAddress`).
//!
//! Each table row is a distinct input along what the rule generalises over — which member
//! names the target, whether the interface is attached to an instance or held by nothing,
//! whether the address is bound to the interface's primary or to a secondary private
//! address, which member names the address, and which member names the association being
//! ended — so no row is the specification.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2::state::ELASTIC_IP_ASSOCIATION_ID_PREFIX;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child, child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The image every launch here uses; `RunInstances` only needs one that resolves.
const AMI: &str = "ami-0123456789abcdef0";

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    ec2_query::call(router, REGION, action, params).await
}

/// One call that must answer HTTP 200, as its body.
async fn served(router: &MultiServiceRouter, case: &str, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Allocate one Elastic IP address and answer `(allocation id, public ip)`.
async fn allocate(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let body = served(router, case, "AllocateAddress", &[]).await;
    let allocation_id = ec2_query::element(&body, "allocationId")
        .unwrap_or_else(|| panic!("case {case:?}: AllocateAddress minted no allocation id: {body}"));
    let public_ip = ec2_query::element(&body, "publicIp")
        .unwrap_or_else(|| panic!("case {case:?}: AllocateAddress minted no address: {body}"));
    (allocation_id, public_ip)
}

/// Launch one instance and answer `(instance id, its primary private address)`.
async fn launch(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let params = [
        param("ImageId", AMI),
        param("MinCount", "1"),
        param("MaxCount", "1"),
    ];
    let body = served(router, case, "RunInstances", &params).await;
    let instance_id = ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instance: {body}"));
    let private_ip = ec2_query::element(&body, "privateIpAddress")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no private address: {body}"));
    (instance_id, private_ip)
}

/// A subnet the account really holds: the first one `DescribeSubnets` reports, which is a
/// default subnet of the Region's default VPC.
async fn any_subnet(router: &MultiServiceRouter, case: &str) -> String {
    let body = served(router, case, "DescribeSubnets", &[]).await;
    ec2_query::element(&body, "subnetId")
        .unwrap_or_else(|| panic!("case {case:?}: the account holds no subnet: {body}"))
}

/// Create one network interface with `extra` beside its subnet, and answer its id with the
/// private addresses it was given, primary first.
async fn create_interface(
    router: &MultiServiceRouter,
    case: &str,
    subnet_id: &str,
    extra: &[Param],
) -> (String, Vec<String>) {
    let mut params = vec![param("SubnetId", subnet_id)];
    params.extend_from_slice(extra);
    let body = served(router, case, "CreateNetworkInterface", &params).await;
    let network_interface_id = ec2_query::element(&body, "networkInterfaceId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateNetworkInterface minted no interface: {body}")
    });
    let parsed = document(case, &body);
    let interface = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no interface element in {body}"));
    let mut held: Vec<(bool, String)> = set_items(interface, "privateIpAddressesSet")
        .into_iter()
        .filter_map(|item| {
            let address = child_text(item, "privateIpAddress")?;
            let primary = child_text(item, "primary").as_deref() == Some("true");
            Some((!primary, address))
        })
        .collect();
    held.sort();
    assert!(
        !held.is_empty(),
        "case {case:?}: a created interface must hold a private address: {body}"
    );
    (
        network_interface_id,
        held.into_iter().map(|(_, address)| address).collect(),
    )
}

/// The interface attached to `instance_id` at the lowest device index — the primary one its
/// launch created, read through the harness every suite that needs it shares.
async fn primary_interface(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    ec2_query::primary_network_interface(router, REGION, case, instance_id).await
}

/// One Elastic IP address as `DescribeAddresses` reports it — the members an association
/// writes, and the two every allocation has.
#[derive(Debug, Default, PartialEq, Eq)]
struct AddressRow {
    allocation_id: Option<String>,
    public_ip: Option<String>,
    association_id: Option<String>,
    instance_id: Option<String>,
    network_interface_id: Option<String>,
    network_interface_owner_id: Option<String>,
    private_ip_address: Option<String>,
}

/// The one address `allocation_id` names, as `DescribeAddresses` reports it.
async fn stored_address(
    router: &MultiServiceRouter,
    case: &str,
    allocation_id: &str,
) -> AddressRow {
    let params = [param("AllocationId.1", allocation_id)];
    let body = served(router, case, "DescribeAddresses", &params).await;
    let parsed = document(case, &body);
    let address = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "allocationId").as_deref() == Some(allocation_id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no address {allocation_id} in {body}"));
    AddressRow {
        allocation_id: child_text(address, "allocationId"),
        public_ip: child_text(address, "publicIp"),
        association_id: child_text(address, "associationId"),
        instance_id: child_text(address, "instanceId"),
        network_interface_id: child_text(address, "networkInterfaceId"),
        network_interface_owner_id: child_text(address, "networkInterfaceOwnerId"),
        private_ip_address: child_text(address, "privateIpAddress"),
    }
}

/// The `association` of one network interface, as `DescribeNetworkInterfaces` reports it —
/// `None` for an interface that carries no public address at all.
#[derive(Debug, Default, PartialEq, Eq)]
struct InterfaceAssociation {
    allocation_id: Option<String>,
    association_id: Option<String>,
    ip_owner_id: Option<String>,
    public_ip: Option<String>,
}

/// The association the interface `network_interface_id` reports, and the association its
/// `bound_to` private address reports — the same fact in the two places the interface
/// carries it.
async fn interface_association(
    router: &MultiServiceRouter,
    case: &str,
    network_interface_id: &str,
    bound_to: &str,
) -> (Option<InterfaceAssociation>, Option<InterfaceAssociation>) {
    let params = [param("NetworkInterfaceId.1", network_interface_id)];
    let body = served(router, case, "DescribeNetworkInterfaces", &params).await;
    let parsed = document(case, &body);
    let interface = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "networkInterfaceId").as_deref()
                    == Some(network_interface_id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no interface {network_interface_id} in {body}"));
    let read = |node: roxmltree::Node<'_, '_>| InterfaceAssociation {
        allocation_id: child_text(node, "allocationId"),
        association_id: child_text(node, "associationId"),
        ip_owner_id: child_text(node, "ipOwnerId"),
        public_ip: child_text(node, "publicIp"),
    };
    let on_interface = child(interface, "association").map(read);
    let on_address = set_items(interface, "privateIpAddressesSet")
        .into_iter()
        .find(|item| child_text(*item, "privateIpAddress").as_deref() == Some(bound_to))
        .and_then(|item| child(item, "association"))
        .map(read);
    (on_interface, on_address)
}

/// The public address `DescribeInstances` reports for `instance_id` (`Instance$ipAddress`).
async fn instance_public_ip(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Option<String> {
    let params = [param("InstanceId.1", instance_id)];
    let body = served(router, case, "DescribeInstances", &params).await;
    ec2_query::element(&body, "ipAddress")
}

/// An association id is [`ELASTIC_IP_ASSOCIATION_ID_PREFIX`] followed by 8 or 17 lowercase
/// hexadecimal digits — the short and the long EC2 resource id form.
fn is_association_id(candidate: &str) -> bool {
    candidate
        .strip_prefix(&format!("{ELASTIC_IP_ASSOCIATION_ID_PREFIX}-"))
        .is_some_and(|suffix| {
            matches!(suffix.len(), 8 | 17)
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// What one row's request names as the resource the address is to be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The INSTANCE, which binds the address to the interface its launch created.
    Instance,
    /// That same interface, named directly.
    InstancePrimaryInterface,
    /// An interface nothing holds, created for itself.
    FreeInterface,
    /// An interface nothing holds, with the address bound to a SECONDARY private address of
    /// it rather than to its primary one.
    FreeInterfaceSecondaryAddress,
}

/// One row of the association table.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    target: Target,
    /// Name the address by its deprecated `PublicIp` rather than by its allocation id.
    by_public_ip: bool,
    /// End the association by the address rather than by the association id.
    disassociate_by_public_ip: bool,
}

/// RULE: the association binds the address to an interface, both sides report it,
/// `DisassociateAddress` ends it and `ReleaseAddress` can then give the allocation up —
/// whichever way the request names the target, the address and the association.
#[tokio::test]
async fn an_association_binds_an_address_to_an_interface_and_both_sides_report_it() {
    let rows = [
        Row {
            case: "the instance, which binds the address to its primary interface",
            target: Target::Instance,
            by_public_ip: false,
            disassociate_by_public_ip: false,
        },
        Row {
            case: "the instance's primary interface, named directly",
            target: Target::InstancePrimaryInterface,
            by_public_ip: false,
            disassociate_by_public_ip: false,
        },
        Row {
            case: "an interface nothing holds, the address named by its public address",
            target: Target::FreeInterface,
            by_public_ip: true,
            disassociate_by_public_ip: false,
        },
        Row {
            case: "a secondary private address of an interface nothing holds",
            target: Target::FreeInterfaceSecondaryAddress,
            by_public_ip: false,
            disassociate_by_public_ip: false,
        },
        Row {
            case: "the instance, with the association ended by the address itself",
            target: Target::Instance,
            by_public_ip: false,
            disassociate_by_public_ip: true,
        },
    ];

    let router = emulator();
    let subnet_id = any_subnet(&router, "the account's own subnet").await;
    let account_id = engine::emulator_account_id().to_string();
    let mut association_ids: BTreeSet<String> = BTreeSet::new();

    for row in &rows {
        let case = row.case;
        let (allocation_id, public_ip) = allocate(&router, case).await;

        // The target, and what the association is then expected to name: the interface it
        // binds to, the private address of that interface, and the instance holding it.
        let (mut params, network_interface_id, private_ip, instance_id) = match row.target {
            Target::Instance => {
                let (instance_id, private_ip) = launch(&router, case).await;
                let network_interface_id = primary_interface(&router, case, &instance_id).await;
                (
                    vec![param("InstanceId", &instance_id)],
                    network_interface_id,
                    private_ip,
                    Some(instance_id),
                )
            }
            Target::InstancePrimaryInterface => {
                let (instance_id, private_ip) = launch(&router, case).await;
                let network_interface_id = primary_interface(&router, case, &instance_id).await;
                (
                    vec![param("NetworkInterfaceId", &network_interface_id)],
                    network_interface_id,
                    private_ip,
                    Some(instance_id),
                )
            }
            Target::FreeInterface => {
                let (network_interface_id, held) =
                    create_interface(&router, case, &subnet_id, &[]).await;
                let private_ip = held[0].clone();
                (
                    vec![param("NetworkInterfaceId", &network_interface_id)],
                    network_interface_id,
                    private_ip,
                    None,
                )
            }
            Target::FreeInterfaceSecondaryAddress => {
                let (network_interface_id, held) = create_interface(
                    &router,
                    case,
                    &subnet_id,
                    &[param("SecondaryPrivateIpAddressCount", "1")],
                )
                .await;
                assert!(
                    held.len() > 1,
                    "case {case:?}: the interface must hold a secondary private address, \
                     got {held:?}"
                );
                let private_ip = held[1].clone();
                (
                    vec![
                        param("NetworkInterfaceId", &network_interface_id),
                        param("PrivateIpAddress", &private_ip),
                    ],
                    network_interface_id,
                    private_ip,
                    None,
                )
            }
        };
        if row.by_public_ip {
            params.push(param("PublicIp", &public_ip));
        } else {
            params.push(param("AllocationId", &allocation_id));
        }

        // The association is minted, and every one is its own.
        let body = served(&router, case, "AssociateAddress", &params).await;
        // "Returns true if the request succeeds": the ec2Query envelope's own flag, which
        // this operation answers beside the association id it mints.
        let returned = ec2_query::member(&body, "return");
        assert_eq!(
            returned.as_deref(),
            Some("true"),
            "case {case:?}: AssociateAddress must answer the envelope's return flag, \
             got {returned:?} in {body}"
        );
        let association_id = ec2_query::element(&body, "associationId").unwrap_or_default();
        assert!(
            is_association_id(&association_id),
            "case {case:?}: expected an {ELASTIC_IP_ASSOCIATION_ID_PREFIX}- association id, \
             got {association_id:?} in {body}"
        );
        assert!(
            association_ids.insert(association_id.clone()),
            "case {case:?}: every association gets its own id, but {association_id} was \
             handed out twice"
        );

        // Side one: the address record.
        let stored = stored_address(&router, case, &allocation_id).await;
        assert_eq!(
            stored,
            AddressRow {
                allocation_id: Some(allocation_id.clone()),
                public_ip: Some(public_ip.clone()),
                association_id: Some(association_id.clone()),
                instance_id: instance_id.clone(),
                network_interface_id: Some(network_interface_id.clone()),
                network_interface_owner_id: Some(account_id.clone()),
                private_ip_address: Some(private_ip.clone()),
            },
            "case {case:?}: DescribeAddresses must report the stored association, \
             got {stored:?}"
        );

        // Side two: the interface, and the private address the Elastic IP is bound to.
        let expected = InterfaceAssociation {
            allocation_id: Some(allocation_id.clone()),
            association_id: Some(association_id.clone()),
            ip_owner_id: Some(account_id.clone()),
            public_ip: Some(public_ip.clone()),
        };
        let (on_interface, on_address) =
            interface_association(&router, case, &network_interface_id, &private_ip).await;
        assert_eq!(
            on_interface.as_ref(),
            Some(&expected),
            "case {case:?}: the interface must report the association, got {on_interface:?}"
        );
        assert_eq!(
            on_address.as_ref(),
            Some(&expected),
            "case {case:?}: the private address the Elastic IP is bound to must report the \
             association, got {on_address:?}"
        );

        // And the instance holding the interface, when one does, reports the address.
        if let Some(instance_id) = instance_id.as_deref() {
            let reported = instance_public_ip(&router, case, instance_id).await;
            assert_eq!(
                reported,
                Some(public_ip.clone()),
                "case {case:?}: the instance must report the Elastic IP address as its \
                 public address, got {reported:?}"
            );
        }

        // An address that still carries an association is in use, so it cannot be released.
        let (status, release_body) =
            api(&router, "ReleaseAddress", &[param("AllocationId", &allocation_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: releasing an associated address must be refused, got {status} \
             with body {release_body}"
        );
        let code = ec2_query::error_code(&release_body);
        assert_eq!(
            code, "InvalidIPAddress.InUse",
            "case {case:?}: expected InvalidIPAddress.InUse, got {code:?} with body \
             {release_body}"
        );
        let message = ec2_query::error_message(&release_body);
        assert_eq!(
            message,
            format!("Address {public_ip} is in use."),
            "case {case:?}: the refusal must name the address that is in use, got {message:?}"
        );

        // Ending the association, named either way, leaves the address allocated and takes
        // the association off both sides.
        let ending = if row.disassociate_by_public_ip {
            vec![param("PublicIp", &public_ip)]
        } else {
            vec![param("AssociationId", &association_id)]
        };
        served(&router, case, "DisassociateAddress", &ending).await;
        let freed = stored_address(&router, case, &allocation_id).await;
        assert_eq!(
            freed,
            AddressRow {
                allocation_id: Some(allocation_id.clone()),
                public_ip: Some(public_ip.clone()),
                ..AddressRow::default()
            },
            "case {case:?}: the disassociated address must keep only its allocation, \
             got {freed:?}"
        );
        let (on_interface, on_address) =
            interface_association(&router, case, &network_interface_id, &private_ip).await;
        assert_eq!(
            on_interface, None,
            "case {case:?}: the interface must report no association, got {on_interface:?}"
        );
        assert_eq!(
            on_address, None,
            "case {case:?}: the private address must report no association, got {on_address:?}"
        );
        if let Some(instance_id) = instance_id.as_deref() {
            let reported = instance_public_ip(&router, case, instance_id).await;
            assert_eq!(
                reported, None,
                "case {case:?}: the instance must report no public address once the \
                 association is ended, got {reported:?}"
            );
        }

        // And the allocation can now be given up, after which the id names nothing.
        served(
            &router,
            case,
            "ReleaseAddress",
            &[param("AllocationId", &allocation_id)],
        )
        .await;
        let (status, described) = api(
            &router,
            "DescribeAddresses",
            &[param("AllocationId.1", &allocation_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a released allocation must name nothing, got {status} with body \
             {described}"
        );
        let code = ec2_query::error_code(&described);
        assert_eq!(
            code, "InvalidAllocationID.NotFound",
            "case {case:?}: expected InvalidAllocationID.NotFound, got {code:?} with body \
             {described}"
        );
    }
}

/// RULE, as an INVARIANT: an association is idempotent and MOVES on request — asking for
/// the one that already holds answers the id it was given and changes nothing, and asking
/// for the address elsewhere takes it off the interface it was on, which reports no
/// association afterwards.
#[tokio::test]
async fn an_association_is_idempotent_and_moves_between_interfaces() {
    let router = emulator();
    let case = "one address across two interfaces";
    let subnet_id = any_subnet(&router, case).await;
    let (allocation_id, public_ip) = allocate(&router, case).await;
    let (left, left_addresses) = create_interface(&router, case, &subnet_id, &[]).await;
    let (right, right_addresses) = create_interface(&router, case, &subnet_id, &[]).await;

    let first = served(
        &router,
        case,
        "AssociateAddress",
        &[
            param("AllocationId", &allocation_id),
            param("NetworkInterfaceId", &left),
        ],
    )
    .await;
    let first = ec2_query::element(&first, "associationId").unwrap_or_default();
    // Idempotent: "if you perform the operation more than once, Amazon EC2 doesn't return an
    // error", and the standing association keeps the id it was given.
    for repeat in 1..=2 {
        let again = served(
            &router,
            case,
            "AssociateAddress",
            &[
                param("AllocationId", &allocation_id),
                param("NetworkInterfaceId", &left),
                param("PrivateIpAddress", &left_addresses[0]),
            ],
        )
        .await;
        let again = ec2_query::element(&again, "associationId").unwrap_or_default();
        assert_eq!(
            again, first,
            "case {case:?}: repeat {repeat} of the standing association must answer its own \
             id, got {again:?} against {first:?}"
        );
    }

    // Moved: the address is on the right interface, and the left one holds nothing.
    let moved = served(
        &router,
        case,
        "AssociateAddress",
        &[
            param("AllocationId", &allocation_id),
            param("NetworkInterfaceId", &right),
        ],
    )
    .await;
    let moved = ec2_query::element(&moved, "associationId").unwrap_or_default();
    assert_ne!(
        moved, first,
        "case {case:?}: a moved association gets its own id, got {moved:?}"
    );
    let stored = stored_address(&router, case, &allocation_id).await;
    assert_eq!(
        (
            stored.network_interface_id.as_deref(),
            stored.association_id.as_deref(),
            stored.private_ip_address.as_deref(),
        ),
        (
            Some(right.as_str()),
            Some(moved.as_str()),
            Some(right_addresses[0].as_str()),
        ),
        "case {case:?}: the address must be reported on the interface it moved to, \
         got {stored:?}"
    );
    let (left_association, _) =
        interface_association(&router, case, &left, &left_addresses[0]).await;
    assert_eq!(
        left_association, None,
        "case {case:?}: the interface the address left must report no association, \
         got {left_association:?}"
    );
    let (right_association, _) =
        interface_association(&router, case, &right, &right_addresses[0]).await;
    assert_eq!(
        right_association.and_then(|association| association.public_ip),
        Some(public_ip.clone()),
        "case {case:?}: the interface the address moved to must report it"
    );

    // Reassociation refused explicitly is the one request that is turned down, and it
    // changes nothing.
    let (status, body) = api(
        &router,
        "AssociateAddress",
        &[
            param("AllocationId", &allocation_id),
            param("NetworkInterfaceId", &left),
            param("AllowReassociation", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a refused reassociation must answer HTTP 400, got {status} with \
         body {body}"
    );
    let code = ec2_query::error_code(&body);
    assert_eq!(
        code, "Resource.AlreadyAssociated",
        "case {case:?}: expected Resource.AlreadyAssociated, got {code:?} with body {body}"
    );
    let message = ec2_query::error_message(&body);
    assert_eq!(
        message,
        format!("resource {allocation_id} is already associated with associate-id {moved}"),
        "case {case:?}: the refusal must name the association that is in the way, \
         got {message:?}"
    );
    let unchanged = stored_address(&router, case, &allocation_id).await;
    assert_eq!(
        unchanged.association_id.as_deref(),
        Some(moved.as_str()),
        "case {case:?}: a refused reassociation must leave the standing one, got {unchanged:?}"
    );
}

/// RULE: a `DisassociateAddress` request the service cannot serve is refused with EC2's own
/// error code and ends nothing — and a `DryRun` of one that would have succeeded ends
/// nothing either.
#[tokio::test]
async fn a_disassociation_it_cannot_serve_is_refused_and_ends_nothing() {
    let router = emulator();
    let case = "the standing association every row leaves alone";
    let (allocation_id, public_ip) = allocate(&router, case).await;
    let (instance_id, _private_ip) = launch(&router, case).await;
    let body = served(
        &router,
        case,
        "AssociateAddress",
        &[
            param("AllocationId", &allocation_id),
            param("InstanceId", &instance_id),
        ],
    )
    .await;
    let association_id = ec2_query::element(&body, "associationId").unwrap_or_default();

    // Rows: `(case, parameters, status, error code)`, each differing along what the refusal
    // generalises over — which member is missing, which pair is given together, what kind of
    // value is named, and the dry run of a request that would have worked.
    let rows: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "no association named at all",
            vec![],
            400,
            "MissingParameter",
        ),
        (
            "an association id no address of the account carries",
            vec![param("AssociationId", "eipassoc-0123456789abcdef0")],
            400,
            "InvalidAssociationID.NotFound",
        ),
        (
            "an association id of the short form the account does not carry",
            vec![param("AssociationId", "eipassoc-0a1b2c3d")],
            400,
            "InvalidAssociationID.NotFound",
        ),
        (
            "something that is no association id at all",
            vec![param("AssociationId", "the-first-one")],
            400,
            "InvalidAssociationID.NotFound",
        ),
        (
            "the association named twice, by association id and by public address",
            vec![
                param("AssociationId", &association_id),
                param("PublicIp", &public_ip),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a public address the account holds no allocation for",
            vec![param("PublicIp", "203.0.113.9")],
            400,
            "InvalidAddress.NotFound",
        ),
        (
            "a publicIp that is not an IPv4 address",
            vec![param("PublicIp", "ten.zero.zero.one")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a request that would have succeeded, under DryRun",
            vec![
                param("AssociationId", &association_id),
                param("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in &rows {
        let (status, body) = api(&router, "DisassociateAddress", params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = ec2_query::error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected the error code {expected_code}, got {code:?} with \
             body {body}"
        );
        let stored = stored_address(&router, case, &allocation_id).await;
        assert_eq!(
            stored.association_id.as_deref(),
            Some(association_id.as_str()),
            "case {case:?}: a refused disassociation must leave the association standing, \
             got {stored:?}"
        );
    }
}
