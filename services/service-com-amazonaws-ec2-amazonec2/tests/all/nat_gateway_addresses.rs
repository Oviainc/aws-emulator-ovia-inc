// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateNatGatewayAddress`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=AssociateNatGatewayAddress`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. the call ASSOCIATES ELASTIC IP ADDRESSES THE ACCOUNT HOLDS WITH ONE SERVING PUBLIC
//!    NAT GATEWAY and persists each association twice over — on the gateway, as a
//!    `NatGatewayAddress` in the transient `associating` status carrying a service-minted
//!    association id, the gateway's network interface and a private address of the subnet
//!    the traffic leaves through, and on the Elastic IP address record itself, so
//!    `DescribeAddresses` and the gateway cannot disagree about who holds the address;
//! 2. the CLOCK finishes the association: an address answered `associating` reaches
//!    `succeeded` one `TRANSITION_DELAY` later, whether or not anybody calls again;
//! 3. a request naming a gateway the account does not hold, a gateway whose state or
//!    connectivity does not admit the association, an allocation the account cannot give
//!    away, a private address the subnet cannot give, an Availability Zone member the
//!    gateway's availability mode does not take, or one address past the published
//!    per-gateway quota is refused with EC2's own error code and stores NOTHING.
//!
//! Each table row is a distinct input along what those rules generalise over — how many
//! allocations in one call, whether the private addresses are named or chosen, a zonal
//! gateway against a regional one, the zone named by name against by id, and every member
//! of the modelled `NatGatewayState` catalog rather than only the states the issue cited —
//! so no row is the specification.
//!
//! The NAT gateway itself is seeded straight into the store, the way `byoip_cidr_lifecycle`
//! seeds its ranges: `CreateNatGateway` is still a scaffold (its own stub issue,
//! `20261023T102725Z-ec2-createnatgateway-stub`), so there is no API path that brings a NAT
//! gateway into the account yet. Everything else each row needs is real: the Elastic IP
//! addresses come out of Amazon's pool through `AllocateAddress`, and the VPC and the
//! subnets are the default ones a fresh account already has.

use std::time::Duration;

use ec2::ops::types::{NAT_GATEWAY_STATE_VALUES, NatGateway, NatGatewayAddress};
use ec2::state::{
    AVAILABILITY_MODE_REGIONAL, AVAILABILITY_MODE_ZONAL, CONNECTIVITY_TYPE_PRIVATE,
    CONNECTIVITY_TYPE_PUBLIC, ComAmazonawsEc2Amazonec2State as Ec2State,
    MAX_EIPS_PER_PUBLIC_NAT_GATEWAY, NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING,
    NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED, NAT_GATEWAY_STATE_AVAILABLE, RouteServerEndpointSpec,
    TRANSITION_DELAY, is_assignable_subnet_address,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed, unescaped};

/// The Region every call here is signed for: the default VPC and its per-zone default
/// subnets a fresh account holds are the ones of that Region, so a seeded NAT gateway sits
/// in a subnet the account really has.
const REGION: &str = "us-east-1";

/// The zone the seeded gateway's own subnet is in.
const ZONE_A: &str = "us-east-1a";

/// Another zone of the same Region, which the default VPC also has a subnet in: the zone a
/// regional gateway's second Elastic IP address is for.
const ZONE_B: &str = "us-east-1b";

/// The zone id of a third zone of the Region, in the `<prefix>-az<n>` form
/// `DescribeAvailabilityZones`'s `zone-id` filter documents.
const ZONE_C_ID: &str = "use1-az3";

/// EC2's error code for a NAT gateway id the account holds no NAT gateway for.
const NAT_GATEWAY_NOT_FOUND: &str = "NatGatewayNotFound";

/// EC2's error code for something that is not a NAT gateway id at all.
const NAT_GATEWAY_MALFORMED: &str = "NatGatewayMalformed";

/// EC2's error code for a resource whose current state does not admit the request.
const INCORRECT_STATE: &str = "IncorrectState";

/// EC2's error code for a value the service rejects.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for members that cannot be given together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// EC2's error code for a parameter the operation needs and the request left out.
const MISSING_PARAMETER: &str = "MissingParameter";

/// The code the request walk answers when a member the model marks required is absent
/// altogether: the framework's own validation refuses the request before the handler reads
/// it, so `allocationId` missing is this rather than the handler's own refusal for a list
/// that is present and empty.
const MALFORMED_INPUT: &str = "MalformedInput";

/// EC2's error code for an allocation id the account holds no address for.
const INVALID_ALLOCATION_ID_NOT_FOUND: &str = "InvalidAllocationID.NotFound";

/// EC2's error code for something that is not an allocation id at all.
const INVALID_ALLOCATION_ID_MALFORMED: &str = "InvalidAllocationID.Malformed";

/// EC2's error code for a resource that is already attached to something.
const RESOURCE_ALREADY_ASSOCIATED: &str = "Resource.AlreadyAssociated";

/// EC2's error code for a private address something already holds.
const INVALID_IP_ADDRESS_IN_USE: &str = "InvalidIPAddress.InUse";

/// EC2's generic resource-count quota code.
const RESOURCE_COUNT_EXCEEDED: &str = "ResourceCountExceeded";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `AssociateNatGatewayAddress` call carrying `params`, as its HTTP status and XML
/// body.
async fn associate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "AssociateNatGatewayAddress", params).await
}

