// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the transit gateway route table propagation family:
//! `EnableTransitGatewayRouteTablePropagation`,
//! `DisableTransitGatewayRouteTablePropagation` and the
//! `GetTransitGatewayRouteTablePropagations` that reads them back, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is the one both writers share: a (propagation route table,
//! attachment) pair either propagates or does not, so an enable needs a pair that does not
//! yet and a disable needs one that does — the other way round each is refused with
//! `IncorrectState` and writes nothing — and the pair moves into the TRANSIENT state of the
//! move asked for, with the clock owing it the end of it (`enabled` after an enable, the
//! removal of the propagation after a disable). Every id a request names is resolved against
//! the store first, so an unknown or malformed one is refused with that resource's own code
//! rather than acted on.
//!
//! Each table row is a distinct input along what that rule generalises over: which kind of
//! attachment propagates (and so which `ResourceType` and `ResourceId` the propagation
//! reports), whether the request names the attachment itself or the route table announcement
//! it propagates through, which of the two writers is called, and which id of the request is
//! the unresolvable one.

use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, TransitGatewayClientVpnAttachment, TransitGatewayPeeringAttachment,
    TransitGatewayRouteTableAnnouncement, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ANNOUNCEMENT_STATE_AVAILABLE, ComAmazonawsEc2Amazonec2State as Ec2State,
    PROPAGATION_STATE_DISABLING, PROPAGATION_STATE_ENABLED, PROPAGATION_STATE_ENABLING,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSITION_DELAY, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// `MissingParameter`, the code EC2 answers for a request that left out a parameter the
/// operation needs (`crate::scaffolded::Scaffolded::missing_parameter`).
const MISSING_PARAMETER: &str = "MissingParameter";

/// `InvalidParameterCombination`, the code EC2 answers for members that cannot hold
/// together (`crate::scaffolded::Scaffolded::invalid_parameter_combination`).
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A transit gateway created through the wire, as the id of the default PROPAGATION route
/// table the create owes it ("we create a default transit gateway route table and use it as
/// … the default propagation route table" — `CreateTransitGateway`).
async fn propagation_route_table(router: &MultiServiceRouter) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "propagationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults has a propagation route table")
}

/// One kind of attachment, with the `ResourceType` and `ResourceId` a propagation of it
/// reports and the stored attachment it is seeded as.
struct Kind {
    /// The kind, as the case name.
    name: &'static str,
    /// The attachment's `TransitGatewayAttachmentResourceType`.
    resource_type: &'static str,
    /// The id of the resource the attachment attaches.
    resource_id: &'static str,
    /// An available attachment of this kind, as it is stored.
    view: fn() -> TransitGatewayAttachmentView,
}

/// A VPC attachment in service.
fn available_vpc_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        vpc_id: Some("vpc-0123456789abcdef0".to_string()),
        vpc_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        subnet_ids: Some(vec!["subnet-0123456789abcdef0".to_string()]),
        ..Default::default()
    })
}

/// A transit gateway peering attachment in service. Its `ResourceId` is the accepter
/// gateway, which is what a report of the attachment names.
fn available_peering_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
        requester_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
            owner_id: Some("210987654321".to_string()),
            region: Some(REGION.to_string()),
            ..Default::default()
        }),
        accepter_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-00000000000000aaa".to_string()),
            owner_id: Some("210987654321".to_string()),
            region: Some("us-west-2".to_string()),
            ..Default::default()
        }),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// A Client VPN attachment in service.
fn available_client_vpn_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::ClientVpn(TransitGatewayClientVpnAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        client_vpn_endpoint_id: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
        client_vpn_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// The attachment kinds a propagation can be of, one row per `ResourceType` this crate
/// stores.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            name: "a VPC attachment",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
            resource_id: "vpc-0123456789abcdef0",
            view: available_vpc_attachment,
        },
        Kind {
            name: "a peering attachment",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING,
            resource_id: "tgw-00000000000000aaa",
            view: available_peering_attachment,
        },
        Kind {
            name: "a Client VPN attachment",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
            resource_id: "cvpn-endpoint-0123456789abcdef0",
            view: available_client_vpn_attachment,
        },
    ]
}

/// How one case names the attachment whose routes propagate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedBy {
    /// `TransitGatewayAttachmentId`: the attachment itself.
    Attachment,
    /// `TransitGatewayRouteTableAnnouncementId`: the route table announcement the
    /// attachment's propagation was asked for through, which names the attachment.
    Announcement,
}

