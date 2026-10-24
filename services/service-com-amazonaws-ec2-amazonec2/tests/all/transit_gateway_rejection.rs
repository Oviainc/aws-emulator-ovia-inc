// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for REJECTING a transit gateway attachment, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: an attachment waiting for the owner of the attached resource is the
//! only attachment that owner's rejection can act on — it leaves that waiting state for the
//! transient state of a rejection, as THIS kind of attachment's own state enum spells it,
//! with the clock owing it the terminal `rejected`; from any other state the same call is
//! refused with `IncorrectState` and writes nothing, and the response is the STORED
//! attachment rather than an echo of the request.
//!
//! Each row of [`rejectable_kinds`] is a distinct input along what the rule generalises
//! over: which kind of attachment the request names (and so which operation rejects it,
//! which element it comes back in, and which spelling the states take — the camel-cased
//! `TransitGatewayAttachmentState` for a VPC attachment, the hyphenated
//! `TransitGatewayAttachmentStatusType` for a Client VPN one, whose catalog has no word for
//! the transient state of a rejection at all, so that kind reaches `rejected` at once).
//! The acceptance side of the same state machine is `transit_gateway_acceptance.rs`, which
//! is also where the fixtures both halves share live (the seeded attachments, the request
//! builder, the store reader and the Region every call is signed for).

use ec2::state::{
    CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE, ComAmazonawsEc2Amazonec2State as Ec2State,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED, TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{element, error_code, unescaped};
// The seeded attachments, the request builder, the store reader and the Region live in the
// acceptance module — ONE set of fixtures for the one state machine both halves drive.
use super::transit_gateway_acceptance::{
    DRY_RUN_STATUS, attachment_param, invoke, past_the_transition, pending_client_vpn_attachment,
    pending_vpc_attachment, stored_state,
};

/// One kind of attachment a rejection can act on: the operations that decide it, the element
/// its result carries it in, the spellings its own state enum gives the states of a
/// rejection, and a member it was STORED with that the response must carry back.
struct RejectableKind {
    /// The ec2Query `Action` that rejects an attachment of this kind.
    reject: &'static str,
    /// The ec2Query `Action` that accepts one — ruled out once a rejection has started.
    accept: &'static str,
    /// The element of the result the rejected attachment comes back in.
    result: &'static str,
    /// `pendingAcceptance`, as this kind's state enum spells it.
    pending_acceptance: &'static str,
    /// The state a rejection puts the attachment into at once.
    rejecting: &'static str,
    /// The terminal state the clock owes it.
    rejected: &'static str,
    /// A pending attachment of this kind, as it is stored.
    view: fn() -> TransitGatewayAttachmentView,
    /// A `(element, value)` the attachment was stored with, which no request here names.
    stored_member: (&'static str, &'static str),
}

/// Every kind of attachment this surface rejects. One row per kind, so the rejection is
/// asserted for each spelling of the states rather than for the one kind an issue cited.
fn rejectable_kinds() -> Vec<RejectableKind> {
    vec![
        RejectableKind {
            reject: "RejectTransitGatewayVpcAttachment",
            accept: "AcceptTransitGatewayVpcAttachment",
            result: "transitGatewayVpcAttachment",
            pending_acceptance: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            rejecting: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING,
            rejected: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            view: pending_vpc_attachment,
            stored_member: ("vpcId", "vpc-0123456789abcdef0"),
        },
        RejectableKind {
            reject: "RejectTransitGatewayClientVpnAttachment",
            accept: "AcceptTransitGatewayClientVpnAttachment",
            result: "transitGatewayClientVpnAttachment",
            pending_acceptance: CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
            // `TransitGatewayAttachmentStatusType` names no transient state of a
            // rejection, so this kind is `rejected` from the moment it is rejected.
            rejecting: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            rejected: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            view: pending_client_vpn_attachment,
            stored_member: ("clientVpnEndpointId", "cvpn-endpoint-0123456789abcdef0"),
        },
    ]
}

/// Rejecting an attachment of ANY kind takes it out of the one state it waits in, answers
/// the STORED attachment in the state of the rejection, and leaves the clock owing it
/// `rejected`. A second decision — of either kind — is then refused, which is how the
/// first one's write is observable to a later request.
#[tokio::test]
async fn rejecting_an_attachment_of_each_kind_moves_it_towards_rejected() {
    for kind in rejectable_kinds() {
        let case = kind.reject;
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
            "case {case:?}: expected the rejected attachment in a <{result}> element, got {body}",
            result = kind.result
        );
        assert_eq!(
            element(&body, "transitGatewayAttachmentId").as_deref(),
            Some(attachment_id.as_str()),
            "case {case:?}: expected the attachment's own id, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(kind.rejecting),
            "case {case:?}: expected the state of a rejection {expected:?}, got {body}",
            expected = kind.rejecting
        );
        // A member the attachment was STORED with comes back with it, so the response is
        // the stored record rather than an echo of the request, which named only the id.
        let (member, value) = kind.stored_member;
        assert_eq!(
            element(&body, member).as_deref(),
            Some(value),
            "case {case:?}: expected the stored {member} {value:?}, got {body}"
        );
        assert_eq!(
            stored_state(&engine, &attachment_id),
            kind.rejecting,
            "case {case:?}: the store must hold what the response reported"
        );

        // A second rejection has nothing to reject, and the acceptance that was available
        // a moment earlier is ruled out: the attachment is no longer waiting.
        for action in [kind.reject, kind.accept] {
            let (status, body) = invoke(&router, action, &attachment_param(&attachment_id)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {action} after the rejection must be refused, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected IncorrectState from {action}, got {body}"
            );
        }

        // The move the clock owed falls due on its own, without anybody asking for it; the
        // next call to reach the store is what materialises it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(&router, case, &attachment_param(&attachment_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a rejected attachment cannot be rejected again, got {status} {body}"
        );
        assert_eq!(
            stored_state(&engine, &attachment_id),
            kind.rejected,
            "case {case:?}: the clock owed this attachment its rejected state"
        );
    }
}

