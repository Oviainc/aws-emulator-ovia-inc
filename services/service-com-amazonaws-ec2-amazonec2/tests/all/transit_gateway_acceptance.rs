// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the transit gateway attachment family: the four acceptances, the
//! rejection, the multicast domain association acceptance and the route table announcement
//! delete, driven over `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the state machine these operations share: a resource waiting for
//! its owner's decision moves, on that decision, into the TRANSIENT state of the move with
//! the clock owing it the terminal one — and from any other state the same call is refused
//! with `IncorrectState` and writes nothing. Each table row is a distinct input along what
//! that generalises over: which kind of attachment (and so which spelling of the states),
//! which decision, which state the resource is already in, and how many subnets the request
//! names.

use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, Tag, TransitGatewayClientVpnAttachment, TransitGatewayPeeringAttachment,
    TransitGatewayRouteTableAnnouncement, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ANNOUNCEMENT_STATE_AVAILABLE, ANNOUNCEMENT_STATE_DELETED, ANNOUNCEMENT_STATE_DELETING,
    CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE, ComAmazonawsEc2Amazonec2State as Ec2State,
    MULTICAST_ASSOCIATION_STATE_ASSOCIATED, MULTICAST_ASSOCIATION_STATE_ASSOCIATING,
    MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE,
    MulticastDomainAssociationRecord, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED, TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING,
    TRANSITION_DELAY, TransitGatewayAttachmentView, TransitGatewayMulticastDomainRecord,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `TransitGatewayAttachmentId=` request member.
fn attachment_param(attachment_id: &str) -> Vec<(String, String)> {
    vec![(
        "TransitGatewayAttachmentId".to_string(),
        attachment_id.to_string(),
    )]
}

/// The state a stored attachment is in right now, read straight out of the store.
fn stored_state(engine: &engine::InMemoryEngine, attachment_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_attachments
        .get(attachment_id)
        .and_then(|record| record.view.state().map(str::to_string))
        .unwrap_or_default()
}

/// A tag, as the seeded attachments carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// One attachment kind, with the operation that accepts it, the element its result carries
/// it in, and the spelling its own state enum gives `pendingAcceptance`.
struct Kind {
    /// The ec2Query `Action` that accepts an attachment of this kind.
    accept: &'static str,
    /// The element of the result the accepted attachment comes back in.
    result: &'static str,
    /// `pendingAcceptance`, as this kind's state enum spells it.
    pending_acceptance: &'static str,
    /// A pending attachment of this kind, as it is stored.
    view: fn() -> TransitGatewayAttachmentView,
}

/// A VPC attachment waiting for the transit gateway owner.
fn pending_vpc_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        vpc_id: Some("vpc-0123456789abcdef0".to_string()),
        vpc_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string()),
        subnet_ids: Some(vec!["subnet-0123456789abcdef0".to_string()]),
        tags: Some(vec![tag("Name", "shared")]),
        ..Default::default()
    })
}

/// A transit gateway peering attachment waiting for the accepter transit gateway's owner.
fn pending_peering_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
        requester_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
            owner_id: Some("210987654321".to_string()),
            region: Some(REGION.to_string()),
            ..Default::default()
        }),
        accepter_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-9876543210fedcba0".to_string()),
            owner_id: Some(engine::emulator_account_id().to_string()),
            region: Some("us-west-2".to_string()),
            ..Default::default()
        }),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string()),
        tags: Some(vec![tag("Name", "peer")]),
        ..Default::default()
    })
}

/// A Client VPN attachment waiting for the transit gateway owner. Its state enum is the
/// HYPHENATED one, so `pending-acceptance` is the state it waits in.
fn pending_client_vpn_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::ClientVpn(TransitGatewayClientVpnAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        client_vpn_endpoint_id: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
        client_vpn_owner_id: Some("210987654321".to_string()),
        state: Some(CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE.to_string()),
        creation_time: Some("2023-11-14T22:13:25Z".to_string()),
        ..Default::default()
    })
}

