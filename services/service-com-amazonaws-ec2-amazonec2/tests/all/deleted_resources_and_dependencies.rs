// SPDX-License-Identifier: CC0-1.0

//! What a DELETE does to the state behind it, driven over the ec2Query wire the service
//! speaks (`Action=<Operation>` in an `application/x-www-form-urlencoded` POST, answered
//! with XML — README.md / OPERATIONS.md "Wire protocol").
//!
//! Two rules, one per suite below.
//!
//! RULE 1 — a delete of a resource something stored still holds is refused with HTTP 400
//! `DependencyViolation`, performs nothing, and the refusal carries the message AWS words
//! for that error class, which names the resource THE REQUEST named and no dependent id
//! ("you must detach or delete all gateways and resources that are associated with the VPC
//! before you can delete it" — `DeleteVpc`; "you must terminate all running instances in
//! the subnet before you can delete the subnet" — `DeleteSubnet`; "if you attempt to
//! delete a security group that is associated with an instance or network interface, is
//! referenced by another security group in the same VPC … the operation fails with
//! `DependencyViolation`" — `DeleteSecurityGroup`). The same delete then succeeds once
//! nothing holds the resource.
//!
//! RULE 2 — once a delete succeeds, the resource is GONE from this account's state: every
//! later read of that id answers HTTP 400 with the resource's own not-found code, and so
//! does a second delete. A waiter that polls for a deleted resource to disappear has
//! nothing else to stop on.
//!
//! The rows of each suite vary what the rule generalises over: which resource kind is
//! deleted, which kind of stored record holds it, and which call reads it back.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{document, param, Param};

/// The Region every call here is signed for: the Region the default network is seeded for.
const REGION: &str = "us-east-1";

/// The code a delete of a resource something still holds is refused with.
const DEPENDENCY_VIOLATION: &str = "DependencyViolation";

/// An Availability Zone of [`REGION`], for the resources that are created in one.
const ZONE: &str = "us-east-1a";

/// The HTTP status and code EC2 answers a dry run of a request that would have succeeded
/// with: "if you have the required permissions, the error response is DryRunOperation"
/// (`DryRun`).
const DRY_RUN_STATUS: u16 = 412;
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn reported(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The text of the `name` element INSIDE the first `nested_in` element, or of the first
/// `name` element anywhere when the rule's source is not nested.
///
/// A referenced group's id is reported inside the rule's `referencedGroupInfo`, beside the
/// rule's OWN `groupId`, so that one source has to be read through the document rather than
/// by taking the first element of that name.
fn nested(case: &str, body: &str, nested_in: Option<&str>, name: &str) -> String {
    let parsed = document(case, body);
    let scope = match nested_in {
        Some(outer) => parsed
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == outer)
            .unwrap_or_else(|| panic!("case {case:?}: no <{outer}> in {body}")),
        None => parsed.root_element(),
    };
    scope
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
        .to_string()
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    reported(case, &body, "vpcId")
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    reported(case, &body, "subnetId")
}

/// A security group of `vpc_id` named `group_name`, as its id.
async fn create_security_group(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    group_name: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("VpcId", vpc_id),
            param("GroupName", group_name),
            param("GroupDescription", "a group the delete suite created"),
        ],
    )
    .await;
    reported(case, &body, "groupId")
}

/// A network interface in `subnet_id` referencing `groups`, as its id.
async fn create_network_interface(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
    groups: &[&str],
) -> String {
    let mut params = vec![param("SubnetId", subnet_id)];
    params.extend(indexed("SecurityGroupId", groups));
    let body = ok(case, router, "CreateNetworkInterface", &params).await;
    reported(case, &body, "networkInterfaceId")
}

/// A carrier gateway attached to `vpc_id`, as its id.
async fn create_carrier_gateway(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateCarrierGateway",
        &[param("VpcId", vpc_id)],
    )
    .await;
    reported(case, &body, "carrierGatewayId")
}

