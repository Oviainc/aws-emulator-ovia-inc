// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateTransitGatewayMulticastDomain`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=AssociateTransitGatewayMulticastDomain`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: an association is made only out of resources the account actually
//! holds and only through an attachment that is in service — the domain, the attachment and
//! every subnet named must each resolve in the store, with the subnets reached through the
//! resource the attachment attaches, and the attachment must be `available`; what the
//! request then asks for is PERSISTED as one association per subnet, starting in the
//! transient `associating` state with the clock owing it `associated`, and anything that
//! cannot be resolved is refused with that resource's own code while NOTHING is written.
//!
//! Each table row is a distinct input along what that generalises over: how many subnets one
//! request names, in which order, whether it repeats one, which of the three resources
//! cannot be resolved, whether a named value is an id of its kind at all, which state the
//! attachment is in, and whether the subnet is one the attachment can reach. The multicast
//! domain and the attachment are seeded through the store's own writers, because
//! `CreateTransitGatewayMulticastDomain` and `CreateTransitGatewayVpcAttachment` are still
//! scaffold stubs with issues of their own; the VPCs and subnets are created over the wire.

use std::time::Duration;

use ec2::ops::types::TransitGatewayVpcAttachment;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INCORRECT_STATE,
    INVALID_SUBNET_ID_NOT_FOUND, INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
    INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
    INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND,
    MULTICAST_ASSOCIATION_STATE_ASSOCIATED, MULTICAST_ASSOCIATION_STATE_ASSOCIATING,
    RESOURCE_ALREADY_ASSOCIATED, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE, TRANSITION_DELAY,
    TransitGatewayAttachmentView, TransitGatewayMulticastDomainRecord,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};
use super::wire::{Param, param};

/// The ec2Query `Action` under test.
const ACTION: &str = "AssociateTransitGatewayMulticastDomain";

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// The HTTP status EC2 answers every refusal of this operation with.
const REFUSAL_STATUS: u16 = 400;

/// The code the request boundary refuses an absent `@required` member with, before any
/// handler of this service runs — the service-wide refusal envelope, not a code a handler
/// chooses.
const BOUNDARY_MISSING_MEMBER: &str = "MalformedInput";

/// A well-formed EC2 id suffix — 17 lowercase hex digits — that nothing in this store mints,
/// so an id built from it is well formed and names no resource.
const UNKNOWN_SUFFIX: &str = "00000000000000000";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of [`ACTION`]'s own service, signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One successful call of `action`, as its body.
async fn ok_body(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A well-formed id of the kind `prefix` names that no resource of this store has.
fn unknown_id(prefix: &str) -> String {
    format!("{prefix}-{UNKNOWN_SUFFIX}")
}

/// The VPC `CreateVpc` makes for `cidr_block`.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok_body(
        "the network",
        router,
        "CreateVpc",
        &[param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// The subnet `CreateSubnet` makes in `vpc_id`.
async fn create_subnet(
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
    zone: &str,
) -> String {
    let body = ok_body(
        "the network",
        router,
        "CreateSubnet",
        &[
            param("VpcId", vpc_id),
            param("CidrBlock", cidr_block),
            param("AvailabilityZone", zone),
        ],
    )
    .await;
    element(&body, "subnetId").unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"))
}

/// A VPC attachment of `vpc_id` in `state`, as it is stored.
fn vpc_attachment(vpc_id: &str, state: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        vpc_id: Some(vpc_id.to_string()),
        // The VPC is one this account created over the wire above, so the attachment names
        // this account as the owner of what it attaches.
        vpc_owner_id: Some(engine::emulator_account_id().to_string()),
        state: Some(state.to_string()),
        ..Default::default()
    })
}