/// Every kind of attachment this surface accepts. One row per kind, so the acceptance is
/// asserted for each spelling of the states rather than for the one kind an issue cited.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            accept: "AcceptTransitGatewayVpcAttachment",
            result: "transitGatewayVpcAttachment",
            pending_acceptance: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            view: pending_vpc_attachment,
        },
        Kind {
            accept: "AcceptTransitGatewayPeeringAttachment",
            result: "transitGatewayPeeringAttachment",
            pending_acceptance: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            view: pending_peering_attachment,
        },
        Kind {
            accept: "AcceptTransitGatewayClientVpnAttachment",
            result: "transitGatewayClientVpnAttachment",
            pending_acceptance: CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            view: pending_client_vpn_attachment,
        },
    ]
}

/// Accepting an attachment of ANY kind takes it out of the one state it waits in, into the
/// transient state of the acceptance, and the clock then owes it `available`. A second
/// acceptance is refused, which is how the first one's write is observable to a later
/// request.
#[tokio::test]
async fn accepting_an_attachment_of_each_kind_moves_it_towards_available() {
    for kind in kinds() {
        let case = kind.accept;
        let (router, engine) = super::ec2_query::emulator();
        let attachment_id = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment((kind.view)())
            .expect("the store holds the seeded attachment");
        assert_eq!(
            stored_state(&engine, &attachment_id),
            kind.pending_acceptance,
            "case {case:?}: the seeded attachment must start in its pendingAcceptance state"
        );

        let (status, body) = invoke(&router, case, &attachment_param(&attachment_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            body.contains(&format!("<{result}", result = kind.result)),
            "case {case:?}: expected the accepted attachment in a <{result}> element, got {body}",
            result = kind.result
        );
        assert_eq!(
            element(&body, "transitGatewayAttachmentId").as_deref(),
            Some(attachment_id.as_str()),
            "case {case:?}: expected the attachment's own id, got {body}"
        );
        // The attachment is answered in the TRANSIENT state of the acceptance: the span is
        // never zero, so a caller acting straight after this call still sees it.
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING),
            "case {case:?}: expected the transient state of an acceptance, got {body}"
        );
        assert_eq!(
            stored_state(&engine, &attachment_id),
            TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
            "case {case:?}: the store must hold what the response reported"
        );

        // A second acceptance has nothing to accept: the attachment is no longer waiting.
        let (status, body) = invoke(&router, case, &attachment_param(&attachment_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second acceptance must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INCORRECT_STATE,
            "case {case:?}: expected IncorrectState, got {body}"
        );

        // The move the clock owed falls due on its own, without anybody asking for it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(&router, case, &attachment_param(&attachment_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an available attachment cannot be accepted, got {status} {body}"
        );
        assert_eq!(
            stored_state(&engine, &attachment_id),
            TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            "case {case:?}: the clock owed this attachment its available state"
        );
    }
}

/// Rejecting is the same state machine with the other outcome: the attachment leaves
/// `pendingAcceptance` for the transient state of a rejection, the clock owes it `rejected`,
/// and the acceptance that was available a moment earlier is now refused.
#[tokio::test]
async fn rejecting_a_vpc_attachment_moves_it_towards_rejected_and_rules_out_accepting_it() {
    let (router, engine) = super::ec2_query::emulator();
    let attachment_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(pending_vpc_attachment())
        .expect("the store holds the seeded attachment");

    let (status, body) = invoke(
        &router,
        "RejectTransitGatewayVpcAttachment",
        &attachment_param(&attachment_id),
    )
    .await;
    assert_eq!(status, 200, "reject: got {status} with body {body}");
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING),
        "reject: expected the transient state of a rejection, got {body}"
    );
    // The members the attachment was stored with come back with it, so the response is the
    // stored record rather than an echo of the request.
    assert_eq!(
        element(&body, "vpcId").as_deref(),
        Some("vpc-0123456789abcdef0"),
        "reject: expected the stored VPC id, got {body}"
    );
    assert!(
        body.contains("subnet-0123456789abcdef0"),
        "reject: expected the stored subnet ids, got {body}"
    );

    let (status, body) = invoke(
        &router,
        "AcceptTransitGatewayVpcAttachment",
        &attachment_param(&attachment_id),
    )
    .await;
    assert_eq!(
        status, 400,
        "accept after reject: expected a refusal, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INCORRECT_STATE,
        "accept after reject: expected IncorrectState, got {body}"
    );

    // The move the clock owed falls due on its own; the next call to reach the store is
    // what materialises it, so the state is read after one.
    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = invoke(
        &router,
        "RejectTransitGatewayVpcAttachment",
        &attachment_param(&attachment_id),
    )
    .await;
    assert_eq!(
        status, 400,
        "a rejected attachment cannot be rejected again, got {status} with {body}"
    );
    assert_eq!(
        stored_state(&engine, &attachment_id),
        TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
        "the clock owed this attachment its rejected state"
    );
}