/// Whether `describe` still reports the resource `id`, read through the implemented
/// describe rather than inferred from the delete's own answer. A describe that refuses the
/// id has to refuse it with `not_found`, which is how a caller tells a resource that is
/// gone from one it never had.
async fn is_stored(
    case: &str,
    router: &MultiServiceRouter,
    describe: &str,
    id_param: &str,
    id: &str,
    not_found: &str,
) -> bool {
    let (status, body) = call(router, describe, &indexed(id_param, &[id])).await;
    match status {
        200 => body.contains(id),
        400 => {
            let code = error_code(&body);
            assert_eq!(
                code, not_found,
                "case {case:?}: {describe} must refuse the gone {id} with {not_found}, got \
                 {code:?} in {body}"
            );
            false
        }
        other => panic!("case {case:?}: unexpected HTTP {other} from {describe}: {body}"),
    }
}

/// Which kind of stored record holds the resource whose delete is refused.
#[derive(Debug, Clone, Copy)]
enum HeldBy {
    /// A VPC held by a subnet of its own.
    VpcBySubnet,
    /// A VPC held by a carrier gateway attached to it — a gateway "associated with the
    /// VPC" that `DeleteVpc` does not remove.
    VpcByCarrierGateway,
    /// A subnet holding a network interface.
    SubnetByInterface,
    /// A group a network interface references.
    GroupByInterface,
    /// A group another group of the same VPC references in an INBOUND rule.
    GroupByReferencingRule,
}

/// One row of the dependency table.
#[derive(Debug)]
struct Refused {
    case: &'static str,
    held_by: HeldBy,
    /// The `/16` the row's own VPC is created with, so rows cannot collide.
    vpc_cidr: &'static str,
    subnet_cidr: &'static str,
}

