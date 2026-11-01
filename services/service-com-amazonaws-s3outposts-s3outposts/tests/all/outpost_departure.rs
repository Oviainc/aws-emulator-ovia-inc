// SPDX-License-Identifier: CC0-1.0

//! What happens to the endpoints ON an Outpost when that Outpost leaves this service's
//! live inventory — driven end to end over HTTP across BOTH emulated services.
//!
//! The rule under test: an endpoint exists only ON the Outpost it was created for —
//! `CreateEndpoint` "creates an endpoint and associates it with the specified Outpost",
//! the pair `(OutpostId, EndpointId)` is its name, and it takes that Outpost's home
//! Region — so an Outpost this service's inventory no longer holds carries no endpoint
//! here either: every endpoint stored on it is released, with everything the record held
//! (the `/26` it committed and the VPC mapping of its subnet), instead of being left
//! behind as a record no operation can report, resolve or remove. The same rule the AWS
//! Outposts control plane itself follows for the resources that exist only as resources
//! OF an Outpost.
//!
//! The Outposts are that co-hosted control plane's own, resolved afresh on every call, so
//! the first table varies the ROUTE an Outpost leaves the inventory by — its only
//! S3-capable order cancelled, a decommission requested, the Outpost deleted outright, the
//! sharing of another account's Outpost withdrawn — beside how many endpoints it carried
//! and whether an endpoint of a surviving Outpost sits in the same subnet.
//!
//! The second case pins the other direction: the inventory is not Region-scoped, so an
//! Outpost reached through another Region's endpoint is still one this service holds, and
//! a call served in one Region releases nothing of another's.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_s3outposts_s3outposts::state::OutpostRecord;

use crate::control_plane_inventory::{
    deliver_orders, reported_ids, string_at, Catalog, Emulator, Item, REGION, SHARING_ACCOUNT_ID,
};
use crate::endpoint_lifecycle::OUTPOST_MISSING;

/// The Outpost the withdrawal row has another account share with the caller: an id of
/// `OutpostId`'s `op-[a-f0-9]{17}` branch.
const SHARED_OUTPOST_ID: &str = "op-2c3d4e5f607182930";

/// The S3 capacity that shared Outpost is declared with — 24 TiB in bytes, the unit
/// `Outpost$CapacityInBytes` ("the Amazon S3 capacity of the outpost in bytes") is stated
/// in.
const SHARED_CAPACITY: i64 = 24 * 1024 * 1024 * 1024 * 1024;

/// The subnet and security group the surviving Outpost's own endpoint sits in, where a row
/// does not have it share the departing Outpost's subnet.
const SURVIVOR_SUBNET: &str = "subnet-0f1e2d3c4b5a69780";
const SURVIVOR_SECURITY_GROUP: &str = "sg-0f1e2d3c4b5a69780";

/// How an Outpost leaves this service's live inventory. Each is a route a client can
/// drive through the co-hosted AWS Outposts control plane, except the last, which is the
/// operator-declared sharing this emulator's inventory merges in.
#[derive(Debug, Clone, Copy)]
enum Departure {
    /// The only order that bought S3 storage capacity for it is cancelled
    /// (`outposts:CancelOrder`), so no order on it buys any.
    OrderCancelled,
    /// A decommission of it is requested (`outposts:StartOutpostDecommission`).
    DecommissionRequested,
    /// The Outpost itself is deleted (`outposts:DeleteOutpost`).
    OutpostDeleted,
    /// Another account stops sharing its Outpost with this one.
    SharingWithdrawn,
}

/// One row: the route the Outpost leaves by, the `(subnet, security group)` of each
/// endpoint placed on it, and whether the surviving Outpost's own endpoint sits in the
/// FIRST of those subnets.
struct Row {
    case: &'static str,
    departure: Departure,
    placed: &'static [(&'static str, &'static str)],
    shares_subnet: bool,
}

/// An endpoint a case placed, as the service reported it.
pub(crate) struct Placed {
    pub(crate) arn: String,
    pub(crate) endpoint_id: String,
    pub(crate) outpost_id: String,
    pub(crate) subnet_id: String,
    pub(crate) cidr_block: String,
}

