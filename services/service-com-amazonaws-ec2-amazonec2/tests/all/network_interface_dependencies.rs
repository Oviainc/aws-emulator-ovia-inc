// SPDX-License-Identifier: CC0-1.0

//! What a STORED NETWORK INTERFACE blocks: the deletes that are refused while an interface
//! still references the resource they name, driven over the ec2Query wire the service
//! speaks (`Action=<Operation>` in an `application/x-www-form-urlencoded` POST, answered
//! with XML — README.md / OPERATIONS.md "Wire protocol").
//!
//! "If you attempt to delete a security group that is associated with an instance or
//! network interface … the operation fails with DependencyViolation"
//! (`DeleteSecurityGroup`), and "you must detach or delete all gateways and resources that
//! are associated with the VPC before you can delete it" (`DeleteVpc`). The rule under test
//! is the one both sentences state: a delete is refused with `DependencyViolation`, and
//! performs nothing, while a STORED record still references the resource it names — a
//! network interface referencing its security groups, its subnet and its VPC for as long as
//! it is stored, whether or not it is attached to an instance — and the same delete succeeds
//! once that reference is gone.
//!
//! The rows vary what the rule generalises over: how many interfaces reference the group,
//! how many groups one interface references, whether the interface references the group
//! under test or another group of the same VPC, whether the request named the group by id
//! or by name, and which resource the refused delete named (group, subnet, VPC).
//!
//! The instance half of the same rule has its own suite (`delete_security_group`); this one
//! holds the interface half.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for: the Region the default network is seeded for.
const REGION: &str = "us-east-1";

/// The code a delete of a resource something still references is refused with.
const DEPENDENCY_VIOLATION: &str = "DependencyViolation";

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
            param(
                "GroupDescription",
                "a group the interface dependency suite created",
            ),
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

/// Delete the interface `network_interface_id`, which has to succeed.
async fn delete_network_interface(case: &str, router: &MultiServiceRouter, id: &str) {
    ok(
        case,
        router,
        "DeleteNetworkInterface",
        &[param("NetworkInterfaceId", id)],
    )
    .await;
}

/// Whether `DescribeSecurityGroups` still reports `group_id`: the store's own answer to
/// whether the delete happened, read back through the implemented describe rather than
/// inferred from the delete's response.
async fn group_is_stored(case: &str, router: &MultiServiceRouter, group_id: &str) -> bool {
    let (status, body) = call(
        router,
        "DescribeSecurityGroups",
        &indexed("GroupId", &[group_id]),
    )
    .await;
    match status {
        200 => true,
        400 => {
            assert_eq!(
                error_code(&body),
                "InvalidGroup.NotFound",
                "case {case:?}: a group that is gone must be refused with \
                 InvalidGroup.NotFound, got {body}"
            );
            false
        }
        other => panic!("case {case:?}: unexpected HTTP {other} from the describe: {body}"),
    }
}

