// SPDX-License-Identifier: CC0-1.0

//! What this service answers for an `OutpostId` that BOTH the operator-declared
//! sharing and the co-hosted AWS Outposts control plane hold a record for — driven end
//! to end over HTTP across both emulated services.
//!
//! The rule under test: AWS Outposts owns the Outpost, so for an `OutpostId` that
//! control plane holds an Outpost under, its own record is the whole of this service's
//! answer — the Outpost is reported and resolvable exactly when that record is active,
//! not being decommissioned and carries ordered S3 storage capacity, and is reported
//! and resolved nowhere otherwise. A declaration of this service's own is the answer
//! only for an id that control plane holds no Outpost under; it can neither change what
//! it reports for a held id nor keep that authority's negative verdict out of it.
//!
//! The first table varies which verdict the control plane reaches on an id a
//! declaration also names — reported with S3 capacity, each of the three gates
//! negative in turn, the Outpost deleted again, and an id it never held — and asserts
//! the declaration is invisible for every id it holds: the inventory the declaration is
//! added to is the one the control plane alone yields, and every reference to the id
//! resolves accordingly.
//!
//! The second table pins the propagation that follows from it: an Outpost leaving the
//! inventory by the control plane's verdict takes the endpoints stored on it with it,
//! declaration or no declaration.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_s3outposts_s3outposts::state::OutpostRecord;

use crate::control_plane_inventory::{
    deliver_orders, string_at, Catalog, Emulator, Item, REGION, SHARING_ACCOUNT_ID,
};
use crate::endpoint_lifecycle::OUTPOST_MISSING;
use crate::outpost_departure::{
    committed_cidrs, place_endpoint, reported_arns, stored,
};

/// An `OutpostId` of the modelled `op-[a-f0-9]{17}` branch that no `CreateOutpost` of
/// these cases mints, so the control plane holds no Outpost under it: what the
/// "never held" row declares.
const UNHELD_OUTPOST_ID: &str = "op-3d4e5f60718293a4b";

/// The S3 capacity every declaration here is made with — 24 TiB in bytes, the unit
/// `Outpost$CapacityInBytes` ("the Amazon S3 capacity of the outpost in bytes") is
/// stated in. Deliberately not a figure the `outposts` catalog publishes for any
/// configuration, so a reported capacity says which side answered.
const DECLARED_CAPACITY: i64 = 24 * 1024 * 1024 * 1024 * 1024;

/// The subnet and security group the surviving Outpost's own endpoint sits in.
const SURVIVOR_SUBNET: &str = "subnet-0f1e2d3c4b5a69781";
const SURVIVOR_SECURITY_GROUP: &str = "sg-0f1e2d3c4b5a69781";

/// The verdict the AWS Outposts control plane reaches on the id a declaration also
/// names, and the route this suite drives it there by — every one of them a route a
/// client drives through that control plane's own API.
#[derive(Debug, Clone, Copy)]
enum Verdict {
    /// It holds the id as an active Outpost the orders on it bought S3 storage
    /// capacity for.
    Provisioned,
    /// It holds the id, with S3 capacity ordered, but the Outpost has not finished
    /// provisioning: the hardware its order bought is not installed yet.
    StillProvisioning,
    /// It holds the id as an active Outpost with S3 capacity ordered, and a
    /// decommission of it has been requested.
    Decommissioning,
    /// It holds the id as an active Outpost whose only S3-capable order was
    /// cancelled.
    OrderCancelled,
    /// It holds the id as an active Outpost no order bought S3 storage capacity for.
    NoS3Capacity,
    /// It HELD the id and no longer does: the Outpost was deleted.
    Deleted,
    /// It holds no Outpost under the id at all.
    NeverHeld,
}

/// What the inventory reports for the id once a declaration names it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The control plane's own record, unchanged by the declaration.
    ControlPlane,
    /// The declared record, because the control plane holds no Outpost under the id.
    Declared,
    /// Nothing: the control plane holds the id and its verdict on it is negative.
    Absent,
}

/// One row of the first table.
struct Row {
    case: &'static str,
    verdict: Verdict,
    answer: Answer,
}