/// What one row of the dependency table does once its resources are in place: the delete
/// under test, the refusal it has to answer, the describe that says whether it happened,
/// and the calls that let the dependency go.
#[derive(Debug)]
struct Plan {
    /// The delete under test, and the member it names its resource by.
    delete: &'static str,
    id_param: &'static str,
    /// The resource the delete names.
    target: String,
    /// The message AWS words for the refusal of THIS resource.
    message: String,
    /// The describe that reports the resource, the member it names it by, and the code it
    /// answers once the resource is gone.
    describe: &'static str,
    describe_param: &'static str,
    not_found: &'static str,
    /// The calls that take the dependency away, after which the delete has to succeed.
    release: Vec<(&'static str, Vec<Param>)>,
}

/// RULE 1: a delete is refused with `DependencyViolation` — carrying the message AWS words
/// for that error class, which names only the resource the request named — while a stored
/// record still holds the resource, performs nothing, and succeeds once nothing holds it.
///
/// The rows differ in the resource deleted (VPC, subnet, security group) and in what holds
/// it (a subnet, a carrier gateway, a network interface, another group's inbound rule).
#[tokio::test]
async fn a_delete_is_refused_while_a_stored_record_holds_the_resource() {
    let router = emulator();
    let rows = [
        Refused {
            case: "a VPC its own subnet holds",
            held_by: HeldBy::VpcBySubnet,
            vpc_cidr: "10.120.0.0/16",
            subnet_cidr: "10.120.1.0/24",
        },
        Refused {
            case: "a VPC a carrier gateway is attached to",
            held_by: HeldBy::VpcByCarrierGateway,
            vpc_cidr: "10.121.0.0/16",
            subnet_cidr: "10.121.1.0/24",
        },
        Refused {
            case: "a subnet holding a network interface",
            held_by: HeldBy::SubnetByInterface,
            vpc_cidr: "10.122.0.0/16",
            subnet_cidr: "10.122.1.0/24",
        },
        Refused {
            case: "a group a network interface references",
            held_by: HeldBy::GroupByInterface,
            vpc_cidr: "10.123.0.0/16",
            subnet_cidr: "10.123.1.0/24",
        },
        Refused {
            case: "a group another group's inbound rule references",
            held_by: HeldBy::GroupByReferencingRule,
            vpc_cidr: "10.124.0.0/16",
            subnet_cidr: "10.124.1.0/24",
        },
    ];

    for row in rows {
        let case = row.case;
        let vpc_id = create_vpc(case, &router, row.vpc_cidr).await;
        let subnet_id = create_subnet(case, &router, &vpc_id, row.subnet_cidr).await;

        // What the row deletes, what holds it, the message AWS words for the refusal, the
        // describe that says whether the delete happened, and the calls that let the
        // dependency go.
        let plan: Plan = match row.held_by {
            HeldBy::VpcBySubnet => Plan {
                delete: "DeleteVpc",
                id_param: "VpcId",
                target: vpc_id.clone(),
                message: format!("The vpc '{vpc_id}' has dependencies and cannot be deleted."),
                describe: "DescribeVpcs",
                describe_param: "VpcId",
                not_found: "InvalidVpcID.NotFound",
                release: vec![("DeleteSubnet", vec![param("SubnetId", &subnet_id)])],
            },
            HeldBy::VpcByCarrierGateway => {
                let carrier_gateway = create_carrier_gateway(case, &router, &vpc_id).await;
                Plan {
                    delete: "DeleteVpc",
                    id_param: "VpcId",
                    target: vpc_id.clone(),
                    message: format!(
                        "The vpc '{vpc_id}' has dependencies and cannot be deleted."
                    ),
                    describe: "DescribeVpcs",
                    describe_param: "VpcId",
                    not_found: "InvalidVpcID.NotFound",
                    release: vec![
                        ("DeleteSubnet", vec![param("SubnetId", &subnet_id)]),
                        (
                            "DeleteCarrierGateway",
                            vec![param("CarrierGatewayId", &carrier_gateway)],
                        ),
                    ],
                }
            }
            HeldBy::SubnetByInterface => {
                let interface = create_network_interface(case, &router, &subnet_id, &[]).await;
                Plan {
                    delete: "DeleteSubnet",
                    id_param: "SubnetId",
                    target: subnet_id.clone(),
                    message: format!(
                        "The subnet '{subnet_id}' has dependencies and cannot be deleted."
                    ),
                    describe: "DescribeSubnets",
                    describe_param: "SubnetId",
                    not_found: "InvalidSubnetID.NotFound",
                    release: vec![(
                        "DeleteNetworkInterface",
                        vec![param("NetworkInterfaceId", &interface)],
                    )],
                }
            }
            HeldBy::GroupByInterface => {
                let group_id = create_security_group(case, &router, &vpc_id, "held-group").await;
                let interface =
                    create_network_interface(case, &router, &subnet_id, &[group_id.as_str()]).await;
                Plan {
                    delete: "DeleteSecurityGroup",
                    id_param: "GroupId",
                    target: group_id.clone(),
                    message: format!("resource {group_id} has a dependent object"),
                    describe: "DescribeSecurityGroups",
                    describe_param: "GroupId",
                    not_found: "InvalidGroup.NotFound",
                    release: vec![(
                        "DeleteNetworkInterface",
                        vec![param("NetworkInterfaceId", &interface)],
                    )],
                }
            }
            HeldBy::GroupByReferencingRule => {
                let group_id = create_security_group(case, &router, &vpc_id, "held-group").await;
                let other = create_security_group(case, &router, &vpc_id, "holding-group").await;
                // The inbound rule of ANOTHER group that references this one: "is
                // referenced by another security group in the same VPC"
                // (`DeleteSecurityGroup`).
                let body = ok(
                    case,
                    &router,
                    "AuthorizeSecurityGroupIngress",
                    &[
                        param("GroupId", &other),
                        param("IpPermissions.1.IpProtocol", "tcp"),
                        param("IpPermissions.1.FromPort", "443"),
                        param("IpPermissions.1.ToPort", "443"),
                        param("IpPermissions.1.Groups.1.GroupId", &group_id),
                    ],
                )
                .await;
                let rule_id = reported(case, &body, "securityGroupRuleId");
                Plan {
                    delete: "DeleteSecurityGroup",
                    id_param: "GroupId",
                    target: group_id.clone(),
                    message: format!("resource {group_id} has a dependent object"),
                    describe: "DescribeSecurityGroups",
                    describe_param: "GroupId",
                    not_found: "InvalidGroup.NotFound",
                    release: vec![(
                        "RevokeSecurityGroupIngress",
                        vec![
                            param("GroupId", &other),
                            param("SecurityGroupRuleId.1", &rule_id),
                        ],
                    )],
                }
            }
        };
        let Plan {
            delete,
            id_param,
            target,
            message,
            describe,
            describe_param,
            not_found,
            release,
        } = plan;

        let delete_params = vec![param(id_param, &target)];
        let (status, body) = call(&router, delete, &delete_params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the delete of {target} must be refused while it is held, got \
             {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, DEPENDENCY_VIOLATION,
            "case {case:?}: expected {DEPENDENCY_VIOLATION} for {target}, got {code:?} in \
             body {body}"
        );
        let answered = error_message(&body);
        assert_eq!(
            answered, message,
            "case {case:?}: expected the refusal of {target} to read {message:?}, got \
             {answered:?}"
        );
        assert!(
            is_stored(case, &router, describe, describe_param, &target, not_found).await,
            "case {case:?}: a refused delete must leave {target} stored"
        );

        // And the same delete succeeds once nothing holds the resource any more.
        for (action, params) in &release {
            ok(case, &router, action, params).await;
        }
        ok(case, &router, delete, &delete_params).await;
        assert!(
            !is_stored(case, &router, describe, describe_param, &target, not_found).await,
            "case {case:?}: {target} must be gone once its delete succeeded"
        );
    }
}

/// One row of the deleted-resource table: a resource kind, the calls that create, delete
/// and read it, and the code every read of it answers once it is gone.
#[derive(Debug)]
struct Deleted {
    case: &'static str,
    create: &'static str,
    /// The form fields of the create, beside the VPC id where the resource needs one.
    create_params: Vec<Param>,
    /// Whether the create's parameters carry the suite's VPC.
    in_vpc: bool,
    /// The element the create answers the new resource's id in.
    id_element: &'static str,
    delete: &'static str,
    /// The member the delete names the resource by.
    delete_param: &'static str,
    /// The state the delete answers the resource with, for the deletes that report one.
    deleted_state: Option<&'static str>,
    describe: &'static str,
    /// The member the describe names the resource by, flattened as `<member>.1`.
    describe_param: &'static str,
    not_found: &'static str,
}

/// RULE 2: a deleted resource is gone from this account's state — the describe that
/// reported it answers HTTP 400 with the resource's own not-found code for that id, and so
/// does a second delete.
///
/// The rows differ in the resource kind (an EBS volume, an Elastic IP allocation, a carrier
/// gateway, a subnet), which is also what varies the call that creates it, the call that
/// deletes it, the call that reads it back and the code each answers.
#[tokio::test]
async fn a_deleted_resource_is_not_readable_any_more() {
    let router = emulator();
    let vpc_id = create_vpc("the deleted-resource table", &router, "10.125.0.0/16").await;
    let rows = [
        Deleted {
            case: "an EBS volume",
            create: "CreateVolume",
            create_params: vec![param("AvailabilityZone", ZONE), param("Size", "8")],
            in_vpc: false,
            id_element: "volumeId",
            delete: "DeleteVolume",
            delete_param: "VolumeId",
            deleted_state: None,
            describe: "DescribeVolumes",
            describe_param: "VolumeId",
            not_found: "InvalidVolume.NotFound",
        },
        Deleted {
            case: "an Elastic IP allocation",
            create: "AllocateAddress",
            create_params: vec![param("Domain", "vpc")],
            in_vpc: false,
            id_element: "allocationId",
            delete: "ReleaseAddress",
            delete_param: "AllocationId",
            deleted_state: None,
            describe: "DescribeAddresses",
            describe_param: "AllocationId",
            not_found: "InvalidAllocationID.NotFound",
        },
        Deleted {
            case: "a carrier gateway",
            create: "CreateCarrierGateway",
            create_params: Vec::new(),
            in_vpc: true,
            id_element: "carrierGatewayId",
            delete: "DeleteCarrierGateway",
            delete_param: "CarrierGatewayId",
            // "Information about the carrier gateway"
            // (`DeleteCarrierGatewayResult$CarrierGateway`) as the delete leaves it.
            deleted_state: Some("deleting"),
            describe: "DescribeCarrierGateways",
            describe_param: "CarrierGatewayId",
            not_found: "InvalidCarrierGatewayID.NotFound",
        },
        Deleted {
            case: "a subnet",
            create: "CreateSubnet",
            create_params: vec![param("CidrBlock", "10.125.4.0/24")],
            in_vpc: true,
            id_element: "subnetId",
            delete: "DeleteSubnet",
            delete_param: "SubnetId",
            deleted_state: None,
            describe: "DescribeSubnets",
            describe_param: "SubnetId",
            not_found: "InvalidSubnetID.NotFound",
        },
    ];

    for row in rows {
        let case = row.case;
        let mut create_params = row.create_params.clone();
        if row.in_vpc {
            create_params.push(param("VpcId", &vpc_id));
        }
        let body = ok(case, &router, row.create, &create_params).await;
        let id = reported(case, &body, row.id_element);

        // The resource is readable before the delete, so the refusal below is the DELETE's
        // doing rather than a describe that never reported it.
        assert!(
            is_stored(
                case,
                &router,
                row.describe,
                row.describe_param,
                &id,
                row.not_found
            )
            .await,
            "case {case:?}: {id} must be readable before it is deleted"
        );

        let delete_params = vec![param(row.delete_param, &id)];

        // "Checks whether you have the required permissions for the action, without
        // actually making the request … the error response is DryRunOperation" (`DryRun`):
        // the probe answers that, and the resource is still readable afterwards, so the
        // delete below is what removes it.
        let mut probe = delete_params.clone();
        probe.push(param("DryRun", "true"));
        let (status, body) = call(&router, row.delete, &probe).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} from a dry-run {delete} of {id}, \
             got {status} with body {body}",
            delete = row.delete
        );
        let code = error_code(&body);
        assert_eq!(
            code, DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION} from the dry run, got {code:?} in \
             body {body}"
        );
        assert!(
            is_stored(
                case,
                &router,
                row.describe,
                row.describe_param,
                &id,
                row.not_found
            )
            .await,
            "case {case:?}: a dry run must leave {id} stored"
        );

