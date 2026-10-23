// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the transit gateway policy table association surface:
//! `AssociateTransitGatewayPolicyTable` and the report that answers what it wrote,
//! `GetTransitGatewayPolicyTableAssociations`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: associating an attachment with a policy table STORES an association
//! of that (table, attachment), described by the attachment's own resource — reported in the
//! transient `associating` state with the clock owing it `associated` — and an id naming a
//! resource the account does not hold is refused with THAT resource's own code while
//! nothing is written. Each table row is a distinct input along what the rule generalises
//! over: which kind of attachment (and so which resource type and resource id the
//! association describes), which id is unknown or malformed, how many associations one
//! table holds, and which attribute a filter selects on.

use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, Tag, TransitGatewayClientVpnAttachment, TransitGatewayPeeringAttachment,
    TransitGatewayPolicyTable, TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
    TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE,
    TRANSITION_DELAY, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The transit gateway the seeded policy tables and attachments belong to.
const TRANSIT_GATEWAY_ID: &str = "tgw-0123456789abcdef0";

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

/// The `TransitGatewayPolicyTableId=` / `TransitGatewayAttachmentId=` pair an association
/// request carries.
fn association_params(policy_table_id: &str, attachment_id: &str) -> Vec<(String, String)> {
    vec![
        (
            "TransitGatewayPolicyTableId".to_string(),
            policy_table_id.to_string(),
        ),
        (
            "TransitGatewayAttachmentId".to_string(),
            attachment_id.to_string(),
        ),
    ]
}

/// One `TransitGatewayPolicyTableId=` request member.
fn table_param(policy_table_id: &str) -> Vec<(String, String)> {
    vec![(
        "TransitGatewayPolicyTableId".to_string(),
        policy_table_id.to_string(),
    )]
}

/// A policy table of the seeded transit gateway, as the store holds one.
fn available_policy_table() -> TransitGatewayPolicyTable {
    TransitGatewayPolicyTable {
        transit_gateway_id: Some(TRANSIT_GATEWAY_ID.to_string()),
        state: Some(TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE.to_string()),
        tags: Some(vec![Tag {
            key: Some("Name".to_string()),
            value: Some("policy".to_string()),
        }]),
        ..Default::default()
    }
}

/// A VPC attachment in service, whose resource is a VPC.
fn vpc_attachment(vpc_id: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
        transit_gateway_id: Some(TRANSIT_GATEWAY_ID.to_string()),
        vpc_id: Some(vpc_id.to_string()),
        vpc_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// A transit gateway peering attachment in service, whose resource is the accepter gateway.
fn peering_attachment(accepter_id: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
        accepter_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some(accepter_id.to_string()),
            owner_id: Some("210987654321".to_string()),
            region: Some(REGION.to_string()),
            ..Default::default()
        }),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// A Client VPN attachment in service, whose resource is the Client VPN endpoint.
fn client_vpn_attachment(endpoint_id: &str) -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::ClientVpn(TransitGatewayClientVpnAttachment {
        transit_gateway_id: Some(TRANSIT_GATEWAY_ID.to_string()),
        client_vpn_endpoint_id: Some(endpoint_id.to_string()),
        client_vpn_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// One kind of attachment, with the resource the association it belongs to describes.
struct Kind {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The attachment, as the store holds it.
    view: fn() -> TransitGatewayAttachmentView,
    /// The `ResourceId` an association of this attachment reports: the id of the resource
    /// the attachment attaches.
    resource_id: &'static str,
    /// The `TransitGatewayAttachmentResourceType` it reports.
    resource_type: &'static str,
}

/// Every kind of attachment the store holds. One row per kind, so the association is
/// asserted for each resource type rather than for the one kind an issue cited.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "vpc attachment",
            view: || vpc_attachment("vpc-0123456789abcdef0"),
            resource_id: "vpc-0123456789abcdef0",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
        },
        Kind {
            case: "peering attachment",
            view: || peering_attachment("tgw-00000000000000001"),
            resource_id: "tgw-00000000000000001",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING,
        },
        Kind {
            case: "client vpn attachment",
            view: || client_vpn_attachment("cvpn-endpoint-0123456789abcdef0"),
            resource_id: "cvpn-endpoint-0123456789abcdef0",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
        },
    ]
}

/// Seed one policy table and one attachment, and answer their ids.
fn seed(engine: &engine::InMemoryEngine, view: TransitGatewayAttachmentView) -> (String, String) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let policy_table_id = guard
        .insert_transit_gateway_policy_table(available_policy_table())
        .expect("the store holds the seeded policy table");
    let attachment_id = guard
        .insert_transit_gateway_attachment(view)
        .expect("the store holds the seeded attachment");
    (policy_table_id, attachment_id)
}