/// The `Outpost` the inventory of `REGION` reports for `outpost_id`, or `None` when it
/// reports none.
async fn reported_outpost(emulator: &Emulator, outpost_id: &str) -> Option<Value> {
    emulator
        .inventory(REGION)
        .await
        .into_iter()
        .find(|outpost| string_at(outpost, "OutpostId") == outpost_id)
}

/// Declare `outpost_id` as an Outpost of ANOTHER account shared with the caller, the
/// way the operator's own setting does.
///
/// No AWS API of this emulator shares an Outpost — Resource Access Manager is not
/// emulated and the control plane serves one account — so the declaration is made
/// through the same store seam that setting is read into.
fn declare_outpost(emulator: &Emulator, outpost_id: &str) {
    let store = emulator.s3_store();
    let mut store = store.lock();
    store.provision_outpost(
        OutpostRecord::provisioned(
            outpost_id,
            SHARING_ACCOUNT_ID,
            DECLARED_CAPACITY,
            REGION,
        )
        .expect("the declared Outpost is a provisionable record"),
    );
}

/// `outposts:StartOutpostDecommission` on `outpost_id`.
async fn request_decommission(emulator: &Emulator, outpost_id: &str) {
    let reply = emulator
        .outposts(
            REGION,
            "POST",
            &format!("/outposts/{outpost_id}/decommission"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (reply.status, string_at(&reply.body, "Status").as_str()),
        (200, "REQUESTED"),
        "StartOutpostDecommission must record the decommission, got {} {}",
        reply.status,
        reply.body
    );
}

/// Stand the control plane up at `verdict`, over the wire wherever an AWS API models
/// the step, and answer the `OutpostId` the row works on.
async fn at_verdict(emulator: &Emulator, catalog: &Catalog, verdict: Verdict) -> String {
    if matches!(verdict, Verdict::NeverHeld) {
        return UNHELD_OUTPOST_ID.to_string();
    }
    let outpost_id = emulator.an_outpost_in(REGION, "shadowed").await;
    match verdict {
        Verdict::NeverHeld => unreachable!("answered above"),
        Verdict::StillProvisioning => {
            // The order that buys the S3 capacity is placed and left undelivered, so
            // the capacity is ordered while the Outpost is still provisioning: the
            // lifecycle gate alone is what makes this verdict negative.
            emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::S3(0), 1)]))
                .await;
        }
        Verdict::NoS3Capacity => {
            emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::EbsOnly, 1)]))
                .await;
            deliver_orders(emulator);
        }
        Verdict::OrderCancelled => {
            // An order is cancellable only while work on it is outstanding, so the
            // hardware that makes the Outpost active is delivered by an EBS-only order
            // first and the S3-capable order is the one cancelled.
            emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::EbsOnly, 1)]))
                .await;
            deliver_orders(emulator);
            let order_id = emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::S3(1), 2)]))
                .await;
            emulator.cancel_order_in(REGION, &order_id).await;
        }
        Verdict::Provisioned | Verdict::Decommissioning | Verdict::Deleted => {
            emulator
                .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::S3(0), 1)]))
                .await;
            deliver_orders(emulator);
            if matches!(verdict, Verdict::Decommissioning) {
                request_decommission(emulator, &outpost_id).await;
            }
            if matches!(verdict, Verdict::Deleted) {
                let reply = emulator
                    .outposts(REGION, "DELETE", &format!("/outposts/{outpost_id}"), None)
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "DeleteOutpost must remove the Outpost, got {} {}",
                    reply.status, reply.body
                );
            }
        }
    }
    outpost_id
}