/// Every kind shares one `TransitGatewayAttachmentId` space, so an id naming an attachment
/// of ANOTHER kind names no attachment the operation called can act on — and is refused as
/// unknown rather than acted on.
#[tokio::test]
async fn an_attachment_of_another_kind_is_not_found_for_this_operation() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded: Vec<(String, String)> = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        kinds()
            .into_iter()
            .map(|kind| {
                (
                    kind.accept.to_string(),
                    guard
                        .insert_transit_gateway_attachment((kind.view)())
                        .expect("the store holds the seeded attachment"),
                )
            })
            .collect()
    };

    // Every (operation, attachment) pair: the matching one accepts, the others refuse.
    for (action, _) in &seeded {
        for (owner, attachment_id) in &seeded {
            if owner == action {
                continue;
            }
            let case = format!("{action} on the attachment of {owner}");
            let (status, body) = invoke(&router, action, &attachment_param(attachment_id)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
                "case {case:?}: expected the attachment's NotFound code, got {body}"
            );
        }
    }
}

/// A dry run of a request that WOULD have succeeded performs nothing: the attachment is
/// still waiting afterwards, and the acceptance that follows is the one that moves it.
#[tokio::test]
async fn a_dry_run_leaves_an_acceptable_attachment_untouched() {
    let (router, engine) = super::ec2_query::emulator();
    let attachment_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(pending_vpc_attachment())
        .expect("the store holds the seeded attachment");

    let mut dry_run = attachment_param(&attachment_id);
    dry_run.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = invoke(&router, "AcceptTransitGatewayVpcAttachment", &dry_run).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "dry run: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        stored_state(&engine, &attachment_id),
        TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
        "a dry run must not move the attachment"
    );

    let (status, body) = invoke(
        &router,
        "AcceptTransitGatewayVpcAttachment",
        &attachment_param(&attachment_id),
    )
    .await;
    assert_eq!(
        status, 200,
        "accept after a dry run: expected HTTP 200, got {status} with body {body}"
    );
}

/// What one seeded multicast domain association is written with.
struct AssociationFixture {
    /// Which seeded attachment the association belongs to.
    attachment_index: usize,
    subnet_id: &'static str,
    state: &'static str,
}

/// The associations the multicast table is written against: two subnets of one attachment
/// waiting for the domain owner, one already associated, and one of a second attachment.
fn association_fixtures() -> Vec<AssociationFixture> {
    vec![
        AssociationFixture {
            attachment_index: 0,
            subnet_id: "subnet-0000000000000000a",
            state: MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE,
        },
        AssociationFixture {
            attachment_index: 0,
            subnet_id: "subnet-0000000000000000b",
            state: MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE,
        },
        AssociationFixture {
            attachment_index: 0,
            subnet_id: "subnet-0000000000000000c",
            state: MULTICAST_ASSOCIATION_STATE_ASSOCIATED,
        },
        AssociationFixture {
            attachment_index: 1,
            subnet_id: "subnet-0000000000000000d",
            state: MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE,
        },
    ]
}

/// A multicast domain holding [`association_fixtures`], and the two attachment ids its
/// associations belong to.
fn seed_multicast_domain(engine: &engine::InMemoryEngine) -> (String, Vec<String>) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let attachments: Vec<String> = (0..2)
        .map(|_| {
            guard
                .insert_transit_gateway_attachment(pending_vpc_attachment())
                .expect("the store holds the seeded attachment")
        })
        .collect();
    let associations = association_fixtures()
        .into_iter()
        .map(|fixture| MulticastDomainAssociationRecord {
            transit_gateway_attachment_id: attachments[fixture.attachment_index].clone(),
            resource_id: "vpc-0123456789abcdef0".to_string(),
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC.to_string(),
            resource_owner_id: "210987654321".to_string(),
            subnet_id: fixture.subnet_id.to_string(),
            state: fixture.state.to_string(),
            pending: None,
        })
        .collect();
    let domain_id = guard
        .insert_transit_gateway_multicast_domain(TransitGatewayMulticastDomainRecord {
            transit_gateway_multicast_domain_id: String::new(),
            transit_gateway_id: "tgw-0123456789abcdef0".to_string(),
            owner_id: engine::emulator_account_id().to_string(),
            // The `TransitGatewayMulticastDomainState` enum's `available` member.
            state: "available".to_string(),
            associations,
        })
        .expect("the store holds the seeded multicast domain");
    (domain_id, attachments)
}