/// The network, the domain and the attachments every row is driven against.
struct Fixture {
    /// The multicast domain the association is made in.
    domain_id: String,
    /// An `available` attachment of the VPC the subnets below belong to.
    attachment_id: String,
    /// An attachment of the same VPC that is still waiting for its owner's acceptance.
    pending_attachment_id: String,
    /// The VPC that attachment attaches.
    vpc_id: String,
    /// Three subnets of that VPC, in creation order.
    subnets: Vec<String>,
    /// A subnet of ANOTHER VPC — one the attachment above cannot reach.
    unreachable_subnet: String,
}

/// Seed [`Fixture`]: two VPCs with their subnets over the wire, then the attachments and the
/// multicast domain through the store's own writers.
async fn fixture(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Fixture {
    let vpc_id = create_vpc(router, "10.90.0.0/16").await;
    let mut subnets = Vec::new();
    for (index, zone) in ["us-east-1a", "us-east-1b", "us-east-1c"]
        .into_iter()
        .enumerate()
    {
        subnets.push(
            create_subnet(
                router,
                &vpc_id,
                &format!("10.90.{}.0/24", index + 1),
                zone,
            )
            .await,
        );
    }
    let other_vpc_id = create_vpc(router, "10.91.0.0/16").await;
    let unreachable_subnet =
        create_subnet(router, &other_vpc_id, "10.91.1.0/24", "us-east-1a").await;

    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let attachment_id = guard
        .insert_transit_gateway_attachment(vpc_attachment(
            &vpc_id,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
        ))
        .expect("the store holds the seeded attachment");
    let pending_attachment_id = guard
        .insert_transit_gateway_attachment(vpc_attachment(
            &vpc_id,
            TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
        ))
        .expect("the store holds the seeded attachment");
    let domain_id = guard
        .insert_transit_gateway_multicast_domain(TransitGatewayMulticastDomainRecord {
            // The store mints the id; what a writer puts here is replaced by it.
            transit_gateway_multicast_domain_id: String::new(),
            transit_gateway_id: "tgw-0123456789abcdef0".to_string(),
            owner_id: engine::emulator_account_id().to_string(),
            // The `TransitGatewayMulticastDomainState` enum's `available` member.
            state: "available".to_string(),
            associations: Vec::new(),
        })
        .expect("the store holds the seeded multicast domain");
    drop(guard);

    Fixture {
        domain_id,
        attachment_id,
        pending_attachment_id,
        vpc_id,
        subnets,
        unreachable_subnet,
    }
}

/// The request members of one association call.
fn association_params(domain_id: &str, attachment_id: &str, subnet_ids: &[&str]) -> Vec<Param> {
    let mut params = vec![
        param("TransitGatewayMulticastDomainId", domain_id),
        param("TransitGatewayAttachmentId", attachment_id),
    ];
    params.extend(indexed("SubnetIds", subnet_ids));
    params
}

/// The associations one domain holds right now, as `(subnet id, state)` pairs read straight
/// out of the store.
fn stored_associations(engine: &engine::InMemoryEngine, domain_id: &str) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_multicast_domains
        .get(domain_id)
        .into_iter()
        .flat_map(|domain| domain.associations.iter())
        .map(|association| (association.subnet_id.clone(), association.state.clone()))
        .collect()
}

/// One row of the success table: which of the fixture's subnets the request names, by index,
/// and the subnets the one association it asks for must carry.
struct Row {
    /// What distinguishes this row.
    case: &'static str,
    /// Indexes into [`Fixture::subnets`], in the order the request names them.
    named: &'static [usize],
    /// The indexes the answer must report, in order.
    expected: &'static [usize],
}

/// The rows: one subnet, two in an order that is not the store's, every subnet of the VPC,
/// and one subnet named twice — which asks for ONE association, not two.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one subnet",
            named: &[0],
            expected: &[0],
        },
        Row {
            case: "two subnets, named in an order of the caller's own",
            named: &[2, 0],
            expected: &[2, 0],
        },
        Row {
            case: "every subnet of the VPC the attachment attaches",
            named: &[0, 1, 2],
            expected: &[0, 1, 2],
        },
        Row {
            case: "one subnet named twice",
            named: &[1, 1],
            expected: &[1],
        },
    ]
}