        let body = ok(case, &router, row.delete, &delete_params).await;
        if let Some(expected) = row.deleted_state {
            let state = reported(case, &body, "state");
            assert_eq!(
                state, expected,
                "case {case:?}: expected the delete of {id} to report the state {expected:?}, \
                 got {state:?}"
            );
        }

        assert!(
            !is_stored(
                case,
                &router,
                row.describe,
                row.describe_param,
                &id,
                row.not_found
            )
            .await,
            "case {case:?}: {id} must be gone from {describe} once it is deleted",
            describe = row.describe
        );

        // A second delete is refused rather than answered as a no-op: a caller that deletes
        // twice has to be able to tell the deletion it performed from one it did not.
        let (status, body) = call(&router, row.delete, &delete_params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second {delete} of {id} must be refused, got {status} with body \
             {body}",
            delete = row.delete
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            row.not_found,
            "case {case:?}: expected {expected} from the second delete of {id}, got {code:?} \
             in body {body}",
            expected = row.not_found
        );
    }
}

/// `ReleaseAddress` names the address by its allocation AND by the network border group it
/// is advertised from: "if you provide an incorrect network border group, you receive an
/// InvalidAddress.NotFound error" (`ReleaseAddressRequest$NetworkBorderGroup`) — so a group
/// that is not the address's own names no address of this account, and the allocation is
/// still there afterwards. The rows vary the group named: the address's own (released), a
/// group of another Region, and one that is not a border group at all.
#[tokio::test]
async fn releasing_an_address_under_a_foreign_network_border_group_is_refused() {
    let router = emulator();
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("another Region's border group", Some("eu-west-1"), false),
        ("a value that is no border group", Some("not-a-group"), false),
        ("the address's own border group", None, true),
    ];

    for (case, named, released) in rows {
        let body = ok(case, &router, "AllocateAddress", &[param("Domain", "vpc")]).await;
        let allocation_id = reported(case, &body, "allocationId");
        // The address's own group is the one the allocation reported, never a value this
        // suite spells out: the emulator derives it from the Region the call was signed for.
        let own = reported(case, &body, "networkBorderGroup");
        let group = named.unwrap_or(own.as_str());

        let params = vec![
            param("AllocationId", &allocation_id),
            param("NetworkBorderGroup", group),
        ];
        if released {
            ok(case, &router, "ReleaseAddress", &params).await;
        } else {
            let (status, body) = call(&router, "ReleaseAddress", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: releasing {allocation_id} under {group:?} must be refused, got \
                 {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidAddress.NotFound",
                "case {case:?}: expected InvalidAddress.NotFound, got {code:?} in body {body}"
            );
        }
        assert_eq!(
            is_stored(
                case,
                &router,
                "DescribeAddresses",
                "AllocationId",
                &allocation_id,
                "InvalidAllocationID.NotFound"
            )
            .await,
            !released,
            "case {case:?}: {allocation_id} must be stored exactly while it is not released"
        );
    }
}

