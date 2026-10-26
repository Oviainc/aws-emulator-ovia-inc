// SPDX-License-Identifier: CC0-1.0

//! An environment whose deletion has been ACCEPTED holds no Elastic IP association, and
//! takes no new one.
//!
//! The rule under test, one level above the example: an Elastic IP association is one
//! "with the elastic network interface in the VLAN subnet" (`EipAssociation` doc) and
//! "environment deletion also deletes the associated Amazon EVS VLAN subnets"
//! (`DeleteEnvironment` doc), so from the instant a deletion is accepted the environment's
//! VLANs have no subnet for an address to be associated with: the deletion releases every
//! association they held — on the account's own Amazon EC2 `Address` records and on the
//! VLAN records this service reports — and a later `AssociateEipToVlan` on that
//! environment is REFUSED instead of consuming an allocation nothing can give back. So,
//! whatever the associate names and however far the deletion has got:
//!
//! * the call is refused (`ValidationException` while the environment is `DELETING`,
//!   `ResourceNotFoundException` once it has reached `DELETED` — "The resource might not be
//!   specified correctly, or it may have a state of DELETED", `ResourceNotFoundException`
//!   doc), and EC2's record of the allocation is left unassociated: nothing is consumed;
//! * the associations the VLAN held before the delete are gone from BOTH stores, so no
//!   reader is told the VLAN holds an address that nothing is associated with;
//! * every allocation STAYS allocated to the account and is associable again — a live
//!   environment takes it afterwards, which is what an allocation left consumed by a
//!   deleted environment could never allow.
//!
//! The rows differ along what the rule generalises over: how many associations the VLAN
//! held when the delete arrived (none, one, several), how far the deletion had got when
//! the associate was tried (in flight, and come due), and which allocation the associate
//! names (one the VLAN never held, and one the deletion has just released from it — the
//! `@idempotent` repeat, which must not report an association that no longer exists).
//!
//! Everything here runs over the wire against a router carrying BOTH services on one
//! engine ([`crate::vlan_subnet_propagation::emulator`]), because what the rows assert is
//! that Amazon EVS and Amazon EC2 read ONE store: the harness is the one
//! [`crate::eip_ec2_propagation`] already owns.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::eip_ec2_propagation::{
    allocate_address, associate, create_environment, ec2_address, reported_association,
    vlan_associations, Environment, Network,
};
use crate::environment_lifecycle::{call_in_region, error_type, PAST_THE_TRANSITION};
use crate::vlan_subnet_propagation::emulator;

/// How far the environment's deletion had got when the row's associate was tried.
#[derive(Debug, Clone, Copy)]
enum Deletion {
    /// Accepted and still in flight: the environment reports `DELETING`.
    InFlight,
    /// Come due: the environment reports `DELETED`.
    Settled,
}

/// Which allocation the row's associate names.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// An allocation of the account the VLAN never held.
    NeverHeld,
    /// The first allocation the VLAN held before the delete, so the associate is the
    /// `@idempotent` repeat of an association the deletion has already released.
    ReleasedByTheDelete,
}

/// The network the environment under deletion is deployed into.
const HOLDER: Network = Network {
    region: "us-west-2",
    zone: "us-west-2a",
    base: "10.0",
};

/// The network of the live environment that takes the allocations over afterwards.
const SUCCESSOR: Network = Network {
    region: "us-west-2",
    zone: "us-west-2b",
    base: "10.1",
};