/// The rule: for an `OutpostId` the AWS Outposts control plane holds an Outpost under,
/// that record is the whole answer — a declaration naming the same id changes neither
/// what the inventory reports for it nor whether a reference to it resolves — and a
/// declaration is the answer only for an id that control plane holds none under.
#[tokio::test]
async fn the_control_plane_answers_for_every_outpost_id_it_holds() {
    let catalog = Catalog::discover(&Emulator::new()).await;

    let rows = [
        Row {
            case: "active, with S3 capacity ordered",
            verdict: Verdict::Provisioned,
            answer: Answer::ControlPlane,
        },
        Row {
            case: "S3 capacity ordered, still provisioning",
            verdict: Verdict::StillProvisioning,
            answer: Answer::Absent,
        },
        Row {
            case: "active, a decommission requested",
            verdict: Verdict::Decommissioning,
            answer: Answer::Absent,
        },
        Row {
            case: "active, its only S3-capable order cancelled",
            verdict: Verdict::OrderCancelled,
            answer: Answer::Absent,
        },
        Row {
            case: "active, no order bought S3 storage capacity",
            verdict: Verdict::NoS3Capacity,
            answer: Answer::Absent,
        },
        Row {
            case: "held and deleted again",
            verdict: Verdict::Deleted,
            answer: Answer::Declared,
        },
        Row {
            case: "an id the control plane never held",
            verdict: Verdict::NeverHeld,
            answer: Answer::Declared,
        },
    ];

    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        let outpost_id = at_verdict(&emulator, &catalog, row.verdict).await;

        // What the control plane alone answers for the id, before any declaration.
        let before = reported_outpost(&emulator, &outpost_id).await;
        let expected_before = match row.answer {
            Answer::ControlPlane => true,
            Answer::Declared | Answer::Absent => false,
        };
        assert_eq!(
            before.is_some(),
            expected_before,
            "case {case:?}: the control plane's own verdict on {outpost_id} decides the \
             inventory before any declaration, got {before:?}"
        );

        declare_outpost(&emulator, &outpost_id);
        let after = reported_outpost(&emulator, &outpost_id).await;

        match row.answer {
            Answer::ControlPlane => assert_eq!(
                after, before,
                "case {case:?}: the declaration is invisible for an id the control plane \
                 holds, expected {before:?}, got {after:?}"
            ),
            Answer::Absent => assert_eq!(
                after, None,
                "case {case:?}: the control plane's negative verdict on an id it holds is \
                 the answer, expected no Outpost, got {after:?}"
            ),
            Answer::Declared => {
                let reported = after
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: the declared Outpost is reported"));
                assert_eq!(
                    (
                        string_at(&reported, "OwnerId"),
                        reported["CapacityInBytes"].as_i64()
                    ),
                    (SHARING_ACCOUNT_ID.to_string(), Some(DECLARED_CAPACITY)),
                    "case {case:?}: an id the control plane holds no Outpost under is the \
                     declaration's to answer for, got {reported}"
                );
            }
        }

        // And every reference to the id resolves exactly as the inventory reports it:
        // an endpoint is minted on an Outpost that is in it and refused on one that is
        // not, with this service's one not-found text.
        let created = emulator.create_endpoint_in(REGION, &outpost_id, 1).await;
        if after.is_some() {
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateEndpoint on an Outpost the inventory reports is \
                 served, got {} {}",
                created.status, created.body
            );
        } else {
            assert_eq!(
                (created.status, created.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: CreateEndpoint on an Outpost no inventory reports is the \
                 modelled 404, got {} {} {}",
                created.status,
                created.error_code(),
                created.body
            );
            assert_eq!(
                created.message(),
                OUTPOST_MISSING,
                "case {case:?}: CreateEndpoint carries this service's not-found text, got {}",
                created.body
            );
            let shared = emulator.shared_endpoints_in(REGION, &outpost_id).await;
            assert_eq!(
                (shared.status, shared.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: ListSharedEndpoints on it is the modelled 404, got {} {} {}",
                shared.status,
                shared.error_code(),
                shared.body
            );
        }
    }
}