/// How many associations the stored policy table holds right now, read straight out of the
/// store.
fn stored_association_count(engine: &engine::InMemoryEngine, policy_table_id: &str) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_policy_tables
        .get(policy_table_id)
        .map_or(0, |table| table.associations.len())
}

/// An association of an attachment of ANY kind is stored, described by the resource that
/// attachment attaches, answered in the transient state of an association — and the report
/// of the table's associations answers with the same record, so the write is observable to
/// a later request.
#[tokio::test]
async fn associating_an_attachment_stores_an_association_the_report_answers_with() {
    for kind in kinds() {
        let case = kind.case;
        let (router, engine) = super::ec2_query::emulator();
        let (policy_table_id, attachment_id) = seed(&engine, (kind.view)());

        // Nothing is associated until a caller associates it.
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayPolicyTableAssociations",
            &table_param(&policy_table_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report of a fresh table must succeed, got {status} with {body}"
        );
        assert!(
            elements(&body, "transitGatewayAttachmentId").is_empty(),
            "case {case:?}: a fresh policy table holds no associations, got {body}"
        );

        let (status, body) = invoke(
            &router,
            "AssociateTransitGatewayPolicyTable",
            &association_params(&policy_table_id, &attachment_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayPolicyTableId").as_deref(),
            Some(policy_table_id.as_str()),
            "case {case:?}: expected the policy table's own id, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayAttachmentId").as_deref(),
            Some(attachment_id.as_str()),
            "case {case:?}: expected the attachment's own id, got {body}"
        );
        // The resource and its type come off the STORED attachment: the request names
        // neither.
        assert_eq!(
            element(&body, "resourceId").as_deref(),
            Some(kind.resource_id),
            "case {case:?}: expected the attached resource's id, got {body}"
        );
        assert_eq!(
            element(&body, "resourceType").as_deref(),
            Some(kind.resource_type),
            "case {case:?}: expected the attachment's resource type, got {body}"
        );
        // The association is answered in the TRANSIENT state: the span is never zero, so a
        // caller acting straight after this call still sees it.
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING),
            "case {case:?}: expected the transient state of an association, got {body}"
        );
        assert_eq!(
            stored_association_count(&engine, &policy_table_id),
            1,
            "case {case:?}: the store must hold the association the response reported"
        );

        // Read it back through the report: same association, same transient state.
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayPolicyTableAssociations",
            &table_param(&policy_table_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {body}"
        );
        assert_eq!(
            elements(&body, "transitGatewayAttachmentId"),
            vec![attachment_id.clone()],
            "case {case:?}: expected exactly the associated attachment, got {body}"
        );
        assert_eq!(
            element(&body, "resourceId").as_deref(),
            Some(kind.resource_id),
            "case {case:?}: the report must describe the attached resource, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING),
            "case {case:?}: the report must agree with the association, got {body}"
        );

        // The move the clock owed falls due on its own, without anybody asking for it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayPolicyTableAssociations",
            &table_param(&policy_table_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the report must succeed, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED),
            "case {case:?}: the clock owed this association its associated state, got {body}"
        );
    }
}

/// What one refusal row names, and the code it must be refused with.
struct Refusal {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The `TransitGatewayPolicyTableId` to send; `None` sends the seeded table's id.
    policy_table_id: Option<&'static str>,
    /// The `TransitGatewayAttachmentId` to send; `None` sends the seeded attachment's id.
    attachment_id: Option<&'static str>,
    /// The EC2 error code the request must be refused with.
    code: &'static str,
}

/// An id naming a resource the account does not hold is refused with THAT resource's own
/// code, a malformed one before an unknown one, and the policy table — the member the
/// operation names first — before the attachment. Nothing is written by any of them.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_that_resources_own_code() {
    // The ids below are well-formed EC2 ids of their kind (`<prefix>-<8 or 17 hex>`) that
    // no seeded resource has, or text that is not an id of that kind at all.
    let rows = [
        Refusal {
            case: "unknown policy table, long id form",
            policy_table_id: Some("tgw-ptb-0123456789abcdef0"),
            attachment_id: None,
            code: ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND,
        },
        Refusal {
            case: "unknown policy table, short id form",
            policy_table_id: Some("tgw-ptb-01234567"),
            attachment_id: None,
            code: ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND,
        },
        Refusal {
            case: "policy table id that is not an id of that kind",
            policy_table_id: Some("tgw-ptb-not-hex"),
            attachment_id: None,
            code: ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_MALFORMED,
        },
        Refusal {
            case: "policy table id of another kind of resource",
            policy_table_id: Some("tgw-rtb-0123456789abcdef0"),
            attachment_id: None,
            code: ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_MALFORMED,
        },
        Refusal {
            case: "unknown attachment",
            policy_table_id: None,
            attachment_id: Some("tgw-attach-0123456789abcdef0"),
            code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        },
        Refusal {
            case: "attachment id that is not an id of that kind",
            policy_table_id: None,
            attachment_id: Some("tgw-attach-not-hex"),
            code: ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        },
        Refusal {
            case: "neither resource exists: the policy table is reported first",
            policy_table_id: Some("tgw-ptb-0123456789abcdef0"),
            attachment_id: Some("tgw-attach-0123456789abcdef0"),
            code: ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND,
        },
    ];

    for row in rows {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let (policy_table_id, attachment_id) = seed(&engine, vpc_attachment("vpc-0123456789abcdef0"));
        let params = association_params(
            row.policy_table_id.unwrap_or(&policy_table_id),
            row.attachment_id.unwrap_or(&attachment_id),
        );

        let (status, body) = invoke(&router, "AssociateTransitGatewayPolicyTable", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {code}, got {body}",
            code = row.code
        );
        assert_eq!(
            stored_association_count(&engine, &policy_table_id),
            0,
            "case {case:?}: a refused association must write nothing"
        );
    }
}