/// Store an available attachment of `kind` and answer its `TransitGatewayAttachmentId`.
fn seed_attachment(engine: &engine::InMemoryEngine, kind: &Kind) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment((kind.view)())
        .expect("the store holds the seeded attachment")
}

/// Store a route table announcement of `route_table_id` made for `attachment_id`, and answer
/// its `TransitGatewayRouteTableAnnouncementId`.
fn seed_announcement(
    engine: &engine::InMemoryEngine,
    route_table_id: &str,
    attachment_id: &str,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_route_table_announcement(TransitGatewayRouteTableAnnouncement {
            transit_gateway_route_table_id: Some(route_table_id.to_string()),
            peering_attachment_id: Some(attachment_id.to_string()),
            state: Some(ANNOUNCEMENT_STATE_AVAILABLE.to_string()),
            ..Default::default()
        })
        .expect("the store holds the seeded announcement")
}

/// The state the store holds for one (route table, attachment) pair, or `None` when it holds
/// no propagation for it at all.
fn stored_state(
    engine: &engine::InMemoryEngine,
    route_table_id: &str,
    attachment_id: &str,
) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_propagations
        .get(&(route_table_id.to_string(), attachment_id.to_string()))
        .map(|record| record.state.clone())
}

/// The states `GetTransitGatewayRouteTablePropagations` reports for one attachment of
/// `route_table_id`, as a list so "reported once" and "not reported" are both assertable.
async fn reported_states(
    router: &MultiServiceRouter,
    route_table_id: &str,
    attachment_id: &str,
) -> Vec<String> {
    let mut params = vec![param("TransitGatewayRouteTableId", route_table_id)];
    params.extend(filters(&[("transit-gateway-attachment-id", &[attachment_id])]));
    let (status, body) =
        invoke(router, "GetTransitGatewayRouteTablePropagations", &params).await;
    assert_eq!(
        status, 200,
        "GetTransitGatewayRouteTablePropagations must succeed, got {status} with body {body}"
    );
    elements(&body, "state")
}