/// Allocate one Elastic IP address out of Amazon's pool through `AllocateAddress`, in
/// `region`, and answer `(allocation id, public address)`.
async fn allocate_address_in(router: &MultiServiceRouter, region: &str) -> (String, String) {
    let (status, body) = call(router, region, "AllocateAddress", &[]).await;
    assert_eq!(
        status, 200,
        "AllocateAddress in {region}: expected HTTP 200, got {status} with body {body}"
    );
    let allocation_id = element(&body, "allocationId").unwrap_or_default();
    let public_ip = element(&body, "publicIp").unwrap_or_default();
    assert!(
        allocation_id.starts_with("eipalloc-") && !public_ip.is_empty(),
        "AllocateAddress in {region}: expected an allocation id and an address, \
         got body {body}"
    );
    (allocation_id, public_ip)
}

/// Allocate one Elastic IP address in the Region this suite's gateways are in.
async fn allocate_address(router: &MultiServiceRouter) -> (String, String) {
    allocate_address_in(router, REGION).await
}

/// What one seeded NAT gateway is written with: the facts of a stored gateway that the
/// handler's rules read.
#[derive(Debug, Clone, Copy)]
struct GatewaySpec {
    /// Which member of the modelled `NatGatewayState` catalog the gateway is in.
    state: &'static str,
    /// Its `ConnectivityType`.
    connectivity: &'static str,
    /// Its `AvailabilityMode`.
    availability_mode: &'static str,
    /// Whether it already holds the one Elastic IP address a public NAT gateway is created
    /// with — its primary address.
    with_primary_eip: bool,
}

impl GatewaySpec {
    /// A serving, public, single-zone NAT gateway holding its primary Elastic IP address:
    /// what a public `CreateNatGateway` leaves behind once the gateway has come up.
    fn serving() -> Self {
        Self {
            state: NAT_GATEWAY_STATE_AVAILABLE,
            connectivity: CONNECTIVITY_TYPE_PUBLIC,
            availability_mode: AVAILABILITY_MODE_ZONAL,
            with_primary_eip: true,
        }
    }

    /// The same gateway, in `state` instead.
    fn in_state(state: &'static str) -> Self {
        Self {
            state,
            ..Self::serving()
        }
    }

    /// A serving public gateway that holds no Elastic IP address yet.
    fn without_primary() -> Self {
        Self {
            with_primary_eip: false,
            ..Self::serving()
        }
    }

    /// A serving gateway that works across the zones of its VPC.
    fn regional() -> Self {
        Self {
            availability_mode: AVAILABILITY_MODE_REGIONAL,
            ..Self::serving()
        }
    }

    /// A serving private NAT gateway, which has no Elastic IP addresses at all.
    fn private() -> Self {
        Self {
            connectivity: CONNECTIVITY_TYPE_PRIVATE,
            with_primary_eip: false,
            ..Self::serving()
        }
    }
}

/// The seeded gateway, and the facts of the network it sits in that the assertions compare
/// against.
#[derive(Debug, Clone)]
struct Seeded {
    /// The `NatGatewayId` the store minted for it.
    nat_gateway_id: String,
    /// The subnet the gateway is in.
    subnet_id: String,
    /// That subnet's IPv4 CIDR block, which a service-chosen private address comes out of.
    cidr_block: String,
    /// The allocation id of its primary Elastic IP address, when it has one.
    primary_allocation_id: Option<String>,
    /// The network interface its addresses are on, when it already has one.
    network_interface_id: Option<String>,
}

/// Write one NAT gateway built to `spec` into the store, in the default VPC's subnet in
/// [`ZONE_A`], and answer what a row needs to assert against it.
async fn seed_gateway(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    spec: GatewaySpec,
) -> Seeded {
    // The primary address comes out of Amazon's pool through `AllocateAddress`, so the
    // gateway holds an Elastic IP address the account really has.
    let primary = if spec.with_primary_eip {
        Some(allocate_address(router).await)
    } else {
        None
    };
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let subnet = guard
        .subnets
        .values()
        .find(|subnet| subnet.availability_zone.as_deref() == Some(ZONE_A))
        .cloned()
        .expect("a fresh account's default VPC has a default subnet in every zone");
    let subnet_id = subnet.subnet_id.clone().unwrap_or_default();
    let cidr_block = subnet.cidr_block.clone().unwrap_or_default();
    let nat_gateway_id = guard.next_id("nat");
    let mut network_interface_id = None;
    let mut addresses: Vec<NatGatewayAddress> = Vec::new();
    if let Some((allocation_id, public_ip)) = primary.clone() {
        let interface_id = guard.next_id("eni");
        let association_id = guard.next_id("eipassoc");
        let private_ip = guard
            .mint_private_ip_in_cidr(&cidr_block)
            .expect("the default subnet has a free private address");
        if let Some(address) = guard.addresses.get_mut(&allocation_id) {
            address.association_id = Some(association_id.clone());
            address.network_interface_id = Some(interface_id.clone());
            address.private_ip_address = Some(private_ip.clone());
        }
        addresses.push(NatGatewayAddress {
            allocation_id: Some(allocation_id),
            association_id: Some(association_id),
            network_interface_id: Some(interface_id.clone()),
            private_ip: Some(private_ip),
            public_ip: Some(public_ip),
            is_primary: Some(true),
            failure_message: None,
            status: Some(NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED.to_string()),
            availability_zone: subnet.availability_zone.clone(),
            availability_zone_id: subnet.availability_zone_id.clone(),
        });
        network_interface_id = Some(interface_id);
    }
    guard.insert_nat_gateway(NatGateway {
        nat_gateway_id: Some(nat_gateway_id.clone()),
        state: Some(spec.state.to_string()),
        connectivity_type: Some(spec.connectivity.to_string()),
        availability_mode: Some(spec.availability_mode.to_string()),
        subnet_id: Some(subnet_id.clone()),
        vpc_id: subnet.vpc_id.clone(),
        nat_gateway_addresses: Some(addresses),
        tags: Some(Vec::new()),
        ..NatGateway::default()
    });
    Seeded {
        nat_gateway_id,
        subnet_id,
        cidr_block,
        primary_allocation_id: primary.map(|(allocation_id, _)| allocation_id),
        network_interface_id,
    }
}