/// The report resolves its policy table id the same way the association does, so an id it
/// cannot resolve is refused rather than read as "no associations".
#[tokio::test]
async fn the_report_refuses_a_policy_table_id_it_cannot_resolve() {
    let rows = [
        (
            "unknown policy table",
            "tgw-ptb-0123456789abcdef0",
            ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND,
        ),
        (
            "policy table id that is not an id of that kind",
            "tgw-ptb-not-hex",
            ec2::state::INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_MALFORMED,
        ),
    ];
    let (router, _engine) = super::ec2_query::emulator();
    for (case, policy_table_id, code) in rows {
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayPolicyTableAssociations",
            &table_param(policy_table_id),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }
}

/// An attachment is associated with at most one policy table: a second association of the
/// same attachment is refused, whether it names the table the attachment already belongs to
/// or another one, and the first association is left exactly as it was.
#[tokio::test]
async fn an_attachment_is_associated_with_at_most_one_policy_table() {
    let (router, engine) = super::ec2_query::emulator();
    let (first_table, attachment_id) = seed(&engine, vpc_attachment("vpc-0123456789abcdef0"));
    let second_table = engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_policy_table(available_policy_table())
        .expect("the store holds the second seeded policy table");

    let (status, body) = invoke(
        &router,
        "AssociateTransitGatewayPolicyTable",
        &association_params(&first_table, &attachment_id),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first association must succeed, got {status} with body {body}"
    );

    for (case, policy_table_id) in [
        ("the same policy table", &first_table),
        ("another policy table", &second_table),
    ] {
        let (status, body) = invoke(
            &router,
            "AssociateTransitGatewayPolicyTable",
            &association_params(policy_table_id, &attachment_id),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second association must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::RESOURCE_ALREADY_ASSOCIATED,
            "case {case:?}: expected Resource.AlreadyAssociated, got {body}"
        );
    }

    assert_eq!(
        stored_association_count(&engine, &first_table),
        1,
        "the first association must be left exactly as it was"
    );
    assert_eq!(
        stored_association_count(&engine, &second_table),
        0,
        "a refused association must write nothing"
    );
}

/// A dry run of a request that WOULD have succeeded performs nothing: no association is
/// stored, and the association that follows is the one that writes it.
#[tokio::test]
async fn a_dry_run_associates_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let (policy_table_id, attachment_id) = seed(&engine, vpc_attachment("vpc-0123456789abcdef0"));

    let mut dry_run = association_params(&policy_table_id, &attachment_id);
    dry_run.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = invoke(&router, "AssociateTransitGatewayPolicyTable", &dry_run).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "dry run: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        stored_association_count(&engine, &policy_table_id),
        0,
        "a dry run must not associate anything"
    );

    let (status, body) = invoke(
        &router,
        "AssociateTransitGatewayPolicyTable",
        &association_params(&policy_table_id, &attachment_id),
    )
    .await;
    assert_eq!(
        status, 200,
        "an association after a dry run: expected HTTP 200, got {status} with body {body}"
    );
}