/// A dry run of a rejection that WOULD have succeeded performs nothing: the attachment is
/// still waiting afterwards, and the rejection that follows is the one that moves it.
#[tokio::test]
async fn a_dry_run_leaves_a_rejectable_attachment_untouched() {
    for kind in rejectable_kinds() {
        let case = kind.reject;
        let (router, engine) = super::ec2_query::emulator();
        let attachment_id = engine
            .state::<Ec2State>()
            .lock()
            .insert_transit_gateway_attachment((kind.view)())
            .expect("the store holds the seeded attachment");

        let mut dry_run = attachment_param(&attachment_id);
        dry_run.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = invoke(&router, case, &dry_run).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_state(&engine, &attachment_id),
            kind.pending_acceptance,
            "case {case:?}: a dry run must not move the attachment"
        );

        let (status, body) = invoke(&router, case, &attachment_param(&attachment_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rejection after a dry run must succeed, got {status} with {body}"
        );
        assert_eq!(
            stored_state(&engine, &attachment_id),
            kind.rejecting,
            "case {case:?}: the rejection after a dry run is the call that moves it"
        );
    }
}

/// A value the service cannot read as a transit gateway attachment id at all is refused as
/// MALFORMED, before any lookup, while a well-formed id the account holds no attachment for
/// is refused as NOT FOUND — so a client can tell "not an id of this kind" from "no such
/// attachment". Asserted for each rejection, over values that differ in how they fail the
/// id's form: another resource's id, the right prefix with a non-hex suffix, and empty text.
#[tokio::test]
async fn a_rejection_tells_a_value_that_is_no_attachment_id_from_an_unknown_one() {
    let (router, _engine) = super::ec2_query::emulator();

    // A well-formed id of this kind — 17 lowercase hex digits — that nothing here mints.
    let unknown = format!(
        "{prefix}-00000000000000000",
        prefix = ec2::state::TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX
    );
    for kind in rejectable_kinds() {
        for (value, expected) in [
            (unknown.as_str(), ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND),
            // An id of another resource of this service is no attachment id.
            (
                "tgw-0123456789abcdef0",
                ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            ),
            // The right prefix with a suffix that is not hex at all.
            (
                "tgw-attach-not-hex",
                ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            ),
            (
                "",
                ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
            ),
        ] {
            let case = format!("{action} with {value:?}", action = kind.reject);
            let (status, body) = invoke(&router, kind.reject, &attachment_param(value)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {body}"
            );
            // The NotFound refusal names the resource kind as the `ResourceType` enum spells
            // it — `transit-gateway-attachment`
            // (`smithy_query(query="shape",shape="ResourceType")`) — and the id from the
            // request, so the wording is derived rather than resource-specific prose.
            if expected == ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND {
                let message = unescaped(&element(&body, "Message").unwrap_or_default());
                // The token SPELT OUT, so a change to the constant the handler formats with
                // fails here rather than moving both sides of the comparison together.
                assert_eq!(
                    message,
                    format!("The transit-gateway-attachment ID '{value}' does not exist"),
                    "case {case:?}: expected the refusal to name the resource type and the \
                     requested id, got {message:?} in {body}"
                );
            }
        }
    }
}

/// Every kind shares one `TransitGatewayAttachmentId` space, so an id naming an attachment
/// of ANOTHER kind names no attachment the rejection called can act on: it is refused as
/// unknown, and the attachment it does name is left where it was.
#[tokio::test]
async fn a_rejection_does_not_act_on_an_attachment_of_another_kind() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded: Vec<(RejectableKind, String)> = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        rejectable_kinds()
            .into_iter()
            .map(|kind| {
                let attachment_id = guard
                    .insert_transit_gateway_attachment((kind.view)())
                    .expect("the store holds the seeded attachment");
                (kind, attachment_id)
            })
            .collect()
    };

    for (kind, _) in &seeded {
        for (owner, attachment_id) in &seeded {
            if owner.reject == kind.reject {
                continue;
            }
            let case = format!("{} on the attachment of {}", kind.reject, owner.reject);
            let (status, body) = invoke(&router, kind.reject, &attachment_param(attachment_id)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
                "case {case:?}: expected the attachment's NotFound code, got {body}"
            );
            assert_eq!(
                stored_state(&engine, attachment_id),
                owner.pending_acceptance,
                "case {case:?}: the refused call must leave the attachment waiting"
            );
        }
    }
}
