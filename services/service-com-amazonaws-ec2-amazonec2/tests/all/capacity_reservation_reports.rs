// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two operations that REPORT on one Capacity Reservation —
//! `GetGroupsForCapacityReservation` and `GetCapacityReservationUsage` — driven over the wire
//! the service actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness).
//!
//! [`super::capacity_reservation_cancellation`] and
//! [`super::interruptible_capacity_reservation_allocations`] cover what the usage report says
//! about a reservation's counts and state; this suite pins the three answers the two reports
//! share:
//!
//! 1. `DryRun` is the permission probe, answered BEFORE the named reservation is resolved and
//!    before the store is read at all: "checks whether you have the required permissions for
//!    the action, without actually making the request … the error response is
//!    DryRunOperation" — so a probe is answered 412 `DryRunOperation` whatever its id names, a
//!    reservation the account holds, a well-formed id it does not, or a value that is no
//!    Capacity Reservation id at all;
//! 2. every Capacity Reservation id these two reports cannot resolve is refused
//!    `InvalidCapacityReservationId.Malformed` — a value outside the long id form EC2 issues
//!    and a well-formed id the account holds no reservation under alike, which is the strict
//!    reading their recorded contracts carry (`DescribeCapacityReservations` reports such an
//!    id not found, and that difference is a fact of the operation);
//! 3. "Lists the resource groups to which a Capacity Reservation has been added"
//!    (`GetGroupsForCapacityReservation`): a reservation the account holds reports an EMPTY
//!    group set with no `nextToken`, because a reservation joins a resource group through the
//!    Resource Groups API and no EC2 operation adds one — and a `NextToken` this service did
//!    not issue is refused before the store is read.
//!
//! Each table row is a distinct input along what its rule generalises over — how much capacity
//! the reservation holds, what the probe's id names, every way an id can fail to resolve, and
//! which of the two reports answers — so no row is the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, member};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The instance type, platform and zone every reservation in this suite reserves capacity for
/// — members of the modelled `InstanceType` and `CapacityReservationInstancePlatform`
/// catalogs.
const INSTANCE_TYPE: &str = "c5.xlarge";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";
const AVAILABILITY_ZONE: &str = "us-east-1a";

/// The two operations this suite reports through.
const GROUPS: &str = "GetGroupsForCapacityReservation";
const USAGE: &str = "GetCapacityReservationUsage";

/// A WELL-FORMED Capacity Reservation id the account holds no reservation for: inside the long
/// form EC2 issues (`crate::state::capacity_reservation_id_is_well_formed`, seventeen
/// lowercase hexadecimal digits with a leading zero), so the service looks it up rather than
/// refusing it for its shape.
const UNKNOWN_CAPACITY_RESERVATION_ID: &str = "cr-0ffffffffffffffff";

