// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a deleted Outpost takes with it, driven over the wire
//! through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses) — `MultiServiceRouter::dispatch` with a restJson1
//! request, exactly as a signed `aws outposts` call arrives.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! * Removing a resource removes with it every record this store keeps that exists
//!   only as part of it — a record whose whole subject is that resource or something
//!   inside it, reachable only once that resource has been resolved. For an Outpost
//!   those are the capacity tasks of it ([`crate::capacity_task_lifecycle`] states
//!   that half) and the installation connections started for the hardware assets it
//!   held: an `AssetId` names one unit of hardware of exactly ONE Outpost (the
//!   modelled route is `/outposts/{OutpostIdentifier}/assets`), so a connection
//!   started for one of its assets installs hardware that goes away with the record.
//!   Afterwards no read answers for the vanished Outpost's connections, and the
//!   slots they held are back in the store's own entry ceiling, which
//!   `StartConnection` refuses at.
//!
//! What the rule does NOT reach, and every row checks it does not: a connection of
//! ANOTHER Outpost of the same region, and a connection of another REGION's Outpost
//! whose hardware carries the same `AssetId` spelling — a resource is part of its own
//! region's inventory and of no other's, so one region's delete is not another
//! region's.
//!
//! Where the hardware comes from: an asset is a PHYSICAL unit that arrives with the
//! fulfilment of an order placed on the Outpost, over spans far longer than a case
//! can wait for, so each row RECORDS its hardware on the stored Outpost through this
//! crate's store — exactly as [`crate::outpost_assets`] and
//! [`crate::outpost_server_connection`] do, whose recording and request helpers this
//! suite reuses.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{store_full, MAX_RESOURCES};

use crate::outpost_assets::{asset, record};
use crate::outpost_server_connection::{
    client_key, outpost_holding_all, route, start_request, START_ROUTE,
};
use crate::site_and_outpost_lifecycle::{segment, text, Outposts, HOME_REGION};

/// A region that is NOT the one the rows work in ([`HOME_REGION`]): an Outpost of it
/// holds hardware of its own, which a delete addressed to the home region cannot
/// reach.
const OTHER_REGION: &str = "us-west-2";

/// The client keys the starts below carry, which differ in which characters of the
/// modelled `WireGuardPublicKey` alphabet fill them — letters of either case and a
/// digit. Nothing in the rule depends on the key; the rows vary it because the
/// requests a real caller makes do.
const KEY_FILLERS: [&str; 3] = ["B", "c", "7"];

/// The route `DeleteOutpost` and `GetOutpost` are bound to, for one identifier:
/// `DELETE`/`GET /outposts/{OutpostId}`, from
/// `smithy_query(query="operation",operation="DeleteOutpost,GetOutpost")`.
fn outpost_route(identifier: &str) -> String {
    format!("/outposts/{}", segment(identifier))
}

/// One Outpost of `region` on a site of its own, holding every asset in `asset_ids`
/// as its installed hardware, as its `OutpostId`.
///
/// The counterpart of [`outpost_holding_all`] for a case that works in a SECOND
/// region: an Outpost can only be placed on a site of its own region, so the site is
/// created there too.
async fn outpost_holding_all_in(
    outposts: &Outposts,
    region: &str,
    name: &str,
    asset_ids: &[&str],
) -> String {
    let site_id = outposts.site_id_in(region, &format!("{name}-site")).await;
    let outpost = outposts
        .create_outpost_in(region, &json!({ "Name": name, "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    let hardware: Vec<_> = asset_ids
        .iter()
        .map(|asset_id| asset(asset_id, "rack-1", Some("COMPUTE"), None, None))
        .collect();
    record(outposts, &outpost_id, &hardware, name);
    outpost_id
}

/// One row: the hardware of the Outpost the delete removes and of the one that
/// stays, each asset with how many connections are started for it.
struct Case {
    case: &'static str,
    /// The assets of the Outpost this row deletes, as `(AssetId, connections
    /// started for it)`.
    doomed: &'static [(&'static str, usize)],
    /// The same for the Outpost of the same region that the row leaves alone.
    kept: &'static [(&'static str, usize)],
}

/// The rows, differing along everything the rule generalises over: how many assets
/// the deleted Outpost holds, how many connections each asset carries (none, one,
/// several), and whether the Outpost that stays carries any at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one asset carrying one connection",
            doomed: &[("asset_one", 1)],
            kept: &[("asset_kept_one", 1)],
        },
        Case {
            case: "three assets, each carrying one connection",
            doomed: &[("asset_a", 1), ("asset_b", 1), ("asset_c", 1)],
            kept: &[("asset_kept_two", 2)],
        },
        Case {
            case: "one asset carrying several connections of its own",
            doomed: &[("asset_restarted", 3)],
            kept: &[("asset_kept_three", 1)],
        },
        Case {
            case: "assets no connection was ever started for",
            doomed: &[("asset_idle_one", 0), ("asset_idle_two", 0)],
            kept: &[("asset_kept_four", 1)],
        },
        Case {
            case: "assets of which only some carry a connection",
            doomed: &[("asset_mixed_one", 2), ("asset_mixed_two", 0)],
            kept: &[("asset_kept_five", 0)],
        },
    ]
}