/// Whether `DescribeSubnets` still reports `subnet_id`.
async fn subnet_is_stored(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> bool {
    let (status, body) = call(router, "DescribeSubnets", &indexed("SubnetId", &[subnet_id])).await;
    match status {
        200 => element(&body, "subnetId").is_some(),
        400 => false,
        other => panic!("case {case:?}: unexpected HTTP {other} from the describe: {body}"),
    }
}

/// Whether `DescribeVpcs` still reports `vpc_id`.
async fn vpc_is_stored(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> bool {
    let (status, body) = call(router, "DescribeVpcs", &indexed("VpcId", &[vpc_id])).await;
    match status {
        200 => element(&body, "vpcId").is_some(),
        400 => false,
        other => panic!("case {case:?}: unexpected HTTP {other} from the describe: {body}"),
    }
}

/// Which security groups the row's interfaces are created with.
#[derive(Clone, Copy, Debug)]
enum Referencing {
    /// Only the group under test.
    TheGroup,
    /// The group under test beside another group of the same VPC, so one interface
    /// referencing several groups holds every one of them.
    TheGroupAndAnother,
    /// Another group of the same VPC only, which must not make the group under test
    /// undeletable.
    AnotherGroup,
    /// No group at all: an interface created without groups gets its VPC's DEFAULT group,
    /// so it references that one and not the group under test.
    NoGroup,
}

/// How the delete request names the group under test.
#[derive(Clone, Copy, Debug)]
enum NamedBy {
    /// `GroupId`, the only form a nondefault VPC's group can be named by.
    Id,
    /// `GroupId` and `GroupName` together, the pair the handler resolves to one group.
    IdAndName,
}

/// One row of the dependency table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    vpc_cidr: &'static str,
    subnet_cidr: &'static str,
    /// How many interfaces the row creates.
    interfaces: usize,
    referencing: Referencing,
    named_by: NamedBy,
    /// Whether the delete has to be refused.
    refused: bool,
}

/// The name the row's group under test is created with.
const GROUP_NAME: &str = "interface-dependency";

/// RULE: `DeleteSecurityGroup` is refused with `DependencyViolation`, and deletes nothing,
/// while a stored network interface still references the group — and the same delete removes
/// the group once no interface does.
///
/// The rows differ in how many interfaces reference the group, how many groups one
/// interface references, whether they reference this group or another one of the same VPC,
/// and how the request names the group.
#[tokio::test]
async fn delete_security_group_refuses_while_a_network_interface_references_the_group() {
    let router = emulator();
    let rows = [
        Row {
            case: "one interface referencing the group",
            vpc_cidr: "10.94.0.0/16",
            subnet_cidr: "10.94.1.0/24",
            interfaces: 1,
            referencing: Referencing::TheGroup,
            named_by: NamedBy::Id,
            refused: true,
        },
        Row {
            case: "three interfaces referencing the group",
            vpc_cidr: "10.95.0.0/16",
            subnet_cidr: "10.95.1.0/24",
            interfaces: 3,
            referencing: Referencing::TheGroup,
            named_by: NamedBy::Id,
            refused: true,
        },
        Row {
            case: "one interface referencing the group beside another",
            vpc_cidr: "10.96.0.0/16",
            subnet_cidr: "10.96.1.0/24",
            interfaces: 1,
            referencing: Referencing::TheGroupAndAnother,
            named_by: NamedBy::Id,
            refused: true,
        },
        Row {
            case: "the group named by id and name while an interface references it",
            vpc_cidr: "10.97.0.0/16",
            subnet_cidr: "10.97.1.0/24",
            interfaces: 1,
            referencing: Referencing::TheGroup,
            named_by: NamedBy::IdAndName,
            refused: true,
        },
        Row {
            case: "an interface referencing another group of the same VPC",
            vpc_cidr: "10.98.0.0/16",
            subnet_cidr: "10.98.1.0/24",
            interfaces: 2,
            referencing: Referencing::AnotherGroup,
            named_by: NamedBy::Id,
            refused: false,
        },
        Row {
            case: "an interface created with no group of its own",
            vpc_cidr: "10.99.0.0/16",
            subnet_cidr: "10.99.1.0/24",
            interfaces: 1,
            referencing: Referencing::NoGroup,
            named_by: NamedBy::Id,
            refused: false,
        },
        Row {
            case: "no interface at all",
            vpc_cidr: "10.100.0.0/16",
            subnet_cidr: "10.100.1.0/24",
            interfaces: 0,
            referencing: Referencing::TheGroup,
            named_by: NamedBy::Id,
            refused: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let vpc_id = create_vpc(case, &router, row.vpc_cidr).await;
        let subnet_id = create_subnet(case, &router, &vpc_id, row.subnet_cidr).await;
        let group_id = create_security_group(case, &router, &vpc_id, GROUP_NAME).await;
        let other_group = create_security_group(case, &router, &vpc_id, "another-group").await;

        let mut created: Vec<String> = Vec::new();
        for _ in 0..row.interfaces {
            let groups: Vec<&str> = match row.referencing {
                Referencing::TheGroup => vec![group_id.as_str()],
                Referencing::TheGroupAndAnother => {
                    vec![group_id.as_str(), other_group.as_str()]
                }
                Referencing::AnotherGroup => vec![other_group.as_str()],
                Referencing::NoGroup => Vec::new(),
            };
            created.push(create_network_interface(case, &router, &subnet_id, &groups).await);
        }

        let mut params = vec![param("GroupId", &group_id)];
        if matches!(row.named_by, NamedBy::IdAndName) {
            params.push(param("GroupName", GROUP_NAME));
        }
        let (status, body) = call(&router, "DeleteSecurityGroup", &params).await;

        if row.refused {
            assert_eq!(
                status, 400,
                "case {case:?}: a group {created:?} references must be refused, got {status} \
                 with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, DEPENDENCY_VIOLATION,
                "case {case:?}: expected {DEPENDENCY_VIOLATION}, got {code:?} in body {body}"
            );
            // The refusal names the group the REQUEST named, and no dependent id.
            let message = error_message(&body);
            assert_eq!(
                message,
                format!("resource {group_id} has a dependent object"),
                "case {case:?}: expected the refusal to name only {group_id}, got {message:?}"
            );
            assert!(
                group_is_stored(case, &router, &group_id).await,
                "case {case:?}: a refused delete must leave {group_id} stored"
            );

            // And the same delete succeeds once nothing references the group any more.
            for network_interface_id in &created {
                delete_network_interface(case, &router, network_interface_id).await;
            }
            let body = ok(case, &router, "DeleteSecurityGroup", &params).await;
            let answered = reported(case, &body, "return");
            assert_eq!(
                answered, "true",
                "case {case:?}: expected the delete to report true once the interfaces are \
                 gone, got {answered:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a group nothing references ({created:?}) must be deletable, \
                 got {status} with body {body}"
            );
            let answered = reported(case, &body, "groupId");
            assert_eq!(
                answered, group_id,
                "case {case:?}: expected the deleted group {group_id:?}, got {answered:?}"
            );
        }

        assert!(
            !group_is_stored(case, &router, &group_id).await,
            "case {case:?}: {group_id} must be gone once the delete succeeded"
        );
    }
}

/// RULE (the same one on the other resource a stored interface references): `DeleteSubnet`
/// is refused with `DependencyViolation`, and deletes nothing, while an interface created in
/// that subnet is stored — and the same delete removes the subnet once no interface is in
/// it. An interface in ANOTHER subnet of the same VPC holds nothing in this one.
///
/// The rows differ in how many interfaces are in the subnet named and in whether they are in
/// that subnet at all; once they are gone the VPC, whose subnet was its last dependency, is
/// deletable too.
#[tokio::test]
async fn delete_subnet_refuses_while_a_network_interface_is_in_the_subnet() {
    let router = emulator();
    let rows: [(&str, usize, usize, bool); 3] = [
        ("one interface in the subnet named", 1, 0, true),
        ("two interfaces in the subnet named", 2, 0, true),
        ("one interface in the other subnet only", 0, 1, false),
    ];

    for (index, (case, here, elsewhere, refused)) in rows.into_iter().enumerate() {
        let vpc_cidr = format!("10.{}.0.0/16", 101 + index);
        let vpc_id = create_vpc(case, &router, &vpc_cidr).await;
        let subnet_id = create_subnet(case, &router, &vpc_id, &format!("10.{}.1.0/24", 101 + index))
            .await;
        let other_subnet =
            create_subnet(case, &router, &vpc_id, &format!("10.{}.2.0/24", 101 + index)).await;

        let mut created: Vec<String> = Vec::new();
        for _ in 0..here {
            created.push(create_network_interface(case, &router, &subnet_id, &[]).await);
        }
        for _ in 0..elsewhere {
            created.push(create_network_interface(case, &router, &other_subnet, &[]).await);
        }

        let params = vec![param("SubnetId", &subnet_id)];
        if refused {
            let (status, body) = call(&router, "DeleteSubnet", &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: the delete must be refused while {created:?} are stored, got \
                 {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, DEPENDENCY_VIOLATION,
                "case {case:?}: expected {DEPENDENCY_VIOLATION}, got {code:?} in body {body}"
            );
            assert!(
                subnet_is_stored(case, &router, &subnet_id).await,
                "case {case:?}: a refused delete must leave {subnet_id} stored"
            );
            for network_interface_id in &created {
                delete_network_interface(case, &router, network_interface_id).await;
            }
            created.clear();
        }
        ok(case, &router, "DeleteSubnet", &params).await;
        assert!(
            !subnet_is_stored(case, &router, &subnet_id).await,
            "case {case:?}: {subnet_id} must be gone after the delete"
        );

        // And the VPC, once its subnets and the interfaces in them are gone, is deletable:
        // the refusals above were the dependencies the caller had to clear, not a VPC that
        // can never be deleted.
        for network_interface_id in &created {
            delete_network_interface(case, &router, network_interface_id).await;
        }
        ok(
            case,
            &router,
            "DeleteSubnet",
            &[param("SubnetId", &other_subnet)],
        )
        .await;
        ok(case, &router, "DeleteVpc", &[param("VpcId", &vpc_id)]).await;
        assert!(
            !vpc_is_stored(case, &router, &vpc_id).await,
            "case {case:?}: {vpc_id} must be gone after its delete"
        );
    }
}