/// The rule's write half, over every row: the call answers one association per subnet named,
/// in the request's order and in the transient state, under the domain, the attachment and
/// the resource that attachment attaches; the store holds exactly those associations; the
/// clock carries each to `associated`; and the SAME call made again is refused, because the
/// association it asks for is already there.
#[tokio::test]
async fn associating_subnets_persists_one_association_per_subnet_named() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let seeded = fixture(&router, &engine).await;
        let named: Vec<&str> = row
            .named
            .iter()
            .map(|index| seeded.subnets[*index].as_str())
            .collect();
        let expected: Vec<String> = row
            .expected
            .iter()
            .map(|index| seeded.subnets[*index].clone())
            .collect();

        let body = ok_body(
            case,
            &router,
            ACTION,
            &association_params(&seeded.domain_id, &seeded.attachment_id, &named),
        )
        .await;

        assert_eq!(
            elements(&body, "subnetId"),
            expected,
            "case {case:?}: expected exactly the subnets the request named, got {body}"
        );
        assert_eq!(
            elements(&body, "state"),
            vec![MULTICAST_ASSOCIATION_STATE_ASSOCIATING.to_string(); expected.len()],
            "case {case:?}: every new association must start in the transient state, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayMulticastDomainId").as_deref(),
            Some(seeded.domain_id.as_str()),
            "case {case:?}: expected the domain's own id, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayAttachmentId").as_deref(),
            Some(seeded.attachment_id.as_str()),
            "case {case:?}: expected the attachment's own id, got {body}"
        );
        assert_eq!(
            element(&body, "resourceId").as_deref(),
            Some(seeded.vpc_id.as_str()),
            "case {case:?}: expected the resource the attachment attaches, got {body}"
        );
        assert_eq!(
            element(&body, "resourceType").as_deref(),
            Some(TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC),
            "case {case:?}: expected the attached resource's type, got {body}"
        );
        assert_eq!(
            element(&body, "resourceOwnerId").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the account that owns the attached resource, got {body}"
        );

        // The store holds one association per subnet named and nothing else.
        let mut held = stored_associations(&engine, &seeded.domain_id);
        held.sort();
        let mut owed: Vec<(String, String)> = expected
            .iter()
            .map(|subnet_id| {
                (
                    subnet_id.clone(),
                    MULTICAST_ASSOCIATION_STATE_ASSOCIATING.to_string(),
                )
            })
            .collect();
        owed.sort();
        assert_eq!(
            held, owed,
            "case {case:?}: the store must hold exactly the associations the call made"
        );

        // A second call naming the same subnets is refused: the association is already there,
        // which is only knowable from what the first call wrote.
        let (status, repeated) = invoke(
            &router,
            ACTION,
            &association_params(&seeded.domain_id, &seeded.attachment_id, &named),
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for an association already held, \
             got {status} with body {repeated}"
        );
        assert_eq!(
            error_code(&repeated),
            RESOURCE_ALREADY_ASSOCIATED,
            "case {case:?}: expected the already-associated code, got {repeated}"
        );
        let message = error_message(&repeated);
        assert!(
            message.contains(&expected[0]) && message.contains(&seeded.domain_id),
            "case {case:?}: the refusal must name the subnet and the domain, got {message:?}"
        );
        assert_eq!(
            stored_associations(&engine, &seeded.domain_id).len(),
            expected.len(),
            "case {case:?}: a refused association must write nothing"
        );

        // The clock owes each new association its terminal state, which falls due whether or
        // not anybody calls again; a later call is what MATERIALISES the move, so the domain
        // is read back through its own describe before the store is inspected.
        tokio::time::sleep(past_the_transition()).await;
        let described = ok_body(
            case,
            &router,
            "DescribeTransitGatewayMulticastDomains",
            &[param("TransitGatewayMulticastDomainId.1", &seeded.domain_id)],
        )
        .await;
        assert_eq!(
            element(&described, "transitGatewayMulticastDomainId").as_deref(),
            Some(seeded.domain_id.as_str()),
            "case {case:?}: the domain the associations were made in must still be there, got \
             {described}"
        );
        let states: Vec<String> = stored_associations(&engine, &seeded.domain_id)
            .into_iter()
            .map(|(_, state)| state)
            .collect();
        assert_eq!(
            states,
            vec![MULTICAST_ASSOCIATION_STATE_ASSOCIATED.to_string(); expected.len()],
            "case {case:?}: every association must reach its terminal state once it is due"
        );
    }
}