/// The state of one seeded association right now, read straight out of the store.
fn association_state(
    engine: &engine::InMemoryEngine,
    domain_id: &str,
    subnet_id: &str,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_multicast_domains
        .get(domain_id)
        .into_iter()
        .flat_map(|domain| domain.associations.iter())
        .find(|association| association.subnet_id == subnet_id)
        .map(|association| association.state.clone())
        .unwrap_or_default()
}

/// The `SubnetIds` and the two ids of one acceptance request.
fn accept_associations_params(
    domain_id: &str,
    attachment_id: &str,
    subnet_ids: &[&str],
) -> Vec<(String, String)> {
    let mut params = vec![
        (
            "TransitGatewayMulticastDomainId".to_string(),
            domain_id.to_string(),
        ),
        (
            "TransitGatewayAttachmentId".to_string(),
            attachment_id.to_string(),
        ),
    ];
    params.extend(indexed("SubnetIds", subnet_ids));
    params
}

/// An acceptance moves exactly the pending associations it selects: the subnets the request
/// names, or every pending subnet of the attachment when it names none — and never an
/// association of another attachment or one that is already associated.
#[tokio::test]
async fn accepting_multicast_associations_moves_only_the_pending_subnets_it_selects() {
    let action = "AcceptTransitGatewayMulticastDomainAssociations";

    // Row 1: one named subnet. Row 2: no subnet named, so every pending one of the
    // attachment. Each row runs against its own freshly seeded store.
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["subnet-0000000000000000a"],
            vec!["subnet-0000000000000000a"],
        ),
        (
            Vec::new(),
            vec!["subnet-0000000000000000a", "subnet-0000000000000000b"],
        ),
    ];
    for (named, expected) in cases {
        let case = format!("SubnetIds={named:?}");
        let (router, engine) = super::ec2_query::emulator();
        let (domain_id, attachments) = seed_multicast_domain(&engine);

        let (status, body) = invoke(
            &router,
            action,
            &accept_associations_params(&domain_id, &attachments[0], &named),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "subnetId"),
            expected
                .iter()
                .map(|subnet| (*subnet).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly the accepted subnets, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayMulticastDomainId").as_deref(),
            Some(domain_id.as_str()),
            "case {case:?}: expected the domain's own id, got {body}"
        );
        assert_eq!(
            element(&body, "resourceType").as_deref(),
            Some(TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC),
            "case {case:?}: expected the attached resource's type, got {body}"
        );

        // Each accepted association is in the transient state now; nothing else moved.
        for subnet in &expected {
            assert_eq!(
                association_state(&engine, &domain_id, subnet),
                MULTICAST_ASSOCIATION_STATE_ASSOCIATING,
                "case {case:?}: {subnet} must be in the transient state of an association"
            );
        }
        assert_eq!(
            association_state(&engine, &domain_id, "subnet-0000000000000000c"),
            MULTICAST_ASSOCIATION_STATE_ASSOCIATED,
            "case {case:?}: an already associated subnet must not move"
        );
        assert_eq!(
            association_state(&engine, &domain_id, "subnet-0000000000000000d"),
            MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE,
            "case {case:?}: an association of another attachment must not move"
        );

        // The clock owes each accepted association its terminal state.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(
            &router,
            action,
            &accept_associations_params(&domain_id, &attachments[0], &named),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: accepting an association that is no longer waiting must be \
             refused, got {status} with body {body}"
        );
        for subnet in &expected {
            assert_eq!(
                association_state(&engine, &domain_id, subnet),
                MULTICAST_ASSOCIATION_STATE_ASSOCIATED,
                "case {case:?}: the clock owed {subnet} its associated state"
            );
        }
    }
}

