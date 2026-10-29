Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# DeleteOutpost takes an Outpost's capacity tasks with it but leaves every connection started for its assets behind, dangling

_Filed: 2026-10-29T02:31:29Z_
_Operation: DeleteOutpost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteOutpost` removes the Outpost record and explicitly purges the capacity tasks nested under it, precisely so no record of a vanished Outpost is left behind. It does NOT touch `state.connections`, even though a `ConnectionRecord` exists only as the installation tunnel for ONE `AssetId` of ONE Outpost, and that asset is part of the Outpost record the delete just dropped.

After `DeleteOutpost`:

* `GetConnection` keeps answering 200 with the full tunnel (`ConnectionDetails`, `UnderlayIpAddress`) for an asset of an Outpost that no longer exists, and that no `ListAssets` / `GetOutpost` call can show the caller any more — the Outpost's `assets` vector went away with the record.
* the connection entry is unreachable for any purpose except that stale read, yet it still counts against the connection store's own ceiling that `StartConnection` refuses at (`store.connections.len() >= crate::state::MAX_RESOURCES`), so deleting Outposts never gives the capacity back.

This contradicts the crate's own stated invariant about what an asset is and what happens to it when its Outpost goes away, and it contradicts the cleanup the SAME handler performs for the other resource keyed off a now-gone Outpost. Nothing else in the crate ever removes a connection: `connections` is only ever inserted into (`start_connection.rs:218`) and read (`state.rs:2954`).

## Evidence

The crate's claim that an asset cannot outlive its Outpost — `services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:474-476`, on `OutpostRecord::assets`:

    /// is: an asset belongs to exactly ONE Outpost (the modelled route is
    /// `/outposts/{OutpostIdentifier}/assets`), so deleting the Outpost takes its
    /// assets with it.

and `state.rs:4072-4077`, on `unfinished_work_on_outpost`, which is the helper `DeleteOutpost` decides on:

    /// * the members of the Outpost's OWN record, which are attributes of the
    ///   Outpost rather than work on it and go away with it: its private
    ///   connectivity configuration, its renewal pricing, its installed assets and
    ///   the instances recorded as running on it. None has an identity outside the
    ///   Outpost, so removing the Outpost leaves nothing of them behind to dangle.

But a connection record DOES hold an identity outside the Outpost — `state.rs:1346-1348`:

    /// The `AssetId` of the Outpost server being installed — the asset the start
    /// resolved, so the record names the hardware the tunnel belongs to.
    pub asset_id: String,

The same handler already performs exactly this cleanup for the other child resource — `src/ops/delete_outpost.rs:207-215`:

        store.remove_outpost(&region, &outpost_id);
        // A capacity task exists only as a task OF an Outpost, so the Outpost going
        // away takes its tasks with it: nothing can name them any more (every
        // capacity-task route resolves the Outpost first), and leaving the records
        // behind would let a later Outpost's calls trip over them. Every task left
        // here is finished work — an unfinished one is the conflict above.
        store
            .capacity_tasks
            .retain(|_, task| task.outpost_id != outpost_id);

No equivalent line for `store.connections` exists; the only writes to it in the whole crate are `src/ops/start_connection.rs:196` (the ceiling test) and `src/ops/start_connection.rs:218` (the insert).

The connection the delete orphans was minted against an asset the delete removed: `StartConnection` resolves the asset through the Outpost (`src/ops/start_connection.rs:183`, `store.outpost_with_asset(&region, &input.asset_id)`), and `GetConnection` resolves nothing but the connection id (`src/ops/get_connection.rs:159-172`, "`ConnectionId` has ONE form, the opaque id the start minted, so there is nothing to resolve before the lookup").

Test coverage: `tests/all/outpost_server_connection.rs` covers start/get of a connection; `tests/all/site_and_outpost_lifecycle.rs` and `tests/all/delete_conflicts.rs` cover `DeleteOutpost`. No test exercises the two together, which is why the leak is unpinned.

## Steps to Reproduce

1. Create a site and an Outpost with SupportedHardwareType=SERVER, then place an order on it and let the order settle so the Outpost reaches the ACTIVE status and `settle_outposts` installs its hardware.
2. ListAssets on the Outpost and take an `AssetId` from the result.
3. StartConnection with that `AssetId` (plus a 44-character `ClientPublicKey`); keep the returned `ConnectionId`.
4. Cancel/complete all outstanding work and call DeleteOutpost on the Outpost (it succeeds: `unfinished_work_on_outpost` reports none, and the capacity tasks are purged).
5. GetOutpost and ListAssets on the Outpost: both answer 404 NotFoundException — the Outpost and its assets are gone.
6. GetConnection with the saved `ConnectionId`: it still answers 200 with the tunnel for an asset of the deleted Outpost, and the record still occupies one of the `MAX_RESOURCES` slots `StartConnection` refuses at.