/// The propagation that follows: an Outpost the control plane's verdict takes out of
/// the inventory takes the endpoints stored on it with it, even while a declaration
/// names the same id — and leaves every other Outpost's endpoints where they were.
#[tokio::test]
async fn a_declared_outpost_the_control_plane_drops_releases_its_endpoints() {
    let catalog = Catalog::discover(&Emulator::new()).await;

    let rows: [(&str, Verdict); 2] = [
        ("a decommission requested", Verdict::Decommissioning),
        ("its only S3-capable order cancelled", Verdict::OrderCancelled),
    ];

    for (case, verdict) in rows {
        let emulator = Emulator::new();

        // One Outpost that stays in the inventory throughout, carrying an endpoint of
        // its own.
        let survivor = emulator.an_outpost_in(REGION, "survivor").await;
        emulator
            .an_order_in(REGION, &survivor, &catalog.line_items(&[(Item::S3(0), 1)]))
            .await;

        // The shadowed one: in the inventory as the control plane's own record, and
        // declared by this service besides.
        let outpost_id = emulator.an_outpost_in(REGION, "shadowed").await;
        emulator
            .an_order_in(
                REGION,
                &outpost_id,
                &catalog.line_items(&[(Item::EbsOnly, 1)]),
            )
            .await;
        deliver_orders(&emulator);
        let order_id = emulator
            .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::S3(1), 2)]))
            .await;
        declare_outpost(&emulator, &outpost_id);
        assert!(
            reported_outpost(&emulator, &outpost_id).await.is_some(),
            "case {case:?}: the shadowed Outpost must be in the inventory before it leaves"
        );

        let placed = [
            place_endpoint(
                &emulator,
                REGION,
                &outpost_id,
                "subnet-0a1b2c3d4e5f60718",
                "sg-0a1b2c3d4e5f60718",
            )
            .await,
            place_endpoint(
                &emulator,
                REGION,
                &outpost_id,
                "subnet-0a1b2c3d4e5f60719",
                "sg-0a1b2c3d4e5f60719",
            )
            .await,
        ];
        let kept = place_endpoint(
            &emulator,
            REGION,
            &survivor,
            SURVIVOR_SUBNET,
            SURVIVOR_SECURITY_GROUP,
        )
        .await;

        match verdict {
            Verdict::Decommissioning => {
                // A decommission is recorded only once nothing is outstanding on the
                // Outpost, so the order placed above delivers its hardware first.
                deliver_orders(&emulator);
                request_decommission(&emulator, &outpost_id).await;
            }
            Verdict::OrderCancelled => emulator.cancel_order_in(REGION, &order_id).await,
            other => unreachable!("this table drives {other:?} through no route"),
        }

        assert_eq!(
            reported_outpost(&emulator, &outpost_id).await,
            None,
            "case {case:?}: the control plane's negative verdict takes the declared \
             Outpost out of the inventory"
        );

        // The endpoints it carried are released with it, together with everything each
        // record held, and the survivor's is untouched. Asserted after a call that
        // reports Outposts alone, so the release is owed by the departure rather than by
        // whichever operation happens to read the endpoints.
        let (keys, subnets) = stored(&emulator);
        assert_eq!(
            keys,
            BTreeSet::from([(kept.outpost_id.clone(), kept.endpoint_id.clone())]),
            "case {case:?}: every endpoint of the departed Outpost is released and the \
             survivor's is kept"
        );
        assert_eq!(
            subnets,
            BTreeSet::from([kept.subnet_id.clone()]),
            "case {case:?}: a subnet keeps its VPC mapping only while a reachable endpoint \
             sits in it"
        );
        let cidrs = committed_cidrs(&emulator);
        for endpoint in &placed {
            assert!(
                !cidrs.contains(&endpoint.cidr_block),
                "case {case:?}: the {} committed by the released endpoint is free again, got \
                 {cidrs:?}",
                endpoint.cidr_block
            );
        }
        assert_eq!(
            reported_arns(&emulator, REGION).await,
            BTreeSet::from([kept.arn.clone()]),
            "case {case:?}: ListEndpoints reports the survivor's endpoint alone"
        );

        // And every reference under the departed Outpost is the modelled 404.
        for endpoint in &placed {
            let deleted = emulator
                .delete_endpoint_in(REGION, &outpost_id, &endpoint.endpoint_id)
                .await;
            assert_eq!(
                (deleted.status, deleted.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: DeleteEndpoint of {} is the modelled 404, got {} {} {}",
                endpoint.endpoint_id,
                deleted.status,
                deleted.error_code(),
                deleted.body
            );
            assert_eq!(
                deleted.message(),
                OUTPOST_MISSING,
                "case {case:?}: DeleteEndpoint carries this service's not-found text, got {}",
                deleted.body
            );
        }
    }
}