/// One row of the refusal table: a request that cannot be served, and the code it is refused
/// with.
struct Refusal {
    /// What distinguishes this row.
    case: &'static str,
    /// The request, built from the seeded fixture.
    params: fn(&Fixture) -> Vec<Param>,
    /// The HTTP status the refusal carries.
    status: u16,
    /// The code the refusal carries.
    code: &'static str,
}

/// The rows: each of the three resources unresolvable in turn, a value that is not an id of
/// its kind for each of the two id members, an attachment that is not in service, a subnet of
/// another VPC, and a request naming no subnet at all.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a well-formed multicast domain id this account holds no domain for",
            params: |seeded| {
                association_params(
                    &unknown_id(ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX),
                    &seeded.attachment_id,
                    &[seeded.subnets[0].as_str()],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND,
        },
        Refusal {
            case: "a value that is not a multicast domain id at all",
            params: |seeded| {
                association_params(
                    "tgw-mcast-domain-not-hex",
                    &seeded.attachment_id,
                    &[seeded.subnets[0].as_str()],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
        },
        Refusal {
            case: "a well-formed attachment id this account holds no attachment for",
            params: |seeded| {
                association_params(
                    &seeded.domain_id,
                    &unknown_id(ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX),
                    &[seeded.subnets[0].as_str()],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        },
        Refusal {
            case: "a value that is not a transit gateway attachment id at all",
            params: |seeded| {
                association_params(
                    &seeded.domain_id,
                    "tgw-attach-not-hex",
                    &[seeded.subnets[0].as_str()],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        },
        Refusal {
            case: "an attachment still waiting for its owner's acceptance",
            params: |seeded| {
                association_params(
                    &seeded.domain_id,
                    &seeded.pending_attachment_id,
                    &[seeded.subnets[0].as_str()],
                )
            },
            status: REFUSAL_STATUS,
            code: INCORRECT_STATE,
        },
        Refusal {
            case: "a well-formed subnet id this account holds no subnet for",
            params: |seeded| {
                association_params(
                    &seeded.domain_id,
                    &seeded.attachment_id,
                    &[&unknown_id("subnet")],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_SUBNET_ID_NOT_FOUND,
        },
        Refusal {
            case: "a value that is not a subnet id at all",
            params: |seeded| {
                association_params(&seeded.domain_id, &seeded.attachment_id, &["not-a-subnet"])
            },
            status: REFUSAL_STATUS,
            code: ec2::state::INVALID_SUBNET_ID_MALFORMED,
        },
        Refusal {
            case: "a subnet of another VPC, which this attachment cannot reach",
            params: |seeded| {
                association_params(
                    &seeded.domain_id,
                    &seeded.attachment_id,
                    &[seeded.unreachable_subnet.as_str()],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_SUBNET_ID_NOT_FOUND,
        },
        Refusal {
            case: "one subnet the attachment reaches and one it does not",
            params: |seeded| {
                association_params(
                    &seeded.domain_id,
                    &seeded.attachment_id,
                    &[
                        seeded.subnets[0].as_str(),
                        seeded.unreachable_subnet.as_str(),
                    ],
                )
            },
            status: REFUSAL_STATUS,
            code: INVALID_SUBNET_ID_NOT_FOUND,
        },
        Refusal {
            // `SubnetIds` is `@required` but `clientOptional`, so where the refusal of a
            // request that names NO subnet comes from is worth pinning rather than assuming:
            // over ec2Query the boundary validator refuses it against the model's own
            // `@required` marking, in the service-wide refusal envelope, so the handler's own
            // empty-list guard is a safety net behind it and never the wire answer.
            case: "a request naming no subnet at all",
            params: |seeded| {
                association_params(&seeded.domain_id, &seeded.attachment_id, &[])
            },
            status: REFUSAL_STATUS,
            code: BOUNDARY_MISSING_MEMBER,
        },
        Refusal {
            // The batch ceiling is the handler's own, the model publishing no length for the
            // list, so one id past it is refused for how many ids it names rather than for
            // any of them.
            case: "one subnet id past the batch ceiling",
            params: |seeded| {
                let ids: Vec<String> = (0..=ec2::state::MAX_RESOURCE_IDS)
                    .map(|index| format!("subnet-{index:017x}"))
                    .collect();
                let named: Vec<&str> = ids.iter().map(String::as_str).collect();
                association_params(&seeded.domain_id, &seeded.attachment_id, &named)
            },
            status: REFUSAL_STATUS,
            code: ec2::state::INVALID_PARAMETER_VALUE,
        },
        Refusal {
            case: "an authorized dry run of a request that would have succeeded",
            params: |seeded| {
                let mut params = association_params(
                    &seeded.domain_id,
                    &seeded.attachment_id,
                    &[seeded.subnets[0].as_str()],
                );
                params.push(param("DryRun", "true"));
                params
            },
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
        Refusal {
            case: "an authorized dry run naming a domain that does not exist",
            params: |seeded| {
                let mut params = association_params(
                    &unknown_id(ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX),
                    &seeded.attachment_id,
                    &[seeded.subnets[0].as_str()],
                );
                params.push(param("DryRun", "true"));
                params
            },
            // The flag asks about PERMISSION, not about the resources the request names, so
            // the answer does not depend on whether they exist.
            status: DRY_RUN_STATUS,
            code: ec2::state::DRY_RUN_OPERATION,
        },
    ]
}

/// The rule's refusal half, over every row: the request is refused with the code of the
/// resource (or the member) at fault, and the store is left exactly as it was.
#[tokio::test]
async fn an_association_that_cannot_be_served_is_refused_and_writes_nothing() {
    for refusal in refusals() {
        let case = refusal.case;
        let (router, engine) = super::ec2_query::emulator();
        let seeded = fixture(&router, &engine).await;

        let (status, body) = invoke(&router, ACTION, &(refusal.params)(&seeded)).await;
        assert_eq!(
            status, refusal.status,
            "case {case:?}: expected HTTP {expected}, got {status} with body {body}",
            expected = refusal.status
        );
        assert_eq!(
            error_code(&body),
            refusal.code,
            "case {case:?}: expected the code {expected:?}, got {body}",
            expected = refusal.code
        );
        assert!(
            stored_associations(&engine, &seeded.domain_id).is_empty(),
            "case {case:?}: a refused association must write nothing, got {held:?}",
            held = stored_associations(&engine, &seeded.domain_id)
        );
    }
}

/// The wording of the refusal for a multicast domain id that resolves to nothing, which is
/// the one the transit gateway family answers for a resource of it that was deleted or never
/// existed — asserted on an id the store never minted, so the sentence is built from the
/// request rather than from any one recording.
#[tokio::test]
async fn an_unknown_multicast_domain_is_refused_in_the_family_wording() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = fixture(&router, &engine).await;
    // Two ids of the same kind, so what the message carries is the id the REQUEST named.
    for domain_id in [
        unknown_id(ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX),
        format!(
            "{prefix}-0123456789abcdef0",
            prefix = ec2::state::TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX
        ),
    ] {
        let case = domain_id.clone();
        let (status, body) = invoke(
            &router,
            ACTION,
            &association_params(
                &domain_id,
                &seeded.attachment_id,
                &[seeded.subnets[0].as_str()],
            ),
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Transit Gateway Multicast Domain {domain_id} was deleted or does not exist."),
            "case {case:?}: expected the family's own wording, got {body}"
        );
    }
}