/// Start one connection per unit named in `hardware`, in `region`, answering every
/// `ConnectionId` minted in the order it was minted.
///
/// Each start names a client key of its own (the rows vary the alphabet the key
/// carries, which [`crate::outpost_server_connection`] states the rule about), a
/// dongle serial on every second one and a device index inside the modelled range
/// — nothing the cascade depends on, so the requests differ freely.
async fn start_connections(
    outposts: &Outposts,
    region: &str,
    hardware: &[(&str, usize)],
    case: &str,
) -> Vec<String> {
    let mut started = Vec::new();
    for (asset_id, count) in hardware {
        for nth in 0..*count {
            let serial = (nth % 2 == 1).then(|| format!("DNG000000{nth}"));
            let payload = start_request(
                asset_id,
                &client_key(KEY_FILLERS[nth % KEY_FILLERS.len()]),
                serial.as_deref(),
                Some(i32::try_from(nth % 2).expect("the device index fits the modelled range")),
            );
            let reply = outposts
                .ok_in(region, "POST", START_ROUTE, Some(&payload))
                .await;
            let connection_id = text(&reply, "ConnectionId");
            assert!(
                !connection_id.is_empty(),
                "case {case:?}: a start of asset {asset_id:?} must mint a ConnectionId, \
                 got {reply}"
            );
            started.push(connection_id);
        }
    }
    started
}

/// The rule: a delete takes the connections of the Outpost's own hardware with it,
/// and leaves every other Outpost's — of this region and of another — exactly as
/// they were.
#[tokio::test]
async fn deleting_an_outpost_takes_the_connections_of_its_assets_with_it() {
    for Case { case, doomed, kept } in cases() {
        // A store per row, so what a row observes is its own records and nothing a
        // previous row left behind.
        let outposts = Outposts::new();
        let doomed_assets: Vec<&str> = doomed.iter().map(|(asset_id, _)| *asset_id).collect();
        let kept_assets: Vec<&str> = kept.iter().map(|(asset_id, _)| *asset_id).collect();

        let doomed_id = outpost_holding_all(&outposts, "doomed", &doomed_assets).await;
        let kept_id = outpost_holding_all(&outposts, "kept", &kept_assets).await;
        // Another region's Outpost, holding hardware spelled exactly like the doomed
        // Outpost's: its connections are no business of this region's delete.
        outpost_holding_all_in(&outposts, OTHER_REGION, "elsewhere", &doomed_assets).await;

        let doomed_connections = start_connections(&outposts, HOME_REGION, doomed, case).await;
        let kept_connections = start_connections(&outposts, HOME_REGION, kept, case).await;
        let elsewhere: &[(&str, usize)] = &[(doomed_assets[0], 1)];
        let elsewhere_connections =
            start_connections(&outposts, OTHER_REGION, elsewhere, case).await;

        // What every surviving connection answers BEFORE the delete, so the check
        // after it is that nothing changed rather than only that something answered.
        let mut survivors: Vec<(&str, String, Value)> = Vec::new();
        for connection_id in &kept_connections {
            let body = outposts.ok("GET", &route(connection_id), None).await;
            survivors.push((HOME_REGION, connection_id.clone(), body));
        }
        for connection_id in &elsewhere_connections {
            let body = outposts
                .ok_in(OTHER_REGION, "GET", &route(connection_id), None)
                .await;
            survivors.push((OTHER_REGION, connection_id.clone(), body));
        }

        outposts
            .ok("DELETE", &outpost_route(&doomed_id), None)
            .await;

        // The Outpost itself is gone — the premise of the rule.
        let read = outposts.call("GET", &outpost_route(&doomed_id), None).await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: the deleted Outpost must be gone, got {} {} {}",
            read.status,
            read.error_code,
            read.body
        );

        // And so is every connection started for the hardware it held: the read is
        // this service's 404, worded as its identifier echo.
        for connection_id in &doomed_connections {
            let reply = outposts.call("GET", &route(connection_id), None).await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: a connection of the deleted Outpost's hardware must be \
                 gone, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            let expected = format!("connectionId={connection_id}");
            assert_eq!(
                text(&reply.body, "Message"),
                expected,
                "case {case:?}: expected $.Message {expected:?}, got {}",
                reply.body
            );
        }

        // Nothing else moved: the Outpost that stays is still readable, and every
        // connection of it and of the other region's Outpost reads back unchanged.
        let kept_read = outposts.get_outpost(&kept_id).await;
        assert_eq!(
            text(&kept_read, "OutpostId"),
            kept_id,
            "case {case:?}: deleting one Outpost leaves every other readable, got \
             {kept_read}"
        );
        for (region, connection_id, before) in &survivors {
            let after = outposts
                .ok_in(region, "GET", &route(connection_id), None)
                .await;
            assert_eq!(
                &after, before,
                "case {case:?}: the connection {connection_id:?} of {region} must read back \
                 unchanged, got {after}"
            );
        }

        // The store holds exactly the survivors: the entries the delete reclaimed are
        // out of the map the ceiling counts, not merely unreachable.
        let held = outposts.store().lock().connections.len();
        assert_eq!(
            held,
            survivors.len(),
            "case {case:?}: expected {} connections left in the store, got {held}",
            survivors.len()
        );
    }
}

