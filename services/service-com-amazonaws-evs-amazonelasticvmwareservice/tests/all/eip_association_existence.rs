// SPDX-License-Identifier: CC0-1.0

//! What an `associationId` may NAME on `DisassociateEipFromVlan`: an Elastic IP
//! association OF THE VLAN THE REQUEST ADDRESSES.
//!
//! The rule under test, one level above the example: an operation addressed by a handle
//! scoped to a parent resource answers the modelled `ResourceNotFoundException` for a
//! handle that names no record of THAT parent — including a handle another parent of the
//! same kind holds — and an `@idempotent` repeat is served only for a record the
//! addressed parent itself held and the service has already removed. It is the rule
//! `DeleteEnvironmentHost` follows for its `hostName` and `DeleteEnvironmentConnector`
//! for its `connectorId` (`tests/all/host_deletion.rs`,
//! `tests/all/connector_deletion.rs`), and the reason `AssociateEipToVlan` refuses an
//! allocation another environment's VLAN is using rather than moving it
//! (`tests/all/eip_ec2_propagation.rs`). So, for the `associationId` of
//! `DisassociateEipFromVlan`:
//!
//! * an id the addressed VLAN has never carried is refused with HTTP 400
//!   `ResourceNotFoundException` naming the id the caller gave, and neither this
//!   service's VLAN records nor the account's own Amazon EC2 `Address` records change;
//! * an association ANOTHER environment's public HCX VLAN holds is such an id — the
//!   holder keeps it, in both stores;
//! * an association the addressed VLAN carries is removed, and the repeat of THAT call
//!   still answers with the VLAN that no longer carries it;
//! * that removal belongs to the VLAN which made it: the same id addressed to another
//!   environment's VLAN is still the missing resource.
//!
//! The rows differ along what the rule generalises over: whether the id names a live
//! association of the addressed VLAN, one of another VLAN, one the addressed VLAN had
//! removed, or one nothing ever minted — and, for the never-minted ones, the LENGTH and
//! character classes of the id, at both published bounds of the modelled `AssociationId`
//! (`smithy_query(query="shape",shape="AssociationId")`: length 9..26, pattern
//! `^eipassoc-[a-zA-Z0-9_-]+$`). The issue's own example — one never-minted
//! `eipassoc-0123456789abcdef0` — is one row.
//!
//! Everything here runs over the wire against a router carrying BOTH services on one
//! engine ([`crate::vlan_subnet_propagation::emulator`]), so the Elastic IP addresses are
//! allocated through Amazon EC2 itself and what a refusal left untouched is read back
//! from EC2's own `DescribeAddresses`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::eip_ec2_propagation::{
    allocate_address, associate, create_environment, ec2_address, reported_association,
    vlan_associations, Environment, Network,
};
use crate::environment_lifecycle::{call_in_region, error_type};
use crate::vlan_subnet_propagation::emulator;

/// The Region both environments under test are addressed in.
const REGION: &str = "us-west-2";

/// The network of the environment a row addresses.
const ADDRESSED: Network = Network {
    region: REGION,
    zone: "us-west-2a",
    base: "10.0",
};

/// The network of the OTHER environment, whose public HCX VLAN holds an association of
/// its own.
const OTHER: Network = Network {
    region: REGION,
    zone: "us-west-2b",
    base: "10.1",
};

/// Allocate one Elastic IP address of the account through Amazon EC2 and associate it
/// with `environment`'s public HCX VLAN, answering `(allocationId, associationId)`.
async fn associate_a_fresh_address(
    router: &MultiServiceRouter,
    environment: &Environment,
    case: &str,
) -> (String, String) {
    let (allocation_id, _) = allocate_address(router, environment.network.region).await;
    let (status, body) = associate(router, environment, &allocation_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: associate expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let association = reported_association(&body, &allocation_id, case);
    let association_id = association["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no associationId in {association}"))
        .to_string();
    (allocation_id, association_id)
}

/// Ask `environment`'s public HCX VLAN to drop `association_id`.
async fn disassociate(
    router: &MultiServiceRouter,
    environment: &Environment,
    association_id: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        environment.network.region,
        "DisassociateEipFromVlan",
        &json!({
            "environmentId": environment.environment_id,
            "vlanName": "hcx",
            "associationId": association_id,
        }),
    )
    .await
}