/// A subnet the attachment has no association for, and an attachment the domain knows
/// nothing about, are each refused rather than silently skipped.
#[tokio::test]
async fn an_unassociated_subnet_or_attachment_is_refused() {
    let action = "AcceptTransitGatewayMulticastDomainAssociations";
    let (router, engine) = super::ec2_query::emulator();
    let (domain_id, attachments) = seed_multicast_domain(&engine);
    let stranger = engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment(pending_vpc_attachment())
        .expect("the store holds the seeded attachment");

    // A subnet of another attachment's association, and one nothing associated at all.
    for subnet in ["subnet-0000000000000000d", "subnet-0000000000000000e"] {
        let (status, body) = invoke(
            &router,
            action,
            &accept_associations_params(&domain_id, &attachments[0], &[subnet]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {subnet:?}: expected a refusal, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {subnet:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // An attachment with no association in this domain.
    let (status, body) = invoke(
        &router,
        action,
        &accept_associations_params(&domain_id, &stranger, &[]),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unassociated attachment: expected a refusal, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        "an unassociated attachment: expected the attachment's NotFound code, got {body}"
    );

    // Accepting every pending association of the attachment twice: the second call has
    // nothing left waiting.
    let (status, _) = invoke(
        &router,
        action,
        &accept_associations_params(&domain_id, &attachments[0], &[]),
    )
    .await;
    assert_eq!(status, 200, "the first acceptance must succeed");
    let (status, body) = invoke(
        &router,
        action,
        &accept_associations_params(&domain_id, &attachments[0], &[]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a second acceptance: expected a refusal, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INCORRECT_STATE,
        "a second acceptance: expected IncorrectState, got {body}"
    );
}

/// Deleting a route table announcement is the same state machine again: it moves to
/// `deleting` with `deleted` owed, the record is KEPT so the outcome stays readable, and a
/// second delete is refused rather than reported as a deletion it did not perform.
#[tokio::test]
async fn deleting_a_route_table_announcement_moves_it_towards_deleted() {
    let (router, engine) = super::ec2_query::emulator();
    let announcement_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_route_table_announcement(TransitGatewayRouteTableAnnouncement {
            transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
            peer_transit_gateway_id: Some("tgw-9876543210fedcba0".to_string()),
            peering_attachment_id: Some("tgw-attach-0123456789abcdef0".to_string()),
            transit_gateway_route_table_id: Some("tgw-rtb-0123456789abcdef0".to_string()),
            announcement_direction: Some("outgoing".to_string()),
            state: Some(ANNOUNCEMENT_STATE_AVAILABLE.to_string()),
            tags: Some(vec![tag("Name", "announcement")]),
            ..Default::default()
        })
        .expect("the store holds the seeded announcement");
    let params = vec![(
        "TransitGatewayRouteTableAnnouncementId".to_string(),
        announcement_id.clone(),
    )];

    let (status, body) = invoke(
        &router,
        "DeleteTransitGatewayRouteTableAnnouncement",
        &params,
    )
    .await;
    assert_eq!(status, 200, "delete: got {status} with body {body}");
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(ANNOUNCEMENT_STATE_DELETING),
        "delete: expected the transient state of a delete, got {body}"
    );
    assert_eq!(
        element(&body, "transitGatewayRouteTableId").as_deref(),
        Some("tgw-rtb-0123456789abcdef0"),
        "delete: expected the stored route table id, got {body}"
    );

    let (status, body) = invoke(
        &router,
        "DeleteTransitGatewayRouteTableAnnouncement",
        &params,
    )
    .await;
    assert_eq!(
        status, 400,
        "a second delete: expected a refusal, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INCORRECT_STATE,
        "a second delete: expected IncorrectState, got {body}"
    );

    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = invoke(
        &router,
        "DeleteTransitGatewayRouteTableAnnouncement",
        &params,
    )
    .await;
    assert_eq!(
        status, 400,
        "a deleted announcement cannot be deleted again, got {status} with body {body}"
    );
    let state = engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_table_announcements
        .get(&announcement_id)
        .and_then(|record| record.view.state.clone())
        .unwrap_or_default();
    assert_eq!(
        state, ANNOUNCEMENT_STATE_DELETED,
        "the clock owed this announcement its deleted state"
    );
}