/// The second half of the rule: the slots a deleted Outpost's connections held come
/// BACK to the store, so a delete gives the capacity to start connections again.
///
/// The ceiling is `MAX_RESOURCES` entries, which no sequence of calls could fill
/// inside a test, so this case fills the store directly — as
/// [`crate::capacity_task_lifecycle`] and [`crate::quote_store_bound`] fill theirs —
/// and then drives the refusal, the delete and the next start over the wire.
#[tokio::test]
async fn the_slots_a_deleted_outposts_connections_held_return_to_the_store() {
    let outposts = Outposts::new();
    let filling_asset = "asset_at_the_ceiling";
    let later_asset = "asset_started_after_the_delete";
    let doomed = outpost_holding_all(&outposts, "ceiling-doomed", &[filling_asset]).await;
    outpost_holding_all(&outposts, "ceiling-later", &[later_asset]).await;
    let key = client_key("D");

    // One real connection on the doomed Outpost's hardware, then the store filled to
    // the ceiling with entries naming that same hardware.
    let started = outposts
        .ok(
            "POST",
            START_ROUTE,
            Some(&start_request(filling_asset, &key, None, Some(0))),
        )
        .await;
    let first = text(&started, "ConnectionId");
    fill_to_ceiling(&outposts, &first);
    let at_ceiling = outposts.store().lock().connections.len();
    assert_eq!(
        at_ceiling, MAX_RESOURCES,
        "the store must be filled to its ceiling: expected {MAX_RESOURCES} connections \
         held, got {at_ceiling}"
    );

    // At the ceiling a start is refused — on hardware of ANOTHER Outpost, so what
    // refuses it is the store bound and nothing about the asset.
    let refused = outposts
        .call(
            "POST",
            START_ROUTE,
            Some(&start_request(later_asset, &key, None, Some(0))),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (500, "InternalServerException"),
        "at the ceiling a start must be refused, got {} {} {}",
        refused.status,
        refused.error_code,
        refused.body
    );
    assert_eq!(
        text(&refused.body, "Message"),
        store_full("connections"),
        "at the ceiling the refusal states the store bound, got {}",
        refused.body
    );

    // Deleting the Outpost whose hardware every stored connection names empties the
    // store of them.
    outposts.ok("DELETE", &outpost_route(&doomed), None).await;
    let after_delete = outposts.store().lock().connections.len();
    assert_eq!(
        after_delete, 0,
        "the delete must reclaim every connection of its hardware: expected 0 held, got \
         {after_delete}"
    );

    // So the next start is accepted again, which it could not be had the entries
    // stayed.
    let accepted = outposts
        .ok(
            "POST",
            START_ROUTE,
            Some(&start_request(later_asset, &key, None, Some(0))),
        )
        .await;
    assert!(
        !text(&accepted, "ConnectionId").is_empty(),
        "below the ceiling again a start is accepted, got {accepted}"
    );
}

/// Fill the store to [`MAX_RESOURCES`] connections, by copying the connection
/// `connection_id` names under further ids of its own.
///
/// Every copy names the same hardware the start named, which is what makes them
/// entries of that Outpost: they occupy slots the ceiling counts and nothing reads
/// them, so the only thing asserted about them is how many there are.
fn fill_to_ceiling(outposts: &Outposts, connection_id: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let template = store
        .connections
        .get(connection_id)
        .cloned()
        .expect("the start persisted its connection");
    for index in 0..MAX_RESOURCES - 1 {
        let mut copy = template.clone();
        copy.connection_id = format!("filler-{index:06}");
        store.connections.insert(copy.connection_id.clone(), copy);
    }
}