/// The addresses the store holds for the gateway `nat_gateway_id`, read straight out of the
/// store so it reports what a later caller would be told.
fn stored_addresses(
    engine: &engine::InMemoryEngine,
    nat_gateway_id: &str,
) -> Vec<NatGatewayAddress> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .nat_gateways
        .get(nat_gateway_id)
        .and_then(|gateway| gateway.nat_gateway_addresses.clone())
        .unwrap_or_default()
}

/// The IPv4 CIDR block of the default subnet in `zone`, which a service-chosen private
/// address of a gateway serving that zone comes out of.
fn subnet_cidr_in_zone(engine: &engine::InMemoryEngine, zone: &str) -> String {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .subnets
        .values()
        .find(|subnet| subnet.availability_zone.as_deref() == Some(zone))
        .and_then(|subnet| subnet.cidr_block.clone())
        .unwrap_or_else(|| panic!("the default VPC must have a subnet in {zone}"))
}

/// The stored gateway address that names `allocation_id`, or `None`.
fn stored_address_of(
    engine: &engine::InMemoryEngine,
    nat_gateway_id: &str,
    allocation_id: &str,
) -> Option<NatGatewayAddress> {
    stored_addresses(engine, nat_gateway_id)
        .into_iter()
        .find(|address| address.allocation_id.as_deref() == Some(allocation_id))
}

/// The association id the Elastic IP address record itself carries, if any.
fn address_association_id(
    engine: &engine::InMemoryEngine,
    allocation_id: &str,
) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .addresses
        .get(allocation_id)
        .and_then(|address| address.association_id.clone())
}

/// The `associationId` `DescribeAddresses` reports for `allocation_id` — the round trip
/// through an implemented reader.
async fn described_association_id(
    router: &MultiServiceRouter,
    allocation_id: &str,
) -> Option<String> {
    let params = indexed("AllocationId", &[allocation_id]);
    let (status, body) = call(router, REGION, "DescribeAddresses", &params).await;
    assert_eq!(
        status, 200,
        "DescribeAddresses for {allocation_id}: expected HTTP 200, \
         got {status} with body {body}"
    );
    element(&body, "associationId")
}

/// The form parameters naming `allocation_ids`.
fn allocation_params(allocation_ids: &[&str]) -> Params {
    indexed("AllocationId", allocation_ids)
}

/// Wait until a move started before now has certainly fallen due. Derived from the
/// service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// What an association stores
// ---------------------------------------------------------------------------