/// An `active` Capacity Reservation of `instance_count` instances, created over the wire.
async fn create_reservation(router: &MultiServiceRouter, instance_count: i32) -> String {
    let params = [
        param("InstanceType", INSTANCE_TYPE),
        param("InstancePlatform", INSTANCE_PLATFORM),
        param("InstanceCount", &instance_count.to_string()),
        param("AvailabilityZone", AVAILABILITY_ZONE),
    ];
    let (status, body) = call(router, REGION, "CreateCapacityReservation", &params).await;
    assert_eq!(
        status, 200,
        "reserving {instance_count} instances: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "capacityReservationId")
        .unwrap_or_else(|| panic!("reserving {instance_count} instances: no id in {body}"))
}

/// One call of `action` naming `capacity_reservation_id`, with `extra` beside it.
async fn report(
    router: &MultiServiceRouter,
    action: &str,
    capacity_reservation_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![param("CapacityReservationId", capacity_reservation_id)];
    params.extend_from_slice(extra);
    call(router, REGION, action, &params).await
}

/// RULE 3: a reservation the account holds reports an empty group set with no page after it,
/// and reading its groups changes nothing a later reader sees.
#[tokio::test]
async fn a_reservation_reports_the_groups_it_has_been_added_to() {
    // Rows differ in how much capacity the reservation holds: one instance, a few, and many —
    // a reservation's group memberships are not a function of its size, so every row reports
    // the same empty set.
    for instance_count in [1, 4, 64] {
        let case = format!("{instance_count} instances");
        let (router, _engine) = super::ec2_query::emulator();
        let reservation_id = create_reservation(&router, instance_count).await;

        let (status, body) = report(&router, GROUPS, &reservation_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {GROUPS}, got {status} with body {body}"
        );
        assert!(
            super::ec2_query::elements(&body, "groupArn").is_empty(),
            "case {case:?}: a reservation no Resource Groups call has added to a group is in \
             none, got {body}"
        );
        assert_eq!(
            member(&body, "nextToken"),
            None,
            "case {case:?}: one page carries the whole list, got {body}"
        );

        // A page size the request names changes nothing about an empty list, and the
        // reservation is still readable afterwards — the report is a READ.
        let (status, body) = report(
            &router,
            GROUPS,
            &reservation_id,
            &[param("MaxResults", "1000")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 naming a page size, got {status} with body {body}"
        );
        assert!(
            super::ec2_query::elements(&body, "groupArn").is_empty(),
            "case {case:?}: expected the same empty list, got {body}"
        );
        let (status, body) = report(&router, USAGE, &reservation_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reservation must still read back, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "totalInstanceCount").as_deref(),
            Some(instance_count.to_string().as_str()),
            "case {case:?}: reading the groups must not change the reservation, got {body}"
        );

        // A `NextToken` this service did not issue names no position in that list, and it is
        // refused before the store is read.
        let (status, body) = report(
            &router,
            GROUPS,
            &reservation_id,
            &[param("NextToken", "not-a-token")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a forged NextToken: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: a forged NextToken: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 2: every id these two reports cannot resolve is refused the malformed half.
#[tokio::test]
async fn an_id_the_reports_cannot_resolve_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let known = create_reservation(&router, 2).await;

    // The rows differ in the FORM of the id: the long form EC2 issues (which is looked up and
    // not held), the all-zero placeholder, the short form, a leading digit above zero, another
    // resource's prefix, a known id with a suffix, and something that is not an id at all.
    let rows: [(&str, String); 7] = [
        (
            "a well-formed id nothing was reserved under",
            UNKNOWN_CAPACITY_RESERVATION_ID.to_string(),
        ),
        ("the null id", "cr-00000000000000000".to_string()),
        ("the short id form", "cr-12345678".to_string()),
        (
            "a leading digit above zero",
            "cr-1234567890abcdef0".to_string(),
        ),
        (
            "another resource's prefix",
            "cb-00000000000000001".to_string(),
        ),
        ("a known id with a suffix", format!("{known}x")),
        ("not an id at all", "not-an-id".to_string()),
    ];

    for (case, capacity_reservation_id) in rows {
        for action in [GROUPS, USAGE] {
            let (status, body) = report(&router, action, &capacity_reservation_id, &[]).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidCapacityReservationId.Malformed",
                "case {case:?} on {action}: expected the malformed half, got {body}"
            );
            assert!(
                error_message(&body).contains(&capacity_reservation_id),
                "case {case:?} on {action}: expected the message to name the id, got {body}"
            );
        }
    }
}

/// RULE 1: `DryRun` answers the permission probe whatever the id names, and reports nothing.
#[tokio::test]
async fn a_dry_run_answers_the_probe_whatever_the_id_names() {
    let (router, _engine) = super::ec2_query::emulator();
    let known = create_reservation(&router, 2).await;

    // The rows differ in what the probe's id names: a reservation the account holds, a
    // well-formed id it does not, and a value that is no Capacity Reservation id at all.
    // "Checks whether you have the required permissions … without actually making the
    // request", so all three answer the same thing on both reports.
    let rows: [(&str, &str); 3] = [
        ("a reservation the account holds", known.as_str()),
        (
            "a well-formed id the account does not hold",
            UNKNOWN_CAPACITY_RESERVATION_ID,
        ),
        ("a value that is no reservation id", "not-an-id"),
    ];

    for (case, capacity_reservation_id) in rows {
        for action in [GROUPS, USAGE] {
            let (status, body) = report(
                &router,
                action,
                capacity_reservation_id,
                &[param("DryRun", "true")],
            )
            .await;
            assert_eq!(
                status, 412,
                "case {case:?} on {action}: expected HTTP 412 from a DryRun, got {status} \
                 with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "DryRunOperation",
                "case {case:?} on {action}: expected DryRunOperation, got {body}"
            );
            assert_eq!(
                error_message(&body),
                "Request would have succeeded, but DryRun flag is set.",
                "case {case:?} on {action}: expected the wording EC2 answers a permitted \
                 probe, got {body}"
            );
            assert!(
                member(&body, "capacityReservationGroupSet").is_none()
                    && member(&body, "totalInstanceCount").is_none(),
                "case {case:?} on {action}: a probe must report nothing, got {body}"
            );
        }
    }

    // `DryRun=false` is not a probe: both reports are served.
    for action in [GROUPS, USAGE] {
        let (status, body) = report(&router, action, &known, &[param("DryRun", "false")]).await;
        assert_eq!(
            status, 200,
            "DryRun=false on {action}: expected HTTP 200, got {status} with body {body}"
        );
    }
}