/// Every endpoint `ListEndpoints` reports in `region`.
pub(crate) async fn listed_endpoints(emulator: &Emulator, region: &str) -> Vec<Value> {
    let reply = emulator.endpoints_in(region).await;
    assert_eq!(
        reply.status, 200,
        "ListEndpoints in {region}: expected HTTP 200, got {} {}",
        reply.status, reply.body
    );
    reply.body["Endpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("ListEndpoints must carry Endpoints: {}", reply.body))
        .clone()
}

/// The `EndpointArn`s `ListEndpoints` reports in `region`.
pub(crate) async fn reported_arns(emulator: &Emulator, region: &str) -> BTreeSet<String> {
    listed_endpoints(emulator, region)
        .await
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect()
}

/// `CreateEndpoint` on `outpost_id` in `region`, as the record the service minted for it
/// — its id read off the ARN it answered and its CIDR read back through `ListEndpoints`,
/// so nothing here is assumed about either.
pub(crate) async fn place_endpoint(
    emulator: &Emulator,
    region: &str,
    outpost_id: &str,
    subnet_id: &str,
    security_group_id: &str,
) -> Placed {
    let reply = emulator
        .s3(
            region,
            "POST",
            "/S3Outposts/CreateEndpoint",
            Some(json!({
                "OutpostId": outpost_id,
                "SubnetId": subnet_id,
                "SecurityGroupId": security_group_id,
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {outpost_id}/{subnet_id}: CreateEndpoint on an Outpost in the inventory is \
         served, got {} {}",
        reply.status, reply.body
    );
    let arn = string_at(&reply.body, "EndpointArn");
    let endpoint_id = arn
        .rsplit('/')
        .next()
        .expect("an EndpointArn ends in the endpoint id")
        .to_string();
    let reported = listed_endpoints(emulator, region)
        .await
        .into_iter()
        .find(|endpoint| string_at(endpoint, "EndpointArn") == arn)
        .unwrap_or_else(|| panic!("case {arn}: the created endpoint must be reported"));
    Placed {
        arn,
        endpoint_id,
        outpost_id: outpost_id.to_string(),
        subnet_id: subnet_id.to_string(),
        cidr_block: string_at(&reported, "CidrBlock"),
    }
}

/// The `(OutpostId, EndpointId)` keys this service's store holds, beside the subnets it
/// still knows a VPC for.
///
/// These two are what the crate's own bounded invariants are about, and no response
/// reports the absence of a record, so this case reads the store for them. A subnet
/// mapping is keyed by the Region the subnet belongs to as well; these rows all place
/// their endpoints in one Region, so the subnet half is what they look at.
pub(crate) fn stored(emulator: &Emulator) -> (BTreeSet<(String, String)>, BTreeSet<String>) {
    let store = emulator.s3_store();
    let store = store.lock();
    (
        store.endpoints.keys().cloned().collect(),
        store
            .subnet_vpcs
            .keys()
            .map(|(_, subnet_id)| subnet_id.clone())
            .collect(),
    )
}

/// The CIDR blocks the stored endpoints still commit — the set the next `/26` is chosen
/// outside of, so a block absent from it is one a later endpoint can be committed.
pub(crate) fn committed_cidrs(emulator: &Emulator) -> BTreeSet<String> {
    let store = emulator.s3_store();
    let store = store.lock();
    store
        .endpoints
        .values()
        .map(|endpoint| endpoint.cidr_block.clone())
        .collect()
}

/// An Outpost that IS in this service's inventory, stood up by the route `departure` will
/// take it out of again: its id, and the order a cancellation row has to cancel.
async fn an_outpost_in_the_inventory(
    emulator: &Emulator,
    catalog: &Catalog,
    departure: Departure,
) -> (String, Option<String>) {
    if matches!(departure, Departure::SharingWithdrawn) {
        // The orders already placed on the account's OWN Outposts still have to deliver
        // the hardware that makes them active, whichever route this row takes.
        deliver_orders(emulator);
        // No AWS API of this emulator shares an Outpost — Resource Access Manager is not
        // emulated and the control plane serves one account — so the sharing is declared
        // through the seam the operator's own setting reaches.
        let store = emulator.s3_store();
        let mut store = store.lock();
        store.provision_outpost(
            OutpostRecord::provisioned(
                SHARED_OUTPOST_ID,
                SHARING_ACCOUNT_ID,
                SHARED_CAPACITY,
                REGION,
            )
            .expect("the shared Outpost is a provisionable record"),
        );
        return (SHARED_OUTPOST_ID.to_string(), None);
    }

    let outpost_id = emulator.an_outpost_in(REGION, "departing").await;
    if matches!(departure, Departure::OrderCancelled) {
        // An order is cancellable only while the service still has work to carry out on
        // it, so the order that buys the S3 capacity is placed AFTER the hardware this
        // Outpost is installed with has been delivered: the install is what makes the
        // Outpost active, and the S3 capacity is what puts it in this inventory.
        emulator
            .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::EbsOnly, 1)]))
            .await;
        deliver_orders(emulator);
        let order_id = emulator
            .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::S3(0), 2)]))
            .await;
        return (outpost_id, Some(order_id));
    }

    emulator
        .an_order_in(REGION, &outpost_id, &catalog.line_items(&[(Item::S3(1), 1)]))
        .await;
    deliver_orders(emulator);
    (outpost_id, None)
}