/// Rule 1: each association is stored on the gateway AND on the Elastic IP address record,
/// with everything the service owns — the association id, the network interface, the
/// private address, the zone, the status — derived rather than taken from the request.
#[tokio::test]
async fn an_association_is_stored_on_the_gateway_and_on_the_address() {
    /// One row: `(case, the gateway, how many EIPs in one call, whether the private
    /// addresses are named, the Availability Zone parameters, the zone the EIPs land in)`.
    type Row = (
        &'static str,
        GatewaySpec,
        usize,
        bool,
        &'static [(&'static str, &'static str)],
        &'static str,
    );

    let rows: &[Row] = &[
        (
            "one EIP, the service choosing the private address",
            GatewaySpec::serving(),
            1,
            false,
            &[],
            ZONE_A,
        ),
        (
            "one EIP with the private address named",
            GatewaySpec::serving(),
            1,
            true,
            &[],
            ZONE_A,
        ),
        (
            "two EIPs at once on a gateway holding none",
            GatewaySpec::without_primary(),
            2,
            false,
            &[],
            ZONE_A,
        ),
        (
            "two EIPs at once with both private addresses named",
            GatewaySpec::without_primary(),
            2,
            true,
            &[],
            ZONE_A,
        ),
        (
            "a regional gateway, the zone named by name",
            GatewaySpec::regional(),
            1,
            false,
            &[("AvailabilityZone", ZONE_B)],
            ZONE_B,
        ),
        (
            "a regional gateway, the zone named by id",
            GatewaySpec::regional(),
            1,
            false,
            &[("AvailabilityZoneId", ZONE_C_ID)],
            "us-east-1c",
        ),
    ];

    for (case, spec, count, name_private_ips, zone_params, expected_zone) in rows {
        let (router, engine) = emulator();
        let seeded = seed_gateway(&router, &engine, *spec).await;
        let mut allocated: Vec<(String, String)> = Vec::new();
        for _ in 0..*count {
            allocated.push(allocate_address(&router).await);
        }
        let allocation_ids: Vec<&str> = allocated
            .iter()
            .map(|(allocation_id, _)| allocation_id.as_str())
            .collect();

        // A named private address is one of the subnet the traffic leaves through: for a
        // regional gateway that is the VPC's subnet in the zone the request named, so the
        // rows that name addresses are the zonal ones.
        let named_private_ips: Vec<String> = if *name_private_ips {
            (0..*count).map(|index| format!("172.31.9.{}", 10 + index)).collect()
        } else {
            Vec::new()
        };

        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(&allocation_ids));
        for (member, value) in *zone_params {
            params.push(p(member, *value));
        }
        if !named_private_ips.is_empty() {
            let borrowed: Vec<&str> = named_private_ips.iter().map(String::as_str).collect();
            params.extend(indexed("PrivateIpAddress", &borrowed));
        }

        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "natGatewayId").as_deref(),
            Some(seeded.nat_gateway_id.as_str()),
            "case {case:?}: expected the gateway's own id in the response, got body {body}"
        );

        // "The IP addresses" the result carries are every address the gateway now has, so
        // the primary it already held is reported beside the ones this call added.
        let mut expected_ids: Vec<String> = seeded.primary_allocation_id.iter().cloned().collect();
        expected_ids.extend(allocation_ids.iter().map(|id| (*id).to_string()));
        let mut reported = elements(&body, "allocationId");
        reported.sort();
        let mut wanted = expected_ids.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the gateway's whole address set, got body {body}"
        );

        let stored = stored_addresses(&engine, &seeded.nat_gateway_id);
        assert_eq!(
            stored.len(),
            expected_ids.len(),
            "case {case:?}: expected {} stored addresses, got {stored:?}",
            expected_ids.len()
        );

        for (index, (allocation_id, public_ip)) in allocated.iter().enumerate() {
            let entry = stored_address_of(&engine, &seeded.nat_gateway_id, allocation_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the gateway must hold {allocation_id}, got {stored:?}")
                });
            // The status an association starts in is the transient one; the clock owes it
            // the terminal one.
            assert_eq!(
                entry.status.as_deref(),
                Some(NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING),
                "case {case:?}: expected {allocation_id} to be associating, got {entry:?}"
            );
            // The association id is the service's to mint, never the caller's.
            let association_id = entry.association_id.clone().unwrap_or_default();
            assert!(
                association_id.starts_with("eipassoc-"),
                "case {case:?}: expected an eipassoc- association id for {allocation_id}, \
                 got {entry:?}"
            );
            assert_eq!(
                entry.public_ip.as_deref(),
                Some(public_ip.as_str()),
                "case {case:?}: expected the address's own public IP for {allocation_id}, \
                 got {entry:?}"
            );
            let private_ip = entry.private_ip.clone().unwrap_or_default();
            if *name_private_ips {
                assert_eq!(
                    Some(private_ip.as_str()),
                    named_private_ips.get(index).map(String::as_str),
                    "case {case:?}: expected the named private address for {allocation_id}, \
                     got {entry:?}"
                );
                // The addresses the rows name are assignable addresses of the gateway's
                // own subnet block, which is what makes them acceptable.
                assert!(
                    is_assignable_subnet_address(&seeded.cidr_block, &private_ip),
                    "case {case:?}: the row's private address must be one of {}, got {private_ip}",
                    seeded.cidr_block
                );
            } else {
                let block = subnet_cidr_in_zone(&engine, expected_zone);
                assert!(
                    is_assignable_subnet_address(&block, &private_ip),
                    "case {case:?}: expected an assignable private address of {block}, \
                     got {entry:?}"
                );
            }
            let interface_id = entry.network_interface_id.clone().unwrap_or_default();
            assert!(
                interface_id.starts_with("eni-"),
                "case {case:?}: expected an eni- network interface for {allocation_id}, \
                 got {entry:?}"
            );
            // A zonal gateway handles its one zone's traffic through the one interface it
            // already has, so an address joining it is on that interface.
            if spec.availability_mode == AVAILABILITY_MODE_ZONAL {
                if let Some(existing) = seeded.network_interface_id.as_deref() {
                    assert_eq!(
                        interface_id, existing,
                        "case {case:?}: expected {allocation_id} on the gateway's own \
                         interface, got {entry:?}"
                    );
                }
            }
            assert_eq!(
                entry.availability_zone.as_deref(),
                Some(*expected_zone),
                "case {case:?}: expected {allocation_id} in {expected_zone}, got {entry:?}"
            );
            // Only a gateway with no primary yet gives one away, and only to the first
            // address of the call.
            let expects_primary = !spec.with_primary_eip && index == 0;
            assert_eq!(
                entry.is_primary,
                Some(expects_primary),
                "case {case:?}: expected isPrimary {expects_primary} for {allocation_id}, \
                 got {entry:?}"
            );

            // The SAME association is on the Elastic IP address record, so an address
            // report cannot disagree with the gateway about who holds it.
            assert_eq!(
                address_association_id(&engine, allocation_id).as_deref(),
                Some(association_id.as_str()),
                "case {case:?}: expected the address record of {allocation_id} to carry \
                 the association"
            );
            assert_eq!(
                described_association_id(&router, allocation_id).await.as_deref(),
                Some(association_id.as_str()),
                "case {case:?}: expected DescribeAddresses to report the association of \
                 {allocation_id}"
            );
        }
    }
}

/// Rule 2: the clock finishes what the association started — an `associating` address
/// reaches `succeeded` once its instant has passed, without anybody touching that gateway
/// again.
#[tokio::test]
async fn an_associating_address_reaches_succeeded_when_it_falls_due() {
    let (router, engine) = emulator();
    let first = seed_gateway(&router, &engine, GatewaySpec::without_primary()).await;
    let second = seed_gateway(&router, &engine, GatewaySpec::without_primary()).await;
    let (allocation_id, _) = allocate_address(&router).await;

    let mut params = vec![p("NatGatewayId", first.nat_gateway_id.clone())];
    params.extend(allocation_params(&[allocation_id.as_str()]));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the association, got {status} with body {body}"
    );
    // The span is never zero, so a caller acting straight after the call still sees the
    // transient status.
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING),
        "expected the association to start as associating, got body {body}"
    );

    past_the_transition().await;

    // Another gateway's call: it touches nothing of the first gateway, so what the first
    // gateway's address reports now is what the clock owed it.
    let (other_allocation_id, _) = allocate_address(&router).await;
    let mut other = vec![p("NatGatewayId", second.nat_gateway_id.clone())];
    other.extend(allocation_params(&[other_allocation_id.as_str()]));
    let (status, body) = associate(&router, &other).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the second gateway's association, \
         got {status} with body {body}"
    );

    let entry = stored_address_of(&engine, &first.nat_gateway_id, &allocation_id)
        .unwrap_or_else(|| panic!("the first gateway must still hold {allocation_id}"));
    assert_eq!(
        entry.status.as_deref(),
        Some(NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED),
        "expected the due association to have succeeded, got {entry:?}"
    );
}