/// One filter row: what it is called, the `Filters` entries it sends, and how many of the
/// table's associations must come back.
struct FilterCase<'a> {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The `Filters` entries, as `(name, values)` pairs.
    entries: Vec<(&'a str, &'a [&'a str])>,
    /// How many associations the filters select.
    matched: usize,
}

/// The report pages over the associations one table holds in attachment-id order, and each
/// filter selects on one attribute of the association.
#[tokio::test]
async fn the_report_pages_and_filters_the_associations_it_holds() {
    // Seven attachments, so a page of the smallest `MaxResults` the model admits (5) leaves
    // a second, shorter page behind it.
    const SEEDED: usize = 7;
    const PAGE: usize = 5;

    let (router, engine) = super::ec2_query::emulator();
    let (policy_table_id, attachment_ids) = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let policy_table_id = guard
            .insert_transit_gateway_policy_table(available_policy_table())
            .expect("the store holds the seeded policy table");
        let attachment_ids: Vec<String> = (0..SEEDED)
            .map(|index| {
                guard
                    .insert_transit_gateway_attachment(vpc_attachment(&format!(
                        "vpc-0123456789abcdef{index}"
                    )))
                    .expect("the store holds the seeded attachment")
            })
            .collect();
        (policy_table_id, attachment_ids)
    };
    for attachment_id in &attachment_ids {
        let (status, body) = invoke(
            &router,
            "AssociateTransitGatewayPolicyTable",
            &association_params(&policy_table_id, attachment_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding {attachment_id}: expected HTTP 200, got {status} with body {body}"
        );
    }
    let mut expected: Vec<String> = attachment_ids.clone();
    expected.sort();

    // The first page carries `MaxResults` associations and a token naming where the next
    // one starts.
    let mut first_page = table_param(&policy_table_id);
    first_page.push(("MaxResults".to_string(), PAGE.to_string()));
    let (status, body) = invoke(
        &router,
        "GetTransitGatewayPolicyTableAssociations",
        &first_page,
    )
    .await;
    assert_eq!(
        status, 200,
        "first page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "transitGatewayAttachmentId"),
        expected[..PAGE].to_vec(),
        "first page: expected the first {PAGE} associations in attachment-id order, got {body}"
    );
    let next_token = element(&body, "nextToken").expect("a token naming the second page");

    let mut second_page = table_param(&policy_table_id);
    second_page.push(("MaxResults".to_string(), PAGE.to_string()));
    second_page.push(("NextToken".to_string(), next_token));
    let (status, body) = invoke(
        &router,
        "GetTransitGatewayPolicyTableAssociations",
        &second_page,
    )
    .await;
    assert_eq!(
        status, 200,
        "second page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "transitGatewayAttachmentId"),
        expected[PAGE..].to_vec(),
        "second page: expected the remaining associations, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "second page: the last page carries no token, got {body}"
    );

    // One row per filterable attribute of the association, each selecting a known subset.
    let one = expected[2].clone();
    let just_one = [one.as_str()];
    let rows = vec![
        FilterCase {
            case: "transit-gateway-attachment-id selects one",
            entries: vec![("transit-gateway-attachment-id", &just_one[..])],
            matched: 1,
        },
        FilterCase {
            case: "resource-type selects every vpc attachment",
            entries: vec![(
                "resource-type",
                &[TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC][..],
            )],
            matched: SEEDED,
        },
        FilterCase {
            case: "resource-type selects none of another type",
            entries: vec![(
                "resource-type",
                &[TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING][..],
            )],
            matched: 0,
        },
        FilterCase {
            // Both words of the state machine, joined with an OR inside one filter, so the
            // row asserts the attribute is filterable without depending on whether the
            // clock-owed `associated` move has fallen due while the test runs.
            case: "state selects every association whichever state it has reached",
            entries: vec![(
                "state",
                &[
                    TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING,
                    TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
                ][..],
            )],
            matched: SEEDED,
        },
        FilterCase {
            // `disassociated` is a member of the same `TransitGatewayAssociationState`
            // catalog (`smithy_query(query="shape",shape="TransitGatewayAssociationState")`
            // -> associating, associated, disassociating, disassociated) that no association
            // of this table has reached.
            case: "state selects none of a state no association is in",
            entries: vec![("state", &["disassociated"][..])],
            matched: 0,
        },
        FilterCase {
            case: "resource-id matches EC2's wildcard",
            entries: vec![("resource-id", &["vpc-0123456789abcdef*"][..])],
            matched: SEEDED,
        },
        FilterCase {
            case: "two filters are joined with an AND",
            entries: vec![
                ("resource-id", &["vpc-0123456789abcdef0"][..]),
                ("transit-gateway-attachment-id", &just_one[..]),
            ],
            matched: 0,
        },
    ];
    for FilterCase {
        case,
        entries,
        matched,
    } in rows
    {
        let mut params = table_param(&policy_table_id);
        params.extend(filters(&entries));
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayPolicyTableAssociations",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "transitGatewayAttachmentId").len(),
            matched,
            "case {case:?}: expected {matched} associations, got {body}"
        );
    }

    // A filter naming no attribute of an association is refused rather than read as "no
    // results".
    let mut unsupported = table_param(&policy_table_id);
    unsupported.extend(filters(&[("tag:Name", &["policy"])]));
    let (status, body) = invoke(
        &router,
        "GetTransitGatewayPolicyTableAssociations",
        &unsupported,
    )
    .await;
    assert_eq!(
        status, 400,
        "an unsupported filter: expected a refusal, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "an unsupported filter: expected InvalidParameterValue, got {body}"
    );
}