/// Take `outpost_id` out of this service's inventory by `departure`'s route, over the
/// wire wherever an AWS API models the step.
async fn leave_the_inventory(
    emulator: &Emulator,
    departure: Departure,
    outpost_id: &str,
    order_id: Option<&str>,
) {
    match departure {
        Departure::OrderCancelled => {
            let order_id = order_id.expect("a cancellation row carries the order it cancels");
            emulator.cancel_order_in(REGION, order_id).await;
        }
        Departure::DecommissionRequested => {
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
        Departure::OutpostDeleted => {
            let reply = emulator
                .outposts(REGION, "DELETE", &format!("/outposts/{outpost_id}"), None)
                .await;
            assert_eq!(
                reply.status, 200,
                "DeleteOutpost must remove the Outpost, got {} {}",
                reply.status, reply.body
            );
        }
        Departure::SharingWithdrawn => {
            let store = emulator.s3_store();
            let mut store = store.lock();
            assert!(
                store.outposts.remove(outpost_id).is_some(),
                "the shared Outpost must have been declared before it is withdrawn"
            );
        }
    }
}

/// The rule: an Outpost leaving this service's live inventory takes the endpoints stored
/// on it with it, whichever route it leaves by, and leaves the endpoints of every other
/// Outpost exactly where they were.
#[tokio::test]
async fn an_outpost_leaving_the_inventory_releases_the_endpoints_on_it() {
    let catalog = Catalog::discover(&Emulator::new()).await;

    let rows = [
        Row {
            case: "the only S3-capable order cancelled, one endpoint on the Outpost",
            departure: Departure::OrderCancelled,
            placed: &[("subnet-0a1b2c3d4e5f60718", "sg-0a1b2c3d4e5f60718")],
            shares_subnet: false,
        },
        Row {
            case: "a decommission requested, one endpoint sharing its subnet with the survivor",
            departure: Departure::DecommissionRequested,
            placed: &[("subnet-0b1c2d3e", "sg-0b1c2d3e")],
            shares_subnet: true,
        },
        Row {
            case: "the Outpost deleted, three endpoints across two subnets",
            departure: Departure::OutpostDeleted,
            placed: &[
                ("subnet-0c1d2e3f405162738", "sg-0c1d2e3f405162738"),
                ("subnet-0c1d2e3f405162738", "sg-0c1d2e3f405162739"),
                ("subnet-0d1e2f30", "sg-0d1e2f30"),
            ],
            shares_subnet: false,
        },
        Row {
            case: "another account's sharing withdrawn, two endpoints on the shared Outpost",
            departure: Departure::SharingWithdrawn,
            placed: &[
                ("subnet-0e1f20314253647a0", "sg-0e1f20314253647a0"),
                ("subnet-0e1f20314253647a1", "sg-0e1f20314253647a1"),
            ],
            shares_subnet: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();

        // One Outpost that stays in the inventory throughout, carrying an endpoint of its
        // own: what is released is the departing Outpost's endpoints and nothing else.
        let survivor = emulator.an_outpost_in(REGION, "survivor").await;
        emulator
            .an_order_in(
                REGION,
                &survivor,
                &catalog.line_items(&[(Item::S3(0), 1)]),
            )
            .await;

        let (departing, order_id) =
            an_outpost_in_the_inventory(&emulator, &catalog, row.departure).await;
        assert_eq!(
            reported_ids(&emulator.inventory(REGION).await)
                .into_iter()
                .collect::<BTreeSet<String>>(),
            BTreeSet::from([survivor.clone(), departing.clone()]),
            "case {case:?}: both Outposts must be in the inventory before the departure"
        );

        let mut placed = Vec::new();
        for (subnet_id, security_group_id) in row.placed {
            placed.push(
                place_endpoint(&emulator, REGION, &departing, subnet_id, security_group_id).await,
            );
        }
        let survivor_subnet = match row.shares_subnet {
            true => row.placed[0].0,
            false => SURVIVOR_SUBNET,
        };
        let kept = place_endpoint(
            &emulator,
            REGION,
            &survivor,
            survivor_subnet,
            SURVIVOR_SECURITY_GROUP,
        )
        .await;

        let before = reported_arns(&emulator, REGION).await;
        for endpoint in placed.iter().chain(std::iter::once(&kept)) {
            assert!(
                before.contains(&endpoint.arn),
                "case {case:?}: {} must be reported before the departure, got {before:?}",
                endpoint.arn
            );
        }

        leave_the_inventory(&emulator, row.departure, &departing, order_id.as_deref()).await;

        // The Outpost is gone from the inventory, and the survivor is not.
        assert_eq!(
            reported_ids(&emulator.inventory(REGION).await),
            vec![survivor.clone()],
            "case {case:?}: only the departing Outpost leaves the inventory"
        );

        // And the endpoints it carried are released with it, together with everything each
        // record held. Asserted after a call that reports Outposts alone, so the release
        // is owed by the departure and not by whichever operation happens to read the
        // endpoints.
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
        assert!(
            cidrs.contains(&kept.cidr_block),
            "case {case:?}: the survivor's endpoint still commits {}, got {cidrs:?}",
            kept.cidr_block
        );

        // Nothing reports them, and every reference under the departed Outpost is the
        // modelled 404 carrying this service's one not-found text.
        assert_eq!(
            reported_arns(&emulator, REGION).await,
            BTreeSet::from([kept.arn.clone()]),
            "case {case:?}: ListEndpoints reports the survivor's endpoint alone"
        );
        let shared = emulator.shared_endpoints_in(REGION, &departing).await;
        assert_eq!(
            (shared.status, shared.error_code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: ListSharedEndpoints on the departed Outpost is the modelled 404, \
             got {} {} {}",
            shared.status,
            shared.error_code(),
            shared.body
        );
        assert_eq!(
            shared.message(),
            OUTPOST_MISSING,
            "case {case:?}: ListSharedEndpoints carries this service's not-found text, got {}",
            shared.body
        );
        for endpoint in &placed {
            let deleted = emulator
                .delete_endpoint_in(REGION, &departing, &endpoint.endpoint_id)
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

        // The survivor's endpoint is still deletable, so the release left the records it
        // did keep reachable.
        let deleted = emulator
            .delete_endpoint_in(REGION, &kept.outpost_id, &kept.endpoint_id)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the surviving endpoint is still the caller's to delete, got {} {}",
            deleted.status, deleted.body
        );
    }
}

/// The other direction: the inventory this service resolves is not Region-scoped, so an
/// Outpost installed in another Region is still one it holds — and a call served in one
/// Region releases no endpoint of another Region's Outposts, even though no listing of
/// the Region it was served in reports them.
#[tokio::test]
async fn a_call_in_one_region_releases_no_endpoint_of_another_region() {
    let catalog = Catalog::discover(&Emulator::new()).await;
    let emulator = Emulator::new();

    // One Outpost per Region, each ordered S3 capacity through the `outposts` API.
    let elsewhere = "eu-west-1";
    let mut outposts = Vec::new();
    for (index, region) in [REGION, elsewhere].into_iter().enumerate() {
        let outpost_id = emulator.an_outpost_in(region, "regional").await;
        emulator
            .an_order_in(
                region,
                &outpost_id,
                &catalog.line_items(&[(Item::S3(index), 1)]),
            )
            .await;
        outposts.push((region, outpost_id));
    }
    deliver_orders(&emulator);

    let mut placed = Vec::new();
    for (index, (region, outpost_id)) in outposts.iter().enumerate() {
        placed.push(
            place_endpoint(
                &emulator,
                region,
                outpost_id,
                &format!("subnet-0a1b2c3d4e5f6071{index}"),
                &format!("sg-0a1b2c3d4e5f6071{index}"),
            )
            .await,
        );
    }

    // Every door into this service, driven in the FIRST Region only.
    let (home, away) = (&placed[0], &placed[1]);
    emulator.inventory(REGION).await;
    emulator.endpoints_in(REGION).await;
    emulator.shared_endpoints_in(REGION, &home.outpost_id).await;
    place_endpoint(
        &emulator,
        REGION,
        &home.outpost_id,
        "subnet-0909090909090909a",
        "sg-0909090909090909a",
    )
    .await;
    emulator
        .delete_endpoint_in(REGION, &home.outpost_id, &home.endpoint_id)
        .await;

    // The other Region's endpoint is untouched: still stored, still holding its subnet
    // mapping, and still reported by its own Region's listing.
    let (keys, subnets) = stored(&emulator);
    assert!(
        keys.contains(&(away.outpost_id.clone(), away.endpoint_id.clone())),
        "an endpoint of {elsewhere} survives every call served in {REGION}, got {keys:?}"
    );
    assert!(
        subnets.contains(&away.subnet_id),
        "its subnet keeps the VPC mapping, got {subnets:?}"
    );
    assert_eq!(
        reported_arns(&emulator, elsewhere).await,
        BTreeSet::from([away.arn.clone()]),
        "and {elsewhere} still reports it"
    );
    assert!(
        committed_cidrs(&emulator).contains(&away.cidr_block),
        "and it still commits {}",
        away.cidr_block
    );
}