// ---------------------------------------------------------------------------
// What is refused
// ---------------------------------------------------------------------------

/// Rule 3: a NAT gateway id this account holds no gateway for is refused as not found, and
/// something that is not a NAT gateway id at all is refused as malformed BEFORE it is
/// looked up — so a caller can tell the two apart.
#[tokio::test]
async fn a_nat_gateway_id_the_account_cannot_resolve_is_refused() {
    // `(case, the NatGatewayId under test, the expected code)`.
    let rows: &[(&str, &str, &str)] = &[
        (
            "a long-form id of no gateway",
            "nat-0123456789abcdef0",
            NAT_GATEWAY_NOT_FOUND,
        ),
        (
            "a short-form id of no gateway",
            "nat-0a1b2c3d",
            NAT_GATEWAY_NOT_FOUND,
        ),
        ("another resource's id", "vpc-0123456789abcdef0", NAT_GATEWAY_MALFORMED),
        ("the prefix alone", "nat-", NAT_GATEWAY_MALFORMED),
        ("a suffix of the wrong length", "nat-0a1b2c3d4", NAT_GATEWAY_MALFORMED),
        ("a suffix that is not hexadecimal", "nat-0a1b2c3g", NAT_GATEWAY_MALFORMED),
        ("an id in upper case", "nat-0A1B2C3D", NAT_GATEWAY_MALFORMED),
    ];

    let (router, engine) = emulator();
    // One gateway the account DOES hold, so a refusal is about the id and not about an
    // empty store.
    seed_gateway(&router, &engine, GatewaySpec::serving()).await;
    let (allocation_id, _) = allocate_address(&router).await;

    for (case, nat_gateway_id, expected_code) in rows {
        let mut params = vec![p("NatGatewayId", *nat_gateway_id)];
        params.extend(allocation_params(&[allocation_id.as_str()]));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {nat_gateway_id}, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code} for {nat_gateway_id}, \
             got {code:?} with body {body}"
        );
        // The message is built from the request: it names the id the caller asked about.
        let message = element(&body, "Message")
            .map(|text| unescaped(&text))
            .unwrap_or_default();
        assert!(
            message.contains(nat_gateway_id),
            "case {case:?}: expected the message to name {nat_gateway_id}, got {message:?}"
        );
        // Nothing was associated, so the address is still free.
        assert_eq!(
            address_association_id(&engine, &allocation_id),
            None,
            "case {case:?}: a refusal must leave the address unassociated"
        );
    }
}

/// Rule 3: only a gateway that is serving, and only a PUBLIC one, has Elastic IP addresses
/// to associate. The state rows are driven off the modelled `NatGatewayState` catalog, so
/// every state the model publishes is covered rather than only the ones the issue cited.
#[tokio::test]
async fn a_gateway_whose_state_or_connectivity_refuses_the_association() {
    for state in NAT_GATEWAY_STATE_VALUES
        .iter()
        .filter(|state| **state != NAT_GATEWAY_STATE_AVAILABLE)
    {
        let (router, engine) = emulator();
        let seeded = seed_gateway(&router, &engine, GatewaySpec::in_state(state)).await;
        let (allocation_id, _) = allocate_address(&router).await;
        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(&[allocation_id.as_str()]));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {state:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INCORRECT_STATE,
            "case {state:?}: expected {INCORRECT_STATE}, got {code:?} with body {body}"
        );
        assert_eq!(
            stored_addresses(&engine, &seeded.nat_gateway_id).len(),
            1,
            "case {state:?}: a refusal must leave the gateway's addresses alone"
        );
    }

    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::private()).await;
    let (allocation_id, _) = allocate_address(&router).await;
    let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
    params.extend(allocation_params(&[allocation_id.as_str()]));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 400,
        "case \"a private NAT gateway\": expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_PARAMETER_VALUE,
        "case \"a private NAT gateway\": expected {INVALID_PARAMETER_VALUE}, \
         got {code:?} with body {body}"
    );
    assert!(
        stored_addresses(&engine, &seeded.nat_gateway_id).is_empty(),
        "case \"a private NAT gateway\": a refusal must store nothing"
    );
}

