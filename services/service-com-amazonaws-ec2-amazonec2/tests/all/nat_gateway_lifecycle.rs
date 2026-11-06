// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateNatGateway` and `DeleteNatGateway`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. a create PERSISTS the gateway it answered, in the subnet the request named: the very
//!    next `DescribeNatGateways` reports the SAME record — its `nat-` id, that subnet and its
//!    VPC, the connectivity it asked for, the tags it assigned, and one address per Elastic IP
//!    address it was created with, each on the gateway's own network interface with a private
//!    address out of the subnet's block. "This action creates a network interface in the
//!    specified subnet with a private IP address from the IP address range of the subnet"
//!    (`CreateNatGateway`);
//! 2. the association is recorded on BOTH sides: `DescribeAddresses` reports the Elastic IP
//!    address as held by that interface, so the gateway and the address cannot disagree about
//!    who holds it;
//! 3. the CLOCK finishes the create: a gateway answered in the non-terminal `pending` state
//!    reaches `available` one `TRANSITION_DELAY` later, whether or not anybody calls again;
//! 4. a `ClientToken` makes the create idempotent — "Unique, case-sensitive identifier that
//!    you provide to ensure the idempotency of the request" (`$ClientToken`) — so a repeat
//!    answers the first gateway and the account holds one;
//! 5. `DeleteNatGateway` moves the gateway to `deleting` and the clock owes it `deleted`;
//!    "deleting a public NAT gateway disassociates its Elastic IP address, but does not
//!    release the address from your account", so the allocation stays in the account and is
//!    free to be released;
//! 6. a request naming a subnet the account does not hold, a public gateway with no Elastic IP
//!    address, a private one with one, an address something already holds, or a private
//!    address the subnet cannot give is refused with EC2's own error code and stores NOTHING;
//!    a `DryRun` of either operation answers `DryRunOperation` and changes nothing.
//!
//! Each table row is a distinct input along what those rules generalise over — the
//! connectivity type, how many Elastic IP addresses the gateway is created with, whether its
//! private address is named or chosen, which subnet of which VPC it sits in, and whether the
//! create carries tags — so no single recorded request is the specification.

use std::time::Duration;