/// RULE: a permission that names no grant at all describes no rule, and the refusal names
/// the kind of grant ITS OWN direction documents — "you must specify exactly one of the
/// following sources" for an inbound rule (`AuthorizeSecurityGroupIngress`) and
/// "destinations" for an outbound one (`AuthorizeSecurityGroupEgress`) — so the two
/// directions share the reading without sharing a word that is wrong for one of them.
///
/// The rows are the two directions; each sends the same permission (a protocol and a port
/// range, no IPv4 range, no IPv6 range, no prefix list, no group).
#[tokio::test]
async fn an_authorize_that_names_no_grant_is_refused_in_its_own_direction_s_words() {
    let router = emulator();
    let case = "the grant-less permission table";
    let vpc_id = create_vpc(case, &router, "10.127.0.0/16").await;
    let group_id = create_security_group(case, &router, &vpc_id, "grant-less").await;
    let rows = [
        ("AuthorizeSecurityGroupIngress", "source"),
        ("AuthorizeSecurityGroupEgress", "destination"),
    ];

    for (action, grant) in rows {
        let case = action;
        let (status, body) = call(
            &router,
            action,
            &[
                param("GroupId", &group_id),
                param("IpPermissions.1.IpProtocol", "tcp"),
                param("IpPermissions.1.FromPort", "443"),
                param("IpPermissions.1.ToPort", "443"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a permission naming no {grant} must be refused, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "MissingParameter",
            "case {case:?}: expected MissingParameter, got {code:?} in body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&format!("must name a {grant} for ipPermissions.1")),
            "case {case:?}: expected the refusal to ask for a {grant}, got {message:?}"
        );
    }
}

/// What one authorized inbound rule is expected to read as once it is stored.
#[derive(Debug)]
struct Authorized {
    case: &'static str,
    /// The form fields describing the rule, beside the group the rules are added to.
    params: Vec<Param>,
    /// The element the stored rule's source is reported in, and the value it carries.
    source_element: &'static str,
    /// The element the source's own is nested inside, where it is nested.
    nested_in: Option<&'static str>,
    source: String,
    protocol: &'static str,
}

/// RULE 2's other half for the operation this fix had to implement to reach it:
/// `AuthorizeSecurityGroupIngress` stores the inbound rules a request describes, so
/// `DescribeSecurityGroupRules` reads back exactly what was added — and a second authorize
/// of the same rule is refused with `InvalidPermission.Duplicate`.
///
/// The rows differ in how the request describes the rule: an `IpPermissions` entry naming
/// an IPv4 range, one naming an IPv6 range, one referencing another security group, and the
/// flat `CidrIp`/`IpProtocol`/`FromPort`/`ToPort` form, which describes one rule on its own.
#[tokio::test]
async fn authorize_security_group_ingress_stores_the_rules_the_request_describes() {
    let router = emulator();
    let case = "the inbound rule table";
    let vpc_id = create_vpc(case, &router, "10.126.0.0/16").await;
    let referenced = create_security_group(case, &router, &vpc_id, "referenced-group").await;

    let rows = [
        Authorized {
            case: "an IPv4 range",
            params: vec![
                param("IpPermissions.1.IpProtocol", "tcp"),
                param("IpPermissions.1.FromPort", "80"),
                param("IpPermissions.1.ToPort", "80"),
                param("IpPermissions.1.IpRanges.1.CidrIp", "10.0.0.0/8"),
            ],
            source_element: "cidrIpv4",
            nested_in: None,
            source: "10.0.0.0/8".to_string(),
            protocol: "tcp",
        },
        Authorized {
            case: "an IPv6 range",
            params: vec![
                param("IpPermissions.1.IpProtocol", "udp"),
                param("IpPermissions.1.FromPort", "53"),
                param("IpPermissions.1.ToPort", "53"),
                param("IpPermissions.1.Ipv6Ranges.1.CidrIpv6", "2001:db8::/32"),
            ],
            source_element: "cidrIpv6",
            nested_in: None,
            source: "2001:db8::/32".to_string(),
            protocol: "udp",
        },
        Authorized {
            case: "another security group",
            params: vec![
                param("IpPermissions.1.IpProtocol", "-1"),
                param("IpPermissions.1.Groups.1.GroupId", &referenced),
            ],
            source_element: "groupId",
            nested_in: Some("referencedGroupInfo"),
            source: referenced.clone(),
            protocol: "-1",
        },
        Authorized {
            case: "the flat single-rule form",
            params: vec![
                param("IpProtocol", "tcp"),
                param("FromPort", "22"),
                param("ToPort", "22"),
                param("CidrIp", "192.168.0.0/16"),
            ],
            source_element: "cidrIpv4",
            nested_in: None,
            source: "192.168.0.0/16".to_string(),
            protocol: "tcp",
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        // A group per row, so one row's rules are never another's duplicates.
        let group_id =
            create_security_group(case, &router, &vpc_id, &format!("inbound-{index}")).await;
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(row.params.iter().cloned());

        let body = ok(case, &router, "AuthorizeSecurityGroupIngress", &params).await;
        let answered = reported(case, &body, "return");
        assert_eq!(
            answered, "true",
            "case {case:?}: expected the authorize to report true, got {answered:?}"
        );
        let rule_id = reported(case, &body, "securityGroupRuleId");
        let source = nested(case, &body, row.nested_in, row.source_element);
        assert_eq!(
            source, row.source,
            "case {case:?}: expected the added rule to name {expected:?}, got {source:?} in \
             body {body}",
            expected = row.source
        );

        // Read the stored rule back, so the answer above is what a later caller sees rather
        // than a reading of the request.
        let body = ok(
            case,
            &router,
            "DescribeSecurityGroupRules",
            &indexed("SecurityGroupRuleId", &[&rule_id]),
        )
        .await;
        let stored_source = nested(case, &body, row.nested_in, row.source_element);
        assert_eq!(
            stored_source, row.source,
            "case {case:?}: expected the stored rule to name {expected:?}, got \
             {stored_source:?} in body {body}",
            expected = row.source
        );
        let stored_protocol = reported(case, &body, "ipProtocol");
        assert_eq!(
            stored_protocol, row.protocol,
            "case {case:?}: expected the stored rule's protocol {expected:?}, got \
             {stored_protocol:?}",
            expected = row.protocol
        );
        let is_egress = reported(case, &body, "isEgress");
        assert_eq!(
            is_egress, "false",
            "case {case:?}: an authorized INBOUND rule must report isEgress false, got \
             {is_egress:?}"
        );

        // "The specified rule already exists for that security group"
        // (`InvalidPermission.Duplicate`): the same request a second time adds nothing.
        let (status, body) = call(&router, "AuthorizeSecurityGroupIngress", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a repeat of the same authorize must be refused, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidPermission.Duplicate",
            "case {case:?}: expected InvalidPermission.Duplicate, got {code:?} in body {body}"
        );
    }
}