/// Rule 3: an allocation the account cannot give to this gateway is refused — one that is
/// not an allocation id, one the account holds no address for, one already associated with
/// something, the same one twice in a call, and one advertised from another Region's
/// network border group.
#[tokio::test]
async fn an_allocation_the_account_cannot_associate_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::without_primary()).await;
    let (free, _) = allocate_address(&router).await;
    // An address of another Region: every Availability Zone of a Region advertises from the
    // one border group named for that Region, so this one is not advertised from the
    // gateway's.
    let (elsewhere, _) = allocate_address_in(&router, "us-west-2").await;
    // An address already associated with something, seeded the way a gateway's primary is.
    let other = seed_gateway(&router, &engine, GatewaySpec::serving()).await;
    let associated = other
        .primary_allocation_id
        .clone()
        .expect("the seeded gateway holds a primary address");

    // `(case, the AllocationIds under test, the expected code)`.
    let rows: &[(&str, Vec<&str>, &str)] = &[
        (
            "an id of no address of the account",
            vec!["eipalloc-0123456789abcdef0"],
            INVALID_ALLOCATION_ID_NOT_FOUND,
        ),
        (
            "another resource's id",
            vec!["eni-0123456789abcdef0"],
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "a suffix of the wrong length",
            vec!["eipalloc-0a1b2c3d4"],
            INVALID_ALLOCATION_ID_MALFORMED,
        ),
        (
            "an address another resource already holds",
            vec![associated.as_str()],
            RESOURCE_ALREADY_ASSOCIATED,
        ),
        (
            "the same address twice in one call",
            vec![free.as_str(), free.as_str()],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an address of another Region's border group",
            vec![elsewhere.as_str()],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a free address beside an unknown one",
            vec![free.as_str(), "eipalloc-0123456789abcdef0"],
            INVALID_ALLOCATION_ID_NOT_FOUND,
        ),
    ];

    for (case, allocation_ids, expected_code) in rows {
        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(allocation_ids));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
        assert!(
            stored_addresses(&engine, &seeded.nat_gateway_id).is_empty(),
            "case {case:?}: a refusal must store nothing on the gateway"
        );
        assert_eq!(
            address_association_id(&engine, &free),
            None,
            "case {case:?}: a refusal must leave the free address unassociated"
        );
    }
}

/// Rule 3: a private address the subnet cannot give is refused — one that is not an IPv4
/// address, a list that does not pair with the allocations, an address outside the subnet's
/// own block, and one something already holds.
#[tokio::test]
async fn a_private_address_the_subnet_cannot_give_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::serving()).await;
    let (free, _) = allocate_address(&router).await;
    // The private address the seeded gateway's own primary already holds.
    let taken = stored_addresses(&engine, &seeded.nat_gateway_id)
        .first()
        .and_then(|address| address.private_ip.clone())
        .expect("the seeded gateway's primary address holds a private address");

    // `(case, the PrivateIpAddress list under test, the expected code)`.
    let rows: &[(&str, Vec<&str>, &str)] = &[
        (
            "two addresses for one allocation",
            vec!["172.31.9.10", "172.31.9.11"],
            INVALID_PARAMETER_VALUE,
        ),
        ("not an IPv4 address", vec!["172.31.9"], INVALID_PARAMETER_VALUE),
        ("an IPv6 address", vec!["2001:db8::1"], INVALID_PARAMETER_VALUE),
        (
            "an address of another block",
            vec!["10.0.0.5"],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an address past the end of the subnet's block",
            vec!["172.31.16.1"],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an address something already holds",
            vec![taken.as_str()],
            INVALID_IP_ADDRESS_IN_USE,
        ),
    ];

    for (case, private_ips, expected_code) in rows {
        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(&[free.as_str()]));
        params.extend(indexed("PrivateIpAddress", private_ips));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
        assert_eq!(
            stored_addresses(&engine, &seeded.nat_gateway_id).len(),
            1,
            "case {case:?}: a refusal must leave the gateway's addresses alone"
        );
    }

    // The block's own boundary addresses: AWS reserves the first four of a subnet CIDR
    // block and the last one, so none of them is an address a NAT gateway can be given.
    for (case, private_ip) in [
        ("the network address", "172.31.0.0"),
        ("the VPC router's address", "172.31.0.1"),
        ("the last reserved address at the bottom", "172.31.0.3"),
        ("the broadcast address", "172.31.15.255"),
    ] {
        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(&[free.as_str()]));
        params.extend(indexed("PrivateIpAddress", &[private_ip]));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {private_ip} to be refused, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE} for {private_ip}, \
             got {code:?} with body {body}"
        );
    }
}

/// Rule 3: the Availability Zone members are a regional gateway's, and name a zone the
/// Region has.
#[tokio::test]
async fn the_availability_zone_members_follow_the_gateways_availability_mode() {
    // `(case, the gateway, the zone parameters, the expected code)`.
    /// One row: `(case, the gateway, the zone parameters, the expected code)`.
    type ZoneRow = (
        &'static str,
        GatewaySpec,
        &'static [(&'static str, &'static str)],
        &'static str,
    );

    let rows: &[ZoneRow] = &[
        (
            "a zone named for a zonal gateway",
            GatewaySpec::serving(),
            &[("AvailabilityZone", ZONE_B)],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a zone id named for a zonal gateway",
            GatewaySpec::serving(),
            &[("AvailabilityZoneId", ZONE_C_ID)],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "no zone named for a regional gateway",
            GatewaySpec::regional(),
            &[],
            MISSING_PARAMETER,
        ),
        (
            "a zone of another Region",
            GatewaySpec::regional(),
            &[("AvailabilityZone", "eu-west-1a")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a zone id of another Region",
            GatewaySpec::regional(),
            &[("AvailabilityZoneId", "euw1-az1")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a name and an id naming different zones",
            GatewaySpec::regional(),
            &[("AvailabilityZone", ZONE_A), ("AvailabilityZoneId", ZONE_C_ID)],
            INVALID_PARAMETER_COMBINATION,
        ),
    ];

    for (case, spec, zone_params, expected_code) in rows {
        let (router, engine) = emulator();
        let seeded = seed_gateway(&router, &engine, *spec).await;
        let (allocation_id, _) = allocate_address(&router).await;
        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(&[allocation_id.as_str()]));
        for (member, value) in *zone_params {
            params.push(p(member, *value));
        }
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
        assert_eq!(
            address_association_id(&engine, &allocation_id),
            None,
            "case {case:?}: a refusal must leave the address unassociated"
        );
    }

    // A name and an id naming the SAME zone are two spellings of one answer, so they hold
    // together.
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::regional()).await;
    let (allocation_id, _) = allocate_address(&router).await;
    let mut params = vec![
        p("NatGatewayId", seeded.nat_gateway_id.clone()),
        p("AvailabilityZone", ZONE_B),
        p("AvailabilityZoneId", "use1-az2"),
    ];
    params.extend(allocation_params(&[allocation_id.as_str()]));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"one zone spelled both ways\": expected HTTP 200, \
         got {status} with body {body}"
    );
    assert_eq!(
        stored_address_of(&engine, &seeded.nat_gateway_id, &allocation_id)
            .and_then(|address| address.availability_zone),
        Some(ZONE_B.to_string()),
        "case \"one zone spelled both ways\": expected the address in {ZONE_B}"
    );
}