use ec2::state::{
    AVAILABILITY_MODE_REGIONAL, AVAILABILITY_MODE_ZONAL, CONNECTIVITY_TYPE_PRIVATE,
    CONNECTIVITY_TYPE_PUBLIC, NAT_GATEWAY_STATE_AVAILABLE, NAT_GATEWAY_STATE_DELETED,
    NAT_GATEWAY_STATE_DELETING, NAT_GATEWAY_STATE_PENDING, NAT_GATEWAY_TAGGABLE_RESOURCE,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// EC2's error code for a subnet id the account holds no subnet for.
const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// EC2's error code for a parameter the operation needs and the request left out.
const MISSING_PARAMETER: &str = "MissingParameter";

/// EC2's error code for members that cannot be given together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// EC2's error code for a value the service rejects.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a resource that is already attached to something.
const RESOURCE_ALREADY_ASSOCIATED: &str = "Resource.AlreadyAssociated";

/// EC2's error code for a NAT gateway id the account holds no NAT gateway for.
const NAT_GATEWAY_NOT_FOUND: &str = "NatGatewayNotFound";

/// The code a dry run of a request that would have succeeded answers, and the status EC2
/// answers it with (HTTP 412, as every other dry run of this service does).
const DRY_RUN_OPERATION: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call of `action` carrying `params`.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A VPC of `cidr_block` and a subnet of `subnet_cidr` in it, both created over the wire, as
/// `(vpc id, subnet id)`.
async fn network(
    router: &MultiServiceRouter,
    case: &str,
    cidr_block: &str,
    subnet_cidr: &str,
    zone: &str,
) -> (String, String) {
    let (status, body) = ec2(router, "CreateVpc", &[p("CidrBlock", cidr_block)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateVpc({cidr_block}) must answer 200, got {status} {body}"
    );
    let vpc_id = element(&body, "vpcId").unwrap_or_default();
    let (status, body) = ec2(
        router,
        "CreateSubnet",
        &[
            p("VpcId", vpc_id.clone()),
            p("CidrBlock", subnet_cidr),
            p("AvailabilityZone", zone),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSubnet({subnet_cidr}) must answer 200, got {status} {body}"
    );
    let subnet_id = element(&body, "subnetId").unwrap_or_default();
    (vpc_id, subnet_id)
}

/// Allocate one Elastic IP address out of Amazon's pool, as `(allocation id, address)`.
async fn allocate(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let (status, body) = ec2(router, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: AllocateAddress must answer 200, got {status} {body}"
    );
    (
        element(&body, "allocationId").unwrap_or_default(),
        element(&body, "publicIp").unwrap_or_default(),
    )
}

/// The `<natGateway>` state a `DescribeNatGateways` of one gateway reports, and the
/// allocation ids its addresses carry.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    nat_gateway_id: &str,
) -> (String, Vec<String>) {
    let (status, body) = ec2(
        router,
        "DescribeNatGateways",
        &[p("NatGatewayId.1", nat_gateway_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNatGateways({nat_gateway_id}) must answer 200, got {status} \
         {body}"
    );
    (
        element(&body, "state").unwrap_or_default(),
        elements(&body, "allocationId"),
    )
}

/// One row: the gateway a create asks for, and what the account must hold afterwards.
struct CreateCase {
    case: &'static str,
    /// The VPC's block and the subnet's, and the zone the subnet is in.
    network: (&'static str, &'static str, &'static str),
    /// Its `ConnectivityType`, or `None` to leave the member out — "the default is public
    /// connectivity" (`$ConnectivityType`).
    connectivity_type: Option<&'static str>,
    /// How many Elastic IP addresses the create is made with: the first is the gateway's
    /// primary allocation, the rest its `SecondaryAllocationIds`.
    allocations: usize,
    /// Whether the create names the gateway's private address itself rather than leaving EC2
    /// to choose one out of the subnet.
    names_private_address: bool,
    /// The tags the create assigns.
    tags: &'static [(&'static str, &'static str)],
}

const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        case: "a public gateway with one address, no connectivity member and two tags",
        network: ("10.80.0.0/16", "10.80.1.0/24", "us-east-1a"),
        connectivity_type: None,
        allocations: 1,
        names_private_address: false,
        tags: &[("Name", "nat a"), ("team", "platform")],
    },
    CreateCase {
        case: "a public gateway with a secondary address and a private address it names",
        network: ("172.21.0.0/20", "172.21.1.0/24", "us-east-1b"),
        connectivity_type: Some(CONNECTIVITY_TYPE_PUBLIC),
        allocations: 2,
        names_private_address: true,
        tags: &[],
    },
    CreateCase {
        case: "a private gateway with no address at all",
        network: ("192.168.0.0/18", "192.168.4.0/22", "us-east-1c"),
        connectivity_type: Some(CONNECTIVITY_TYPE_PRIVATE),
        allocations: 0,
        names_private_address: false,
        tags: &[("Tier", "private")],
    },
];

#[tokio::test]
async fn a_created_nat_gateway_persists_and_comes_up() {
    for row in CREATE_CASES {
        let case = row.case;
        let router = emulator();
        let (vpc_id, subnet_id) =
            network(&router, case, row.network.0, row.network.1, row.network.2).await;

        let mut allocation_ids = Vec::new();
        for _ in 0..row.allocations {
            allocation_ids.push(allocate(&router, case).await);
        }
        let mut params: Params = vec![p("SubnetId", subnet_id.clone())];
        if let Some(connectivity_type) = row.connectivity_type {
            params.push(p("ConnectivityType", connectivity_type));
        }
        for (index, (allocation_id, _)) in allocation_ids.iter().enumerate() {
            match index {
                0 => params.push(p("AllocationId", allocation_id.clone())),
                _ => params.push(p(
                    &format!("SecondaryAllocationId.{index}"),
                    allocation_id.clone(),
                )),
            }
        }
        // A private address of the subnet that an interface may be given: the fifth address
        // of the block, AWS reserving the first four and the last of every subnet CIDR.
        let named_private_address = row.names_private_address.then(|| {
            let mut octets = row.network.1.split('/').next().unwrap_or_default().split('.');
            let head: Vec<&str> = octets.by_ref().take(3).collect();
            format!("{}.{}.{}.5", head[0], head[1], head[2])
        });
        if let Some(private_address) = named_private_address.clone() {
            params.push(p("PrivateIpAddress", private_address));
        }
        if !row.tags.is_empty() {
            params.push(p("TagSpecification.1.ResourceType", NAT_GATEWAY_TAGGABLE_RESOURCE));
            for (position, (key, value)) in row.tags.iter().enumerate() {
                params.push(p(
                    &format!("TagSpecification.1.Tag.{}.Key", position + 1),
                    *key,
                ));
                params.push(p(
                    &format!("TagSpecification.1.Tag.{}.Value", position + 1),
                    *value,
                ));
            }
        }

        // A dry run of exactly this request changes nothing: no gateway is stored.
        let mut dry = params.clone();
        dry.push(p("DryRun", "true"));
        let (status, body) = ec2(&router, "CreateNatGateway", &dry).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (DRY_RUN_STATUS, DRY_RUN_OPERATION),
            "case {case:?}: expected a dry run to answer {DRY_RUN_OPERATION}, got {status} \
             {body}"
        );
        let (status, listed) = ec2(&router, "DescribeNatGateways", &[]).await;
        assert_eq!(
            (status, elements(&listed, "natGatewayId")),
            (200, Vec::new()),
            "case {case:?}: expected a dry run to store no gateway, got {listed}"
        );

        let (status, created) = ec2(&router, "CreateNatGateway", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateNatGateway must answer 200, got {status} {created}"
        );
        let nat_gateway_id = element(&created, "natGatewayId").unwrap_or_default();
        assert!(
            super::wire::is_long_id("nat", &nat_gateway_id),
            "case {case:?}: expected a nat-… id, got {nat_gateway_id:?} in {created}"
        );
        // "The NAT gateway is being created and is not ready to process traffic": the state a
        // create answers with.
        assert_eq!(
            element(&created, "state").as_deref(),
            Some(NAT_GATEWAY_STATE_PENDING),
            "case {case:?}: expected the create to answer the pending state, got {created}"
        );
        assert_eq!(
            element(&created, "subnetId").as_deref(),
            Some(subnet_id.as_str()),
            "case {case:?}: expected the gateway to be in the subnet the request named, got \
             {created}"
        );
        assert_eq!(
            element(&created, "vpcId").as_deref(),
            Some(vpc_id.as_str()),
            "case {case:?}: expected the gateway to report its subnet's VPC, got {created}"
        );
        assert_eq!(
            element(&created, "connectivityType").as_deref(),
            Some(row.connectivity_type.unwrap_or(CONNECTIVITY_TYPE_PUBLIC)),
            "case {case:?}: expected the connectivity the request asked for, got {created}"
        );
        assert_eq!(
            element(&created, "availabilityMode").as_deref(),
            Some(AVAILABILITY_MODE_ZONAL),
            "case {case:?}: expected a zonal gateway by default, got {created}"
        );

        // One address per Elastic IP address the create was made with — and one private-only
        // address for a private gateway — each on the gateway's own interface.
        let (reported_state, reported_allocations) =
            described(&router, case, &nat_gateway_id).await;
        assert_eq!(
            reported_state, NAT_GATEWAY_STATE_PENDING,
            "case {case:?}: expected the read to report the state the create answered"
        );
        let expected_allocations: Vec<String> = allocation_ids
            .iter()
            .map(|(allocation_id, _)| allocation_id.clone())
            .collect();
        assert_eq!(
            reported_allocations, expected_allocations,
            "case {case:?}: expected the gateway to hold the allocations the create named"
        );
        let (_, body) = ec2(
            &router,
            "DescribeNatGateways",
            &[p("NatGatewayId.1", &nat_gateway_id)],
        )
        .await;
        let interfaces = elements(&body, "networkInterfaceId");
        assert!(
            !interfaces.is_empty() && interfaces.iter().all(|held| held == &interfaces[0]),
            "case {case:?}: expected every address on the one interface, got {interfaces:?}"
        );
        let private_ips = elements(&body, "privateIp");
        assert_eq!(
            private_ips.len(),
            row.allocations.max(1),
            "case {case:?}: expected one private address per address of the gateway, got \
             {private_ips:?}"
        );
        if let Some(expected) = named_private_address.as_deref() {
            assert_eq!(
                private_ips.first().map(String::as_str),
                Some(expected),
                "case {case:?}: expected the private address the request named, got \
                 {private_ips:?}"
            );
        }
        for key in row.tags.iter().map(|(key, _)| *key) {
            assert!(
                elements(&body, "key").iter().any(|held| held == key),
                "case {case:?}: expected the gateway to carry the tag {key:?}, got {body}"
            );
        }

        // The association is on the ADDRESS too: `DescribeAddresses` reports the gateway's
        // interface as the holder of every Elastic IP address it was created with.
        for (allocation_id, public_ip) in &allocation_ids {
            let (status, address) = ec2(
                &router,
                "DescribeAddresses",
                &[p("AllocationId.1", allocation_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeAddresses({allocation_id}) must answer 200, got \
                 {status} {address}"
            );
            assert_eq!(
                element(&address, "publicIp").as_deref(),
                Some(public_ip.as_str()),
                "case {case:?}: expected the allocation's own address, got {address}"
            );
            assert_eq!(
                element(&address, "networkInterfaceId").as_deref(),
                interfaces.first().map(String::as_str),
                "case {case:?}: expected the address to be held by the gateway's interface, \
                 got {address}"
            );
        }

        // The clock finishes the create: the gateway reaches `available` on its own.
        tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
        let (reported_state, _) = described(&router, case, &nat_gateway_id).await;
        assert_eq!(
            reported_state, NAT_GATEWAY_STATE_AVAILABLE,
            "case {case:?}: expected the gateway to come up once its instant had passed"
        );

        // Deleting it disassociates the Elastic IP addresses WITHOUT releasing them, so each
        // allocation is still the account's and can now be released.
        let (status, deleted) = ec2(
            &router,
            "DeleteNatGateway",
            &[p("NatGatewayId", &nat_gateway_id)],
        )
        .await;
        assert_eq!(
            (status, element(&deleted, "natGatewayId").as_deref()),
            (200, Some(nat_gateway_id.as_str())),
            "case {case:?}: DeleteNatGateway must answer 200 naming the gateway, got {status} \
             {deleted}"
        );
        let (reported_state, _) = described(&router, case, &nat_gateway_id).await;
        assert_eq!(
            reported_state, NAT_GATEWAY_STATE_DELETING,
            "case {case:?}: expected the delete to leave the gateway deleting"
        );
        for (allocation_id, _) in &allocation_ids {
            let (status, address) = ec2(
                &router,
                "DescribeAddresses",
                &[p("AllocationId.1", allocation_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the allocation {allocation_id} must still be the account's, \
                 got {status} {address}"
            );
            assert_eq!(
                element(&address, "associationId"),
                None,
                "case {case:?}: expected the address to be disassociated, got {address}"
            );
            let (status, released) = ec2(
                &router,
                "ReleaseAddress",
                &[p("AllocationId", allocation_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a disassociated address must be releasable, got {status} \
                 {released}"
            );
        }
        // The clock finishes the deletion too.
        tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
        let (reported_state, _) = described(&router, case, &nat_gateway_id).await;
        assert_eq!(
            reported_state, NAT_GATEWAY_STATE_DELETED,
            "case {case:?}: expected the gateway to reach deleted once its instant had passed"
        );
    }
}

#[tokio::test]
async fn one_client_token_creates_one_nat_gateway() {
    let router = emulator();
    let case = "the same request twice under one client token";
    let (_, subnet_id) = network(&router, case, "10.81.0.0/16", "10.81.1.0/24", "us-east-1a").await;
    let (allocation_id, _) = allocate(&router, case).await;
    let params: Params = vec![
        p("SubnetId", subnet_id),
        p("AllocationId", allocation_id),
        p("ClientToken", "the-one-token"),
    ];
    let (status, first) = ec2(&router, "CreateNatGateway", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first create must answer 200, got {status} {first}"
    );
    let (status, second) = ec2(&router, "CreateNatGateway", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the repeat must answer 200, got {status} {second}"
    );
    assert_eq!(
        element(&second, "natGatewayId"),
        element(&first, "natGatewayId"),
        "case {case:?}: expected the repeat to answer the gateway the first create made"
    );
    assert_eq!(
        element(&second, "clientToken").as_deref(),
        Some("the-one-token"),
        "case {case:?}: expected the client token to be echoed, got {second}"
    );
    let (_, listed) = ec2(&router, "DescribeNatGateways", &[]).await;
    assert_eq!(
        elements(&listed, "natGatewayId").len(),
        1,
        "case {case:?}: expected the account to hold one gateway, got {listed}"
    );

    // "Constraint: Maximum 64 ASCII characters" (`$ClientToken`): a longer token is refused
    // rather than stored, so the idempotency index cannot hold a string of a caller's
    // choosing.
    let case = "a client token past the documented maximum";
    let (_, subnet_id) = network(&router, case, "10.82.0.0/16", "10.82.1.0/24", "us-east-1a").await;
    let (allocation_id, _) = allocate(&router, case).await;
    let (status, refused) = ec2(
        &router,
        "CreateNatGateway",
        &[
            p("SubnetId", subnet_id),
            p("AllocationId", allocation_id),
            p("ClientToken", "t".repeat(65)),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, INVALID_PARAMETER_VALUE),
        "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {status} {refused}"
    );
    let (_, listed) = ec2(&router, "DescribeNatGateways", &[]).await;
    assert_eq!(
        elements(&listed, "natGatewayId").len(),
        1,
        "case {case:?}: expected the refused create to store nothing, got {listed}"
    );
}

/// One refused create: what the request asks for, and the code EC2 answers.
struct RefusalCase {
    case: &'static str,
    /// Whether the request names the subnet created for the row (`false` names an id the
    /// account holds no subnet for).
    real_subnet: bool,
    connectivity_type: Option<&'static str>,
    /// Whether the request names the Elastic IP address allocated for the row.
    with_allocation: bool,
    /// Whether that address is already held by something else.
    address_already_held: bool,
    private_address: Option<&'static str>,
    availability_mode: Option<&'static str>,
    expected_code: &'static str,
    expected_status: u16,
}

const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a subnet the account does not hold",
        real_subnet: false,
        connectivity_type: None,
        with_allocation: true,
        address_already_held: false,
        private_address: None,
        availability_mode: None,
        expected_code: INVALID_SUBNET_ID_NOT_FOUND,
        expected_status: 400,
    },
    RefusalCase {
        case: "a public gateway with no Elastic IP address",
        real_subnet: true,
        connectivity_type: Some(CONNECTIVITY_TYPE_PUBLIC),
        with_allocation: false,
        address_already_held: false,
        private_address: None,
        availability_mode: None,
        expected_code: MISSING_PARAMETER,
        expected_status: 400,
    },
    RefusalCase {
        case: "a private gateway with an Elastic IP address",
        real_subnet: true,
        connectivity_type: Some(CONNECTIVITY_TYPE_PRIVATE),
        with_allocation: true,
        address_already_held: false,
        private_address: None,
        availability_mode: None,
        expected_code: INVALID_PARAMETER_COMBINATION,
        expected_status: 400,
    },
    RefusalCase {
        case: "an Elastic IP address another gateway already holds",
        real_subnet: true,
        connectivity_type: None,
        with_allocation: true,
        address_already_held: true,
        private_address: None,
        availability_mode: None,
        expected_code: RESOURCE_ALREADY_ASSOCIATED,
        expected_status: 400,
    },
    RefusalCase {
        case: "a private address of another subnet's block",
        real_subnet: true,
        connectivity_type: None,
        with_allocation: true,
        address_already_held: false,
        private_address: Some("10.99.9.9"),
        availability_mode: None,
        expected_code: INVALID_PARAMETER_VALUE,
        expected_status: 400,
    },
    RefusalCase {
        // A regional gateway is a different resource — it spans the zones of its VPC — and
        // nobody has written it here, so the honest answer is 501 rather than the zonal
        // gateway the request did not ask for.
        case: "a regional gateway",
        real_subnet: true,
        connectivity_type: None,
        with_allocation: true,
        address_already_held: false,
        private_address: None,
        availability_mode: Some(AVAILABILITY_MODE_REGIONAL),
        expected_code: "NotImplemented",
        expected_status: 501,
    },
];

#[tokio::test]
async fn a_refused_create_stores_nothing() {
    for row in REFUSAL_CASES {
        let case = row.case;
        let router = emulator();
        let (_, subnet_id) =
            network(&router, case, "10.90.0.0/16", "10.90.1.0/24", "us-east-1a").await;
        let (allocation_id, _) = allocate(&router, case).await;
        // An address "already associated under association ID …" is one another gateway of
        // the account holds, which is what the row sets up by creating that gateway first.
        let held_by = if row.address_already_held {
            let (status, body) = ec2(
                &router,
                "CreateNatGateway",
                &[
                    p("SubnetId", subnet_id.clone()),
                    p("AllocationId", allocation_id.clone()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the gateway holding the address must be created, got {status} \
                 {body}"
            );
            elements(&body, "natGatewayId")
        } else {
            Vec::new()
        };

        let mut params: Params = vec![p(
            "SubnetId",
            if row.real_subnet {
                subnet_id.clone()
            } else {
                "subnet-0123456789abcdef0".to_string()
            },
        )];
        if let Some(connectivity_type) = row.connectivity_type {
            params.push(p("ConnectivityType", connectivity_type));
        }
        if row.with_allocation {
            params.push(p("AllocationId", allocation_id.clone()));
        }
        if let Some(private_address) = row.private_address {
            params.push(p("PrivateIpAddress", private_address));
        }
        if let Some(availability_mode) = row.availability_mode {
            params.push(p("AvailabilityMode", availability_mode));
        }
        let (status, refused) = ec2(&router, "CreateNatGateway", &params).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (row.expected_status, row.expected_code),
            "case {case:?}: expected the refusal {}, got {status} {refused}",
            row.expected_code
        );
        assert!(
            !error_message(&refused).is_empty(),
            "case {case:?}: expected the refusal to carry a message, got {refused}"
        );
        // Nothing beyond what the row set up itself is stored.
        let (_, listed) = ec2(&router, "DescribeNatGateways", &[]).await;
        assert_eq!(
            elements(&listed, "natGatewayId"),
            held_by,
            "case {case:?}: expected the refused create to store nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_delete_names_a_gateway_the_account_holds() {
    let router = emulator();
    let case = "a NAT gateway id the account holds nothing for";
    let (status, refused) = ec2(
        &router,
        "DeleteNatGateway",
        &[p("NatGatewayId", "nat-0123456789abcdef0")],
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, NAT_GATEWAY_NOT_FOUND),
        "case {case:?}: expected {NAT_GATEWAY_NOT_FOUND}, got {status} {refused}"
    );

    // A dry run of a delete that WOULD have gone through leaves the gateway as it is.
    let case = "a dry run of a delete that would have succeeded";
    let (_, subnet_id) = network(&router, case, "10.91.0.0/16", "10.91.1.0/24", "us-east-1a").await;
    let (allocation_id, _) = allocate(&router, case).await;
    let (status, created) = ec2(
        &router,
        "CreateNatGateway",
        &[p("SubnetId", subnet_id), p("AllocationId", allocation_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must answer 200, got {status} {created}"
    );
    let nat_gateway_id = element(&created, "natGatewayId").unwrap_or_default();
    let (status, body) = ec2(
        &router,
        "DeleteNatGateway",
        &[p("NatGatewayId", &nat_gateway_id), p("DryRun", "true")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (DRY_RUN_STATUS, DRY_RUN_OPERATION),
        "case {case:?}: expected {DRY_RUN_OPERATION}, got {status} {body}"
    );
    let (reported_state, _) = described(&router, case, &nat_gateway_id).await;
    assert_eq!(
        reported_state, NAT_GATEWAY_STATE_PENDING,
        "case {case:?}: expected the dry run to leave the gateway as it was"
    );
}