/// Enabling a propagation of ANY attachment kind, named either way, puts the pair into the
/// transient state of an enable with the clock owing it `enabled`; disabling it then puts it
/// into the transient state of a disable with the clock owing its REMOVAL, after which the
/// pair can propagate again. Each call that has nothing to do is refused, which is how the
/// previous one's write is observable to a later request.
#[tokio::test]
async fn enabling_and_disabling_a_propagation_walks_the_pair_through_both_moves() {
    for kind in kinds() {
        for named_by in [NamedBy::Attachment, NamedBy::Announcement] {
            let case = format!("{} named by {named_by:?}", kind.name);
            let (router, engine) = super::ec2_query::emulator();
            let route_table_id = propagation_route_table(&router).await;
            let attachment_id = seed_attachment(&engine, &kind);
            let mut request = vec![param("TransitGatewayRouteTableId", &route_table_id)];
            match named_by {
                NamedBy::Attachment => {
                    request.push(param("TransitGatewayAttachmentId", &attachment_id));
                }
                NamedBy::Announcement => {
                    let announcement_id =
                        seed_announcement(&engine, &route_table_id, &attachment_id);
                    request.push(param(
                        "TransitGatewayRouteTableAnnouncementId",
                        &announcement_id,
                    ));
                }
            }

            // Nothing propagates yet, so there is nothing to disable.
            let (status, body) =
                invoke(&router, "DisableTransitGatewayRouteTablePropagation", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a pair that does not propagate cannot be disabled, got \
                 {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected IncorrectState, got {body}"
            );
            assert_eq!(
                stored_state(&engine, &route_table_id, &attachment_id),
                None,
                "case {case:?}: a refused disable must write nothing"
            );

            // The enable: answered in the TRANSIENT state, carrying the stored attachment's
            // own resource rather than an echo of the request.
            let (status, body) =
                invoke(&router, "EnableTransitGatewayRouteTablePropagation", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the enable, got {status} with body \
                 {body}"
            );
            assert_eq!(
                element(&body, "state").as_deref(),
                Some(PROPAGATION_STATE_ENABLING),
                "case {case:?}: expected the transient state of an enable, got {body}"
            );
            assert_eq!(
                element(&body, "transitGatewayAttachmentId").as_deref(),
                Some(attachment_id.as_str()),
                "case {case:?}: expected the propagating attachment's id, got {body}"
            );
            assert_eq!(
                element(&body, "transitGatewayRouteTableId").as_deref(),
                Some(route_table_id.as_str()),
                "case {case:?}: expected the propagation route table's id, got {body}"
            );
            assert_eq!(
                element(&body, "resourceType").as_deref(),
                Some(kind.resource_type),
                "case {case:?}: expected the stored attachment's resource type, got {body}"
            );
            assert_eq!(
                element(&body, "resourceId").as_deref(),
                Some(kind.resource_id),
                "case {case:?}: expected the stored attachment's resource id, got {body}"
            );
            assert_eq!(
                stored_state(&engine, &route_table_id, &attachment_id).as_deref(),
                Some(PROPAGATION_STATE_ENABLING),
                "case {case:?}: the store must hold what the response reported"
            );
            assert_eq!(
                reported_states(&router, &route_table_id, &attachment_id).await,
                vec![PROPAGATION_STATE_ENABLING.to_string()],
                "case {case:?}: the propagation must be reported once, enabling"
            );

            // A second enable has nothing to enable: the pair already propagates.
            let (status, body) =
                invoke(&router, "EnableTransitGatewayRouteTablePropagation", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a second enable must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected IncorrectState, got {body}"
            );

            // The move the clock owed falls due on its own, without anybody asking for it.
            tokio::time::sleep(past_the_transition()).await;
            assert_eq!(
                reported_states(&router, &route_table_id, &attachment_id).await,
                vec![PROPAGATION_STATE_ENABLED.to_string()],
                "case {case:?}: the clock owed this propagation its enabled state"
            );

            // The disable: the same state machine with the other outcome.
            let (status, body) =
                invoke(&router, "DisableTransitGatewayRouteTablePropagation", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the disable, got {status} with body \
                 {body}"
            );
            assert_eq!(
                element(&body, "state").as_deref(),
                Some(PROPAGATION_STATE_DISABLING),
                "case {case:?}: expected the transient state of a disable, got {body}"
            );
            assert_eq!(
                reported_states(&router, &route_table_id, &attachment_id).await,
                vec![PROPAGATION_STATE_DISABLING.to_string()],
                "case {case:?}: a propagation mid-disable is still reported, disabling"
            );
            let (status, body) =
                invoke(&router, "DisableTransitGatewayRouteTablePropagation", &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a second disable must be refused, got {status} with {body}"
            );

            // Once the removal the disable owed falls due the pair does not propagate at
            // all, so it is reported no longer and can be enabled again.
            tokio::time::sleep(past_the_transition()).await;
            assert_eq!(
                reported_states(&router, &route_table_id, &attachment_id).await,
                Vec::<String>::new(),
                "case {case:?}: a disabled propagation is not one the route table has"
            );
            assert_eq!(
                stored_state(&engine, &route_table_id, &attachment_id),
                None,
                "case {case:?}: the clock owed this propagation its removal"
            );
            let (status, body) =
                invoke(&router, "EnableTransitGatewayRouteTablePropagation", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a pair that no longer propagates can be enabled again, got \
                 {status} with body {body}"
            );
        }
    }
}

/// Every id a propagation request names is resolved against the store, so an id of the wrong
/// form is refused as malformed and an id that resolves to nothing as not found — each with
/// that resource's own code, and never as a 501 or an empty success.
///
/// The rows are one per id of the request and per way it can fail to resolve, asserted for
/// BOTH writers and for the reader, because the resolution is the same for all of them.
#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_refused_with_its_own_code() {
    let (router, engine) = super::ec2_query::emulator();
    let route_table_id = propagation_route_table(&router).await;
    let attachment_id = seed_attachment(&engine, &kinds()[0]);
    let announcement_id = seed_announcement(&engine, &route_table_id, &attachment_id);
    let other_attachment_id = seed_attachment(&engine, &kinds()[1]);

    // (case, the request's fields, the code expected)
    type UnresolvableCase = (&'static str, Vec<(String, String)>, &'static str);
    let cases: Vec<UnresolvableCase> = vec![
        (
            "a route table id of the right form that names no route table",
            vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-0000000000000000f"),
                param("TransitGatewayAttachmentId", &attachment_id),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a short-form route table id that names no route table",
            vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-0000000f"),
                param("TransitGatewayAttachmentId", &attachment_id),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a route table id that is not an id of that kind",
            vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-not-an-id"),
                param("TransitGatewayAttachmentId", &attachment_id),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "an announcement id where a route table id belongs",
            vec![
                param("TransitGatewayRouteTableId", &announcement_id),
                param("TransitGatewayAttachmentId", &attachment_id),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "an attachment id of the right form that names no attachment",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("TransitGatewayAttachmentId", "tgw-attach-0000000000000000f"),
            ],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        ),
        (
            "an attachment id that is not an id of that kind",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("TransitGatewayAttachmentId", "tgw-attach-nope"),
            ],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "an announcement id that names no announcement",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param(
                    "TransitGatewayRouteTableAnnouncementId",
                    "tgw-rtb-announce-0000000000000000f",
                ),
            ],
            ec2::state::INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND,
        ),
        (
            "neither an attachment nor an announcement",
            vec![param("TransitGatewayRouteTableId", &route_table_id)],
            MISSING_PARAMETER,
        ),
        (
            "an attachment that is not the one the announcement was made for",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("TransitGatewayAttachmentId", &other_attachment_id),
                param("TransitGatewayRouteTableAnnouncementId", &announcement_id),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
    ];

    for action in [
        "EnableTransitGatewayRouteTablePropagation",
        "DisableTransitGatewayRouteTablePropagation",
    ] {
        for (name, request, expected) in &cases {
            let case = format!("{action} with {name}");
            let (status, body) = invoke(&router, action, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                &error_code(&body),
                expected,
                "case {case:?}: expected {expected}, got {body}"
            );
        }
    }

    // The reader resolves the route table the same way: a route table the account does not
    // hold is an error, never an empty page.
    for (name, id, expected) in [
        (
            "a route table id that names no route table",
            "tgw-rtb-0000000000000000f",
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a route table id that is not an id of that kind",
            "tgw-rtb-not-an-id",
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
    ] {
        let case = format!("GetTransitGatewayRouteTablePropagations with {name}");
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayRouteTablePropagations",
            &[param("TransitGatewayRouteTableId", id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// A dry run of a request that WOULD have succeeded performs nothing: the pair is in the
/// same state afterwards, and the call that follows is the one that moves it.
#[tokio::test]
async fn a_dry_run_leaves_the_propagation_untouched() {
    let (router, engine) = super::ec2_query::emulator();
    let route_table_id = propagation_route_table(&router).await;
    let attachment_id = seed_attachment(&engine, &kinds()[0]);
    let request = vec![
        param("TransitGatewayRouteTableId", &route_table_id),
        param("TransitGatewayAttachmentId", &attachment_id),
    ];
    let dry_run = {
        let mut dry_run = request.clone();
        dry_run.push(param("DryRun", "true"));
        dry_run
    };

    // (case, the writer dry-run, whether the pair propagates before that row runs)
    for (case, action, propagates_before) in [
        (
            "the enable",
            "EnableTransitGatewayRouteTablePropagation",
            false,
        ),
        (
            "the disable",
            "DisableTransitGatewayRouteTablePropagation",
            true,
        ),
    ] {
        // The state is CAPTURED rather than named: which transient word the pair carries
        // depends on how much of TRANSITION_DELAY has passed, while whether it propagates at
        // all does not.
        let before = stored_state(&engine, &route_table_id, &attachment_id);
        assert_eq!(
            before.is_some(),
            propagates_before,
            "case {case:?}: expected a propagating pair: {propagates_before}, got {before:?}"
        );
        let (status, body) = invoke(&router, action, &dry_run).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, &attachment_id),
            before,
            "case {case:?}: a dry run must not move the propagation"
        );
        let (status, body) = invoke(&router, action, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 after a dry run, got {status} with body {body}"
        );
    }
}

/// The report answers the propagations of the route table it names and no others, narrowed
/// by each documented filter and paged in attachment-id order.
///
/// The rows differ in which documented filter narrows the report, and in the page size,
/// including the smallest the model's `MaxResults` range publishes.
#[tokio::test]
async fn the_report_narrows_by_each_documented_filter_and_pages_in_attachment_id_order() {
    let (router, engine) = super::ec2_query::emulator();
    let route_table_id = propagation_route_table(&router).await;
    let other_route_table_id = propagation_route_table(&router).await;

    // Seven propagations of one route table, cycling through every attachment kind, and one
    // of a SECOND route table that must never be reported under the first.
    let all_kinds = kinds();
    let mut propagating: Vec<String> = Vec::new();
    for index in 0..7 {
        let kind = &all_kinds[index % all_kinds.len()];
        let attachment_id = seed_attachment(&engine, kind);
        let (status, body) = invoke(
            &router,
            "EnableTransitGatewayRouteTablePropagation",
            &[
                param("TransitGatewayRouteTableId", &route_table_id),
                param("TransitGatewayAttachmentId", &attachment_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "propagation {index}: expected HTTP 200, got {status} with body {body}"
        );
        propagating.push(attachment_id);
    }
    let elsewhere = seed_attachment(&engine, &all_kinds[0]);
    let (status, body) = invoke(
        &router,
        "EnableTransitGatewayRouteTablePropagation",
        &[
            param("TransitGatewayRouteTableId", &other_route_table_id),
            param("TransitGatewayAttachmentId", &elsewhere),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the second route table's propagation: got {status} with body {body}"
    );
    propagating.sort();

    // Each documented filter, from `GetTransitGatewayRouteTablePropagationsRequest$Filters`:
    // the attachment, the resource and the resource type. (case, filter, value, how many
    // propagations of this route table hold it)
    let by_kind = |resource_type: &str| {
        (0..7)
            .filter(|index| all_kinds[index % all_kinds.len()].resource_type == resource_type)
            .count()
    };
    let filter_cases: Vec<(&str, &str, String, usize)> = vec![
        (
            "every propagation of the route table",
            "",
            String::new(),
            7,
        ),
        (
            "one attachment",
            "transit-gateway-attachment-id",
            propagating[0].clone(),
            1,
        ),
        (
            "an attachment of another route table",
            "transit-gateway-attachment-id",
            elsewhere.clone(),
            0,
        ),
        (
            "the VPC attachments",
            "resource-type",
            TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC.to_string(),
            by_kind(TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC),
        ),
        (
            "the peering attachments",
            "resource-type",
            TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING.to_string(),
            by_kind(TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING),
        ),
        (
            "one resource",
            "resource-id",
            all_kinds[2].resource_id.to_string(),
            by_kind(all_kinds[2].resource_type),
        ),
        (
            "a resource nothing attaches",
            "resource-id",
            "vpc-00000000000000fff".to_string(),
            0,
        ),
    ];

    for (case, filter, value, expected) in filter_cases {
        let mut params = vec![param("TransitGatewayRouteTableId", &route_table_id)];
        if !filter.is_empty() {
            params.extend(filters(&[(filter, &[value.as_str()])]));
        }
        let (status, body) =
            invoke(&router, "GetTransitGatewayRouteTablePropagations", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = elements(&body, "transitGatewayAttachmentId");
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} propagations, got {reported:?} in {body}"
        );
        assert!(
            !reported.contains(&elsewhere),
            "case {case:?}: the second route table's propagation must never be reported \
             here, got {reported:?}"
        );
    }

    // A filter name the operation does not document is refused rather than read as "no
    // results".
    let mut params = vec![param("TransitGatewayRouteTableId", &route_table_id)];
    params.extend(filters(&[("resource-owner-id", &["210987654321"])]));
    let (status, body) = invoke(&router, "GetTransitGatewayRouteTablePropagations", &params).await;
    assert_eq!(
        status, 400,
        "an undocumented filter must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_PARAMETER_VALUE,
        "expected InvalidParameterValue, got {body}"
    );

    // Paging: a `NextToken` exactly when a propagation follows the page, and following it
    // reports the rest once each, in attachment-id order.
    for page_size in [5usize, 6, 7, 1000] {
        let case = format!("a page size of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("MaxResults", &page_size.to_string()),
            ];
            if let Some(next) = token.as_deref() {
                params.push(param("NextToken", next));
            }
            let (status, body) =
                invoke(&router, "GetTransitGatewayRouteTablePropagations", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "transitGatewayAttachmentId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} propagations, got {}",
                page.len()
            );
            seen.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= propagating.len(),
                "case {case:?}: a token must advance, but {} of {} propagations are already \
                 reported",
                seen.len(),
                propagating.len()
            );
        }
        assert_eq!(
            seen, propagating,
            "case {case:?}: expected every propagation once in attachment-id order, got \
             {seen:?}"
        );
    }

    // A pagination token this service did not mint names no position in the report.
    let (status, body) = invoke(
        &router,
        "GetTransitGatewayRouteTablePropagations",
        &[
            param("TransitGatewayRouteTableId", &route_table_id),
            param("NextToken", "not-a-token"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a bogus pagination token must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_PAGINATION_TOKEN,
        "expected InvalidPaginationToken, got {body}"
    );
}