/// The `associationId`s `environment`'s public HCX VLAN carries, in stored order, read
/// back through `ListEnvironmentVlans`.
async fn stored_association_ids(
    router: &MultiServiceRouter,
    environment: &Environment,
) -> Vec<String> {
    vlan_associations(router, environment)
        .await
        .iter()
        .filter_map(|association| association["associationId"].as_str().map(str::to_string))
        .collect()
}

/// The `associationId`s a response's `vlan` reports, in reported order.
fn reported_association_ids(case: &str, body: &Value) -> Vec<String> {
    body["vlan"]["eipAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no eipAssociations in {body}"))
        .iter()
        .filter_map(|association| association["associationId"].as_str().map(str::to_string))
        .collect()
}

/// The refusal the not-found class answers with: the modelled `ResourceNotFoundException`
/// naming the handle the request carried.
fn assert_not_found(case: &str, status: u16, body: &Value, association_id: &str) {
    // Every modelled CLIENT error of this awsJson1_0 service is reported with HTTP 400 —
    // `tests/all/error_wire_status.rs` owns that rule.
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert_eq!(
        error_type(body),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["resourceId"],
        json!(association_id),
        "case {case:?}: expected resourceId {association_id:?}, got {}",
        body["resourceId"]
    );
    assert!(
        body["resourceType"]
            .as_str()
            .is_some_and(|resource_type| !resource_type.is_empty()),
        "case {case:?}: expected a resourceType on the refusal, got {}",
        body["resourceType"]
    );
    // Built from the request: the refusal names the id the caller asked for.
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains(association_id)),
        "case {case:?}: expected the refusal to name {association_id:?}, got {}",
        body["message"]
    );
}

/// `association_id` with its last character replaced by `z`: a value the modelled
/// pattern admits and EC2's own minter — `<prefix>-` and 17 LOWERCASE HEX digits — can
/// never produce, so it names no association of any VLAN.
fn with_last_character_replaced(association_id: &str) -> String {
    let kept: String = association_id
        .chars()
        .take(association_id.chars().count().saturating_sub(1))
        .collect();
    format!("{kept}z")
}