/// Rule 3: the published per-gateway quota of Elastic IP addresses is what a public NAT
/// gateway holds — the one past it is refused, whether it is asked for in its own call or
/// together with the others.
#[tokio::test]
async fn the_quota_of_addresses_per_public_nat_gateway_is_enforced() {
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::without_primary()).await;

    // Up to the quota, one address per call.
    for index in 0..MAX_EIPS_PER_PUBLIC_NAT_GATEWAY {
        let (allocation_id, _) = allocate_address(&router).await;
        let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
        params.extend(allocation_params(&[allocation_id.as_str()]));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 200,
            "address {} of {MAX_EIPS_PER_PUBLIC_NAT_GATEWAY}: expected HTTP 200, \
             got {status} with body {body}",
            index + 1
        );
    }
    // One past it.
    let (past, _) = allocate_address(&router).await;
    let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
    params.extend(allocation_params(&[past.as_str()]));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 400,
        "one address past the quota: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, RESOURCE_COUNT_EXCEEDED,
        "one address past the quota: expected {RESOURCE_COUNT_EXCEEDED}, \
         got {code:?} with body {body}"
    );
    assert_eq!(
        stored_addresses(&engine, &seeded.nat_gateway_id).len(),
        MAX_EIPS_PER_PUBLIC_NAT_GATEWAY,
        "one address past the quota: the gateway must still hold exactly the quota"
    );
    assert_eq!(
        address_association_id(&engine, &past),
        None,
        "one address past the quota: the refused address must stay unassociated"
    );

    // The same quota, asked for in ONE call on a fresh gateway.
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::without_primary()).await;
    let mut allocated: Vec<String> = Vec::new();
    for _ in 0..=MAX_EIPS_PER_PUBLIC_NAT_GATEWAY {
        allocated.push(allocate_address(&router).await.0);
    }
    let borrowed: Vec<&str> = allocated.iter().map(String::as_str).collect();
    let mut params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
    params.extend(allocation_params(&borrowed));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 400,
        "the whole quota plus one in one call: expected HTTP 400, \
         got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, RESOURCE_COUNT_EXCEEDED,
        "the whole quota plus one in one call: expected {RESOURCE_COUNT_EXCEEDED}, \
         got {code:?} with body {body}"
    );
    assert!(
        stored_addresses(&engine, &seeded.nat_gateway_id).is_empty(),
        "the whole quota plus one in one call: a refusal must store nothing"
    );

    // A REGIONAL gateway "uses a separate EIP in each AZ", so the quota is the addresses of
    // one zone: each zone takes the whole quota, and the address past it is refused for
    // THAT zone while another zone still takes its own.
    let (router, engine) = emulator();
    let seeded = seed_gateway(
        &router,
        &engine,
        GatewaySpec {
            with_primary_eip: false,
            ..GatewaySpec::regional()
        },
    )
    .await;
    for zone in [ZONE_A, ZONE_B] {
        for index in 0..MAX_EIPS_PER_PUBLIC_NAT_GATEWAY {
            let (allocation_id, _) = allocate_address(&router).await;
            let mut params = vec![
                p("NatGatewayId", seeded.nat_gateway_id.clone()),
                p("AvailabilityZone", zone),
            ];
            params.extend(allocation_params(&[allocation_id.as_str()]));
            let (status, body) = associate(&router, &params).await;
            assert_eq!(
                status, 200,
                "regional, address {index} of the quota in {zone}: expected HTTP 200, \
                 got {status} with body {body}"
            );
        }
        let (past, _) = allocate_address(&router).await;
        let mut params = vec![
            p("NatGatewayId", seeded.nat_gateway_id.clone()),
            p("AvailabilityZone", zone),
        ];
        params.extend(allocation_params(&[past.as_str()]));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "regional, one address past the quota in {zone}: expected HTTP 400, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, RESOURCE_COUNT_EXCEEDED,
            "regional, one address past the quota in {zone}: expected \
             {RESOURCE_COUNT_EXCEEDED}, got {code:?} with body {body}"
        );
    }
    // The gateway as a whole therefore holds one quota per zone it serves.
    assert_eq!(
        stored_addresses(&engine, &seeded.nat_gateway_id)
            .iter()
            .filter(|address| address.allocation_id.is_some())
            .count(),
        2 * MAX_EIPS_PER_PUBLIC_NAT_GATEWAY,
        "regional: each zone must hold its own quota of Elastic IP addresses"
    );
}

