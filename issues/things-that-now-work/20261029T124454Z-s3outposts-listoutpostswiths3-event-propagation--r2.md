Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 099b8d2e90cdf0281a13a19adea252c3b1691b86

# A declared (shared) Outpost entry is not overridden by the AWS Outposts control plane's own record, so a decommissioned / S3-less Outpost stays in the inventory forever

_Filed: 2026-10-29T12:44:54Z_
_Operation: ListOutpostsWithS3 | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `099b8d2e90cdf0281a13a19adea252c3b1691b86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::outpost_inventory` merges this service's store-held (operator-declared, "shared with the caller") Outposts with the ones the co-hosted AWS Outposts control plane holds S3 capacity on. Its own doc states that for an id BOTH sides know, the control plane wins because it "is the authority on that resource". The code only ever implements a POSITIVE override: it iterates `control_plane_outposts(engine)`, which is already filtered down to Outposts that are `Active`, not being decommissioned, and that have ordered S3 capacity (`s3_provisioned_outpost`), and inserts those over the declared map.

So in exactly the case where the authority says *no* — the control plane holds that `OutpostId` but it is `life_cycle_status != Active`, or `decommission_requested_at` is set, or no order bought S3 capacity for it — `control_plane_outposts` yields NO record for the id, nothing is inserted, and the stale declared entry survives untouched. The authority's negative answer is silently dropped and the operator's declaration wins, which is the opposite of the rule the function documents.

Consequences, all through the live HTTP surface:

* `ListOutpostsWithS3` keeps reporting the Outpost, with the operator-declared `CapacityInBytes` and `OwnerId`, after the control plane has been told to decommission it or after its only S3-capable order was cancelled.
* `resolve_outpost` keeps resolving it, so `CreateEndpoint` happily mints new endpoints (and commits a `/26` and a VPC mapping) on hardware the control plane reports as decommissioned or S3-less, and `ListSharedEndpoints`/`ListEndpoints` keep reporting them.
* `release_endpoints_of_absent_outposts` is driven off membership of this same merged inventory, so the "an Outpost that LEAVES that inventory takes the endpoints stored on it with it" rule never fires for a shadowed id — the decommission/cancel routes `tests/all/outpost_departure.rs` enumerates propagate for an un-shadowed Outpost and are inert for a shadowed one.

The fix is confined to the merge: the control plane's verdict for an id it holds has to be honoured in both directions, so a declared entry for an id the control plane holds without S3-on-Outposts capacity is removed from the inventory instead of being left as the answer.

## Evidence

The rule the function states — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:745-750:

    /// Resolved per call rather than copied into this service's store: AWS Outposts
    /// owns the Outpost, so what this service reports has to be what that control
    /// plane holds NOW — an Outpost created, ordered for or decommissioned there is one
    /// this service answers for from the next call on. A declared entry for an id the
    /// control plane also holds is overridden by the control plane's own record, which
    /// is the authority on that resource.

The code under it, which can only override POSITIVELY — src/state.rs:756-765:

    pub fn outpost_inventory<Eng: Send + Sync + 'static>(
        engine: &Eng,
        store: &ComAmazonawsS3outpostsS3outpostsState,
    ) -> OutpostInventory {
        let mut inventory = store.outposts.clone();
        for record in control_plane_outposts(engine) {
            inventory.insert(record.outpost_id.clone(), record);
        }
        inventory
    }

and the filter that makes `control_plane_outposts` yield nothing for exactly the ids whose authority verdict is negative — src/state.rs:833-838:

        if record.outpost.life_cycle_status.as_deref() != Some(outposts_state::LIFE_CYCLE_STATUS_ACTIVE)
        {
            return None;
        }
        if record.decommission_requested_at.is_some() {
            return None;
        }

(and src/state.rs:842-847, where an Outpost whose orders bought no S3 capacity gets `capacity_in_bytes = 0` and is then dropped by `OutpostRecord::provisioned`'s `capacity_in_bytes > 0` gate at src/state.rs:278-281.)

The same gate stated as ABSOLUTE by the module header — src/state.rs:41-47:

    //! Which of them belong in the inventory is GATED, not assumed: S3 capacity is
    //! installed on an Outpost by ordering hardware that carries it, so an Outpost
    //! enters this inventory only once the orders placed on it buy S3 storage
    //! capacity, and only while it is the "Active (not pending any provisioning nor
    //! decommissioned)" Outpost its own control plane reports — otherwise this
    //! service would report capacity, and endpoints, for hardware nobody ordered.

and the release rule that is driven off this same merged inventory — src/state.rs:36-39:

    //! created for or decommissioned through the `outposts` API is one this
    //! service answers for at the same instant. An Outpost that LEAVES that inventory
    //! takes the endpoints stored on it with it — they are named by it and reached
    //! only through it — which is what [`settled_inventory`] materialises on every

Test coverage of this seam: NONE. `tests/all/control_plane_inventory.rs:14-25` states rule 1 as "An Outpost the AWS Outposts control plane holds is reported by `ListOutpostsWithS3` exactly when it is active, not being decommissioned, and the orders placed on it bought S3 storage capacity", and rule 4 separately as "An Outpost of another account, shared through Resource Access Manager, is reported beside the control plane's own" — the two are only ever exercised on DISJOINT ids (the shared fixture uses its own `SHARED_OUTPOST_ID: &str = "op-2c3d4e5f607182930"`, tests/all/outpost_departure.rs:44, declared under `SHARING_ACCOUNT_ID`). No case declares an entry for an id the control plane also holds, so honouring the authority's negative verdict leaves every existing test passing.

## Steps to Reproduce

1. Stand up the emulator hosting BOTH services over one engine, as tests/all/control_plane_inventory.rs does, and provision an Outpost `X` through the AWS Outposts API with an order that buys S3 storage capacity (deliver_orders / Catalog in that suite).
2. Confirm GET /S3Outposts/ListOutpostsWithS3?maxResults=100 reports `X` with the capacity the orders bought, and that CreateEndpoint on `X` answers 200.
3. Also declare the SAME id in this service's store, the way the documented operator seam does: S3_OUTPOSTS_PROVISIONED_OUTPOSTS="X:26388279066624" (state.rs:146-164), or equivalently `store.provision_outpost(OutpostRecord::provisioned("X", <account>, 26388279066624, "us-east-1").unwrap())`.
4. Now make the control plane's verdict on `X` negative — request its decommission (so `decommission_requested_at` is set), or cancel its only S3-capable order (so its ordered S3 capacity is 0), exactly as the rows of tests/all/outpost_departure.rs do.
5. GET /S3Outposts/ListOutpostsWithS3?maxResults=100 again: `X` is STILL reported, now carrying the operator-declared CapacityInBytes instead of the control plane's. Expected per state.rs:745-750 and state.rs:41-47: the control plane is the authority on an id it holds, so `X` leaves the inventory.
6. POST /S3Outposts/CreateEndpoint with OutpostId=X: it answers 200 and commits a /26 plus a VPC mapping on an Outpost the control plane reports as decommissioned / S3-less, instead of the modelled 404.
7. Repeat steps 4-6 with the declaration removed and observe the contrast: without the shadowing entry `X` leaves the inventory immediately, every reference to it is the modelled 404, and `release_endpoints_of_absent_outposts` releases the endpoints it carried — so the propagation is suppressed purely by the merge order.