#[tokio::test]
async fn an_association_id_the_addressed_vlan_never_held_is_not_found_and_changes_nothing() {
    let router = emulator();
    let addressed = create_environment(&router, ADDRESSED, "eip-handle-addressed").await;
    let other = create_environment(&router, OTHER, "eip-handle-other").await;

    let (held_allocation, held) =
        associate_a_fresh_address(&router, &addressed, "the addressed VLAN's own association").await;
    let (other_allocation, held_by_other) = associate_a_fresh_address(
        &router,
        &other,
        "the other environment's own association",
    )
    .await;

    // Each row: the case, and the `associationId` the request names. Every one of them is
    // a value the modelled `AssociationId` admits, so no row's outcome can depend on the
    // shape of the id rather than on the addressed VLAN holding an association of it.
    let rows: Vec<(&str, String)> = vec![
        (
            "an association another environment's public HCX VLAN still holds",
            held_by_other.clone(),
        ),
        (
            // The pattern needs at least one character after `eipassoc-`, so this is the
            // shortest value the model admits.
            "an id nothing minted, as short as the model admits",
            "eipassoc-a".to_string(),
        ),
        (
            // 9 + 17 characters: the model's length maximum.
            "an id nothing minted, as long as the model admits",
            "eipassoc-0123456789abcdef0".to_string(),
        ),
        (
            "an id nothing minted, of the underscore and dash characters the model admits",
            "eipassoc-a_b-c".to_string(),
        ),
        (
            "an id nothing minted, of the upper-case letters the model admits",
            "eipassoc-ABCDEF".to_string(),
        ),
        (
            "an id nothing minted, all digits",
            "eipassoc-00000000".to_string(),
        ),
        (
            "an id differing from the addressed VLAN's own in its last character",
            with_last_character_replaced(&held),
        ),
    ];

    for (case, association_id) in rows {
        let (status, body) = disassociate(&router, &addressed, &association_id).await;
        assert_not_found(case, status, &body, &association_id);

        // Nothing moved in either store: each VLAN keeps the one association it had...
        let addressed_ids = stored_association_ids(&router, &addressed).await;
        assert_eq!(
            addressed_ids,
            vec![held.clone()],
            "case {case:?}: expected the addressed VLAN to keep {held:?}, got {addressed_ids:?}"
        );
        let other_ids = stored_association_ids(&router, &other).await;
        assert_eq!(
            other_ids,
            vec![held_by_other.clone()],
            "case {case:?}: expected the other VLAN to keep {held_by_other:?}, got {other_ids:?}"
        );
        // ...and so does Amazon EC2's own record of each allocation.
        for (whose, allocation_id, association) in [
            ("the addressed VLAN", &held_allocation, &held),
            ("the other VLAN", &other_allocation, &held_by_other),
        ] {
            let (_, associated_with) = ec2_address(&router, REGION, allocation_id).await;
            assert_eq!(
                associated_with.as_deref(),
                Some(association.as_str()),
                "case {case:?}: expected EC2 to keep {whose}'s association {association:?} on \
                 {allocation_id}, got {associated_with:?}"
            );
        }
    }

    // And the refusals are not a blanket one: the association the addressed VLAN DOES
    // hold is removed, here and on the account's EC2 record.
    let (status, body) = disassociate(&router, &addressed, &held).await;
    assert_eq!(
        status, 200,
        "the association the addressed VLAN holds: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let left = reported_association_ids("the association the addressed VLAN holds", &body);
    assert!(
        left.is_empty(),
        "the association the addressed VLAN holds: expected the VLAN to be left with none, \
         got {left:?}"
    );
    let (_, associated_with) = ec2_address(&router, REGION, &held_allocation).await;
    assert_eq!(
        associated_with, None,
        "the removal must give the allocation back on EC2, got {associated_with:?}"
    );
}

#[tokio::test]
async fn the_idempotent_repeat_resolves_only_on_the_vlan_whose_association_was_removed() {
    let router = emulator();
    let holder = create_environment(&router, ADDRESSED, "eip-repeat-holder").await;
    let other = create_environment(&router, OTHER, "eip-repeat-other").await;

    let (removed_allocation, removed) =
        associate_a_fresh_address(&router, &holder, "the association the calls below remove").await;
    let (kept_allocation, kept) =
        associate_a_fresh_address(&router, &holder, "the association kept throughout").await;
    let (_, live_on_other) =
        associate_a_fresh_address(&router, &other, "the other VLAN's live association").await;

    // The call that removes it and every repeat of that call answer alike: the VLAN no
    // longer carries it, and the association it was not asked about stays.
    for attempt in [
        "the call that removes the association",
        "the first repeat of that call",
        "a second repeat of that call",
    ] {
        let (status, body) = disassociate(&router, &holder, &removed).await;
        assert_eq!(
            status, 200,
            "case {attempt:?}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let left = reported_association_ids(attempt, &body);
        assert_eq!(
            left,
            vec![kept.clone()],
            "case {attempt:?}: expected only {kept:?} to be left, got {left:?}"
        );
        // Amazon EC2 holds the removal once: the allocation is unassociated and still the
        // account's, while the association nobody asked about is untouched.
        let (_, released) = ec2_address(&router, REGION, &removed_allocation).await;
        assert_eq!(
            released, None,
            "case {attempt:?}: expected the allocation to be unassociated on EC2, got \
             {released:?}"
        );
        let (_, still_held) = ec2_address(&router, REGION, &kept_allocation).await;
        assert_eq!(
            still_held.as_deref(),
            Some(kept.as_str()),
            "case {attempt:?}: expected EC2 to keep {kept:?} on {kept_allocation}, got \
             {still_held:?}"
        );
    }

    // The removal belongs to the VLAN that made it, and so does a live association: both
    // ids are the missing resource for the OTHER environment's VLAN, and the other way
    // round.
    for (case, environment, association_id) in [
        (
            "the id this VLAN removed, addressed to another environment's VLAN",
            &other,
            &removed,
        ),
        (
            "another environment's live association, addressed to this VLAN",
            &holder,
            &live_on_other,
        ),
    ] {
        let (status, body) = disassociate(&router, environment, association_id).await;
        assert_not_found(case, status, &body, association_id);
    }

    // Which left both VLANs exactly as they stood.
    let holder_ids = stored_association_ids(&router, &holder).await;
    assert_eq!(
        holder_ids,
        vec![kept.clone()],
        "expected this VLAN to keep {kept:?}, got {holder_ids:?}"
    );
    let other_ids = stored_association_ids(&router, &other).await;
    assert_eq!(
        other_ids,
        vec![live_on_other.clone()],
        "expected the other VLAN to keep {live_on_other:?}, got {other_ids:?}"
    );
}