/// A `DryRun` request that would have succeeded is answered from the permission check and
/// changes nothing; one that would have been refused is refused for its own reason.
#[tokio::test]
async fn a_dry_run_request_changes_nothing() {
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::serving()).await;
    let (allocation_id, _) = allocate_address(&router).await;

    let mut params = vec![
        p("NatGatewayId", seeded.nat_gateway_id.clone()),
        p("DryRun", "true"),
    ];
    params.extend(allocation_params(&[allocation_id.as_str()]));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, \
         got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, \
         got {code:?} with body {body}"
    );
    assert_eq!(
        stored_addresses(&engine, &seeded.nat_gateway_id).len(),
        1,
        "a dry run must leave the gateway's addresses alone"
    );
    assert_eq!(
        address_association_id(&engine, &allocation_id),
        None,
        "a dry run must leave the address unassociated"
    );

    // A dry run of a request that would have been refused is refused with THAT reason,
    // never with DryRunOperation.
    let mut params = vec![
        p("NatGatewayId", "nat-0123456789abcdef0"),
        p("DryRun", "true"),
    ];
    params.extend(allocation_params(&[allocation_id.as_str()]));
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 400,
        "a dry run that would have been refused: expected HTTP 400, \
         got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, NAT_GATEWAY_NOT_FOUND,
        "a dry run that would have been refused: expected {NAT_GATEWAY_NOT_FOUND}, \
         got {code:?} with body {body}"
    );
}

/// A call naming no allocation at all would associate nothing, so it is refused as a
/// missing parameter rather than answered with an unchanged gateway.
#[tokio::test]
async fn a_call_naming_no_allocation_is_refused() {
    let (router, engine) = emulator();
    let seeded = seed_gateway(&router, &engine, GatewaySpec::serving()).await;
    let params = vec![p("NatGatewayId", seeded.nat_gateway_id.clone())];
    let (status, body) = associate(&router, &params).await;
    assert_eq!(
        status, 400,
        "expected a call naming no allocation to be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        MALFORMED_INPUT,
        "expected {MALFORMED_INPUT} for a call naming no allocation, got body {body}"
    );
    assert_eq!(
        stored_addresses(&engine, &seeded.nat_gateway_id).len(),
        1,
        "a refusal must leave the gateway's addresses alone"
    );
    assert!(
        !seeded.subnet_id.is_empty(),
        "the seeded gateway must name the subnet it is in"
    );
}

/// One subnet never gives the same address to two interfaces: the private address of an
/// association — the one the service chooses AND the one the caller names — is an
/// assignable address of the subnet that NOTHING in the account holds, including the
/// elastic network interface address of a route server endpoint, which lives on the
/// endpoint record rather than in the set of private addresses handed to instances. A
/// caller that names one of those is refused with `InvalidIPAddress.InUse`.
///
/// The rows differ in how long a run of taken addresses the choice has to walk past, so no
/// row is satisfied by skipping exactly one address.
#[tokio::test]
async fn a_subnet_address_a_route_server_endpoint_holds_is_never_chosen_again() {
    for taken in [1usize, 2, 5] {
        let (router, engine) = emulator();
        let seeded = seed_gateway(&router, &engine, GatewaySpec::without_primary()).await;

        // Each endpoint takes the lowest address of the subnet still free, so the taken
        // addresses are exactly the run the association's own choice starts from.
        let mut held: Vec<String> = Vec::new();
        {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            for index in 0..taken {
                let eni_address = guard
                    .mint_route_server_eni_address(&seeded.cidr_block)
                    .unwrap_or_else(|| {
                        panic!("case {taken}: the subnet must have a free address for {index}")
                    });
                guard.insert_route_server_endpoint(RouteServerEndpointSpec {
                    route_server_id: format!("rs-{index:017x}"),
                    subnet_id: seeded.subnet_id.clone(),
                    vpc_id: None,
                    eni_address: Some(eni_address.clone()),
                    tags: Vec::new(),
                });
                held.push(eni_address);
            }
        }

        let (allocation_id, _) = allocate_address(&router).await;
        let mut params = allocation_params(&[allocation_id.as_str()]);
        params.push(p("NatGatewayId", seeded.nat_gateway_id.clone()));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {taken}: expected the association to be stored, got {status} with body {body}"
        );

        let private_ip = stored_address_of(&engine, &seeded.nat_gateway_id, &allocation_id)
            .and_then(|address| address.private_ip)
            .unwrap_or_default();
        assert!(
            is_assignable_subnet_address(&seeded.cidr_block, &private_ip),
            "case {taken}: expected an assignable address of {}, got {private_ip:?}",
            seeded.cidr_block
        );
        assert!(
            !held.contains(&private_ip),
            "case {taken}: expected a private address none of the route server endpoints \
             hold ({held:?}), got {private_ip:?}"
        );
        // And the next endpoint does not get the address the association just took, so the
        // two choosers read the one record of what the subnet has given out.
        let next = {
            let store = engine.state::<Ec2State>();
            let guard = store.lock();
            guard.mint_route_server_eni_address(&seeded.cidr_block)
        };
        assert!(
            next.as_deref() != Some(private_ip.as_str()),
            "case {taken}: expected the next route server endpoint address to differ from \
             the association's {private_ip:?}, got {next:?}"
        );

        // The same rule for an address the CALLER names: one a route server endpoint holds
        // is an address of the subnet already given out, so the request is refused with
        // EC2's own code for that and nothing is stored.
        let (named_allocation_id, _) = allocate_address(&router).await;
        let mut params = allocation_params(&[named_allocation_id.as_str()]);
        params.push(p("NatGatewayId", seeded.nat_gateway_id.clone()));
        params.extend(indexed("PrivateIpAddress", &[held[0].as_str()]));
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {taken}: expected naming the route server address {:?} to be refused, \
             got {status} with body {body}",
            held[0]
        );
        assert_eq!(
            error_code(&body),
            INVALID_IP_ADDRESS_IN_USE,
            "case {taken}: expected {INVALID_IP_ADDRESS_IN_USE} for the held address {:?}, \
             got body {body}",
            held[0]
        );
        assert!(
            stored_address_of(&engine, &seeded.nat_gateway_id, &named_allocation_id).is_none(),
            "case {taken}: a refused call must store no address for {named_allocation_id}"
        );
    }
}