/// Delete `environment` and answer the `environmentState` the call reports.
async fn delete_environment(
    router: &MultiServiceRouter,
    environment: &Environment,
    case: &str,
) -> String {
    let (status, body) = call_in_region(
        router,
        environment.network.region,
        "DeleteEnvironment",
        &json!({ "environmentId": environment.environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteEnvironment expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"]["environmentState"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentState in {body}"))
        .to_string()
}

/// The `environmentState` `GetEnvironment` reports for `environment`.
async fn environment_state(
    router: &MultiServiceRouter,
    environment: &Environment,
    case: &str,
) -> String {
    let (status, body) = call_in_region(
        router,
        environment.network.region,
        "GetEnvironment",
        &json!({ "environmentId": environment.environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnvironment expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"]["environmentState"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentState in {body}"))
        .to_string()
}

#[tokio::test]
async fn an_environment_being_deleted_releases_its_eips_and_takes_no_new_association() {
    // Each row: the case, how many allocations the VLAN held when the delete arrived, how
    // far the deletion had got when the associate was tried, which allocation it named,
    // and the modelled error the refusal carries.
    let rows: Vec<(&str, usize, Deletion, Target, &str)> = vec![
        (
            "an allocation the VLAN never held, while the deletion is in flight",
            0,
            Deletion::InFlight,
            Target::NeverHeld,
            "ValidationException",
        ),
        (
            "an allocation the VLAN never held, with three released by the delete",
            3,
            Deletion::InFlight,
            Target::NeverHeld,
            "ValidationException",
        ),
        (
            "the allocation the delete just released, while the deletion is in flight",
            1,
            Deletion::InFlight,
            Target::ReleasedByTheDelete,
            "ValidationException",
        ),
        (
            "an allocation the VLAN never held, once the deletion has come due",
            0,
            Deletion::Settled,
            Target::NeverHeld,
            "ResourceNotFoundException",
        ),
        (
            "the allocation the delete released, once the deletion has come due",
            2,
            Deletion::Settled,
            Target::ReleasedByTheDelete,
            "ResourceNotFoundException",
        ),
    ];

    for (case, held_count, deletion, target, expected_error) in rows {
        // A fresh emulator per row, so what Amazon EC2 holds is exactly what this row made.
        let router = emulator();
        let holder = create_environment(&router, HOLDER, "eip-deleted-holder").await;
        let successor = create_environment(&router, SUCCESSOR, "eip-successor").await;
        let region = holder.network.region;

        // The associations the VLAN holds when the delete arrives: each is a real EC2
        // association of an allocation of the account.
        let mut held: Vec<(String, String, String)> = Vec::new();
        for index in 0..held_count {
            let (allocation_id, public_ip) = allocate_address(&router, region).await;
            let (status, body) = associate(&router, &holder, &allocation_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: associate {index} expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let association_id = reported_association(&body, &allocation_id, case)["associationId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no associationId in {body}"))
                .to_string();
            let (_, in_ec2) = ec2_address(&router, region, &allocation_id).await;
            assert_eq!(
                in_ec2.as_deref(),
                Some(association_id.as_str()),
                "case {case:?}: allocation {index} must be associated in EC2 with \
                 {association_id} before the delete, got {in_ec2:?}"
            );
            held.push((allocation_id, public_ip, association_id));
        }
        // The allocation a `NeverHeld` row names: an address of the account that no VLAN
        // has ever used, so what refuses the associate is the environment's deletion and
        // nothing about the allocation.
        let (never_held, _) = allocate_address(&router, region).await;
        let (target_allocation, previous_association) = match target {
            Target::NeverHeld => (never_held.clone(), None),
            Target::ReleasedByTheDelete => {
                let (allocation_id, _, association_id) = held
                    .first()
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: this row holds an association"));
                (allocation_id, Some(association_id))
            }
        };

        let reported = delete_environment(&router, &holder, case).await;
        assert_eq!(
            reported, "DELETING",
            "case {case:?}: the delete reports the environment going away, got {reported}"
        );

        // THE POINT, half one: while the deletion is in flight the environment takes no
        // new association — the VLAN subnet an address would be associated with is
        // already gone — so the call is refused rather than consuming the allocation.
        if matches!(deletion, Deletion::InFlight) {
            let (status, body) = associate(&router, &holder, &target_allocation).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an associate on a departing environment expected HTTP 400, \
                 got {status} {body}"
            );
            assert!(
                error_type(&body).contains(expected_error),
                "case {case:?}: expected {expected_error}, got {} in {body}",
                error_type(&body)
            );
        }

        // Past the transition the deletion has come due.
        std::thread::sleep(PAST_THE_TRANSITION);
        let settled = environment_state(&router, &holder, case).await;
        assert_eq!(
            settled, "DELETED",
            "case {case:?}: the environment settles to DELETED, got {settled}"
        );

        // THE POINT, half two: and a deleted environment is not there to associate with
        // at all.
        if matches!(deletion, Deletion::Settled) {
            let (status, body) = associate(&router, &holder, &target_allocation).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an associate on a deleted environment expected HTTP 400, \
                 got {status} {body}"
            );
            assert!(
                error_type(&body).contains(expected_error),
                "case {case:?}: expected {expected_error}, got {} in {body}",
                error_type(&body)
            );
        }

        // The deletion released every association the VLAN held, in this service's own
        // record as much as in EC2's.
        let left = vlan_associations(&router, &holder).await;
        assert!(
            left.is_empty(),
            "case {case:?}: a deleted environment's VLAN holds no association, got {left:?}"
        );
        for (index, (allocation_id, public_ip, association_id)) in held.iter().enumerate() {
            let (held_ip, held_association) = ec2_address(&router, region, allocation_id).await;
            assert_eq!(
                held_ip, *public_ip,
                "case {case:?}: allocation {index} stays allocated with its address, got \
                 {held_ip}"
            );
            assert_eq!(
                held_association, None,
                "case {case:?}: allocation {index} must be unassociated in EC2 after the \
                 delete, got {held_association:?} (was {association_id})"
            );
        }
        // And the refused associate consumed nothing: the allocation it named carries no
        // association of its own.
        let (_, after_refusal) = ec2_address(&router, region, &target_allocation).await;
        assert_eq!(
            after_refusal, None,
            "case {case:?}: the refused associate must leave {target_allocation} \
             unassociated, got {after_refusal:?}"
        );

        // The whole point of releasing an address rather than leaving it consumed: a LIVE
        // environment can associate it, which is what an allocation held by a deleted
        // environment could never allow.
        let (status, body) = associate(&router, &successor, &target_allocation).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a live environment taking the released {target_allocation} \
             expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let association = reported_association(&body, &target_allocation, case);
        let association_id = association["associationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no associationId in {association}"))
            .to_string();
        if let Some(previous_association) = previous_association {
            assert_ne!(
                association_id, previous_association,
                "case {case:?}: a new association is a new id, got {association_id}"
            );
        }
        let (_, now_in_ec2) = ec2_address(&router, region, &target_allocation).await;
        assert_eq!(
            now_in_ec2.as_deref(),
            Some(association_id.as_str()),
            "case {case:?}: EC2 must report the live environment's association, got \
             {now_in_ec2:?}"
        );
        let taken = vlan_associations(&router, &successor).await;
        assert_eq!(
            taken.len(),
            1,
            "case {case:?}: the live environment's VLAN carries exactly the association it \
             made, got {taken:?}"
        );
    }
}
